//! Offline, bounded workspace search.
//!
//! A synchronous, read-only scanner that turns workspace file candidates plus a
//! query into ordered hits. It is the domain half of the Find in Files
//! migration: the UI slice schedules it, groups hits and renders snippets.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::Regex;

use crate::git::WorkspaceFile;

mod ripgrep;

/// Largest file that is scanned; larger files are skipped.
pub const MAX_FILE_BYTES: u64 = 2 << 20;
/// Global hit budget for one search.
pub const MAX_MATCHES: usize = 300;
/// Matches collected per line (single-line query) or per file (multiline
/// query); mirrors the legacy scanner so huge lines cannot flood the result.
pub const MAX_MATCHES_PER_PASS: usize = 40;

/// Maximum line bytes retained in one hit snippet, ellipses excluded. This
/// bounds memory even when a match spans megabytes.
const MAX_SNIPPET_BYTES: usize = 320;
const SNIPPET_BEFORE: usize = 76;
const SNIPPET_AFTER: usize = 120;

/// Cooperative cancellation handle; clones share one flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Requests cancellation; in-flight searches stop at the next checkpoint.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

/// Search parameters.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions<'a> {
    /// Match case exactly; the default is case-insensitive.
    pub case_sensitive: bool,
    /// Treat `query` as a regular expression instead of literal text.
    pub regex: bool,
    /// Match only whole words using Unicode word boundaries.
    pub whole_word: bool,
    /// Comma separated basename globs: positive selectors and `!` exclusions.
    pub mask: Option<&'a str>,
}

/// Whether a search ran to completion or stopped on cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStatus {
    Completed,
    Cancelled,
}

/// One match, with the identity of the owning workspace file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_path: String,
    pub rel_path: String,
    /// Canonical path of the file that was actually scanned.
    pub abs_path: String,
    pub color_idx: usize,
    /// One-based line number; `\n` separates lines and `\r\n` counts as one.
    pub line: usize,
    /// One-based column in Unicode scalar values (chars), not bytes.
    pub column: usize,
    /// Zero-based column in UTF-16 code units, as NSH `EditorPosition` expects.
    pub column_utf16: usize,
    /// Bounded display text around the match, with `…` for clipped context.
    /// At most `MAX_SNIPPET_BYTES` of line data plus both ellipses.
    pub snippet: String,
    /// UTF-8 byte range of the displayed match inside `snippet`.
    pub match_in_snippet: Range<usize>,
    /// UTF-8 byte range of the snippet body covered in the logical line.
    pub snippet_in_line: Range<usize>,
    /// UTF-8 byte range of the match inside the logical line.
    pub match_in_line: Range<usize>,
    /// Exact UTF-8 byte range of the match in the file, CRLF and multiline
    /// included. A later Replace plan reopens the file and verifies the
    /// preimage at this range before writing.
    pub file_range: Range<u64>,
    /// Digest of the original on-disk bytes. Filled once per matched file.
    pub raw_sha256: String,
}

/// A preview uses the bounded file that was searched. Match offsets are
/// valid only for the exact on-disk text recorded by the search.
pub(crate) fn read_preview_file(hit: &SearchHit) -> Option<String> {
    let path = Path::new(&hit.abs_path);
    if fs::canonicalize(path).ok()? != path {
        return None;
    }
    let body = read_text(path)?;
    (crate::local_history::sha256_hex(body.as_bytes()) == hit.raw_sha256).then_some(body)
}

/// Result of one search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    /// Hits in candidate order, then document order; at most `MAX_MATCHES`.
    pub hits: Vec<SearchHit>,
    /// True when the global hit budget was reached.
    pub truncated: bool,
    pub status: SearchStatus,
}

impl SearchOutcome {
    fn empty() -> Self {
        Self::completed(Vec::new(), false)
    }

    fn completed(hits: Vec<SearchHit>, truncated: bool) -> Self {
        Self {
            hits,
            truncated,
            status: SearchStatus::Completed,
        }
    }

    fn cancelled(hits: Vec<SearchHit>) -> Self {
        Self {
            hits,
            truncated: false,
            status: SearchStatus::Cancelled,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.status == SearchStatus::Cancelled
    }
}

/// Errors raised before or while scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchError {
    InvalidQuery { message: String },
    InvalidMask { pattern: String, message: String },
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidQuery { message } => write!(f, "invalid search expression: {message}"),
            Self::InvalidMask { pattern, message } => {
                write!(f, "invalid file mask {pattern:?}: {message}")
            }
        }
    }
}

impl std::error::Error for SearchError {}

/// Scans `files` offline and returns ordered hits.
///
/// Candidates are visited in slice order and duplicate resolved paths are
/// scanned once. Every candidate is resolved with `fs::canonicalize`; a
/// candidate that resolves outside its `WorkspaceFile::repo_path` (or outside
/// `scope_dir`, when set) is skipped, so symlinks cannot read outside the
/// workspace. Scope checks are path-component based, never string prefixes.
/// `query` is literal unless `options.regex`; a literal containing `\n`
/// matches across lines treating `\r\n` and `\n` alike. The mask is a comma
/// separated list of basename globs where a leading `!` excludes; malformed
/// globs are reported.
///
/// Hits are one-based: `line` counts `\n` separators and `column` counts
/// Unicode scalar values (chars, not bytes) before the match. Byte ranges are
/// UTF-8 byte offsets. Files are opened and validated through the handle, then
/// read through `Read::take(MAX_FILE_BYTES + 1)`; binary (NUL byte), non-UTF-8
/// or oversized files are skipped. Snippets are bounded by
/// `MAX_SNIPPET_BYTES` of line data, each line/file contributes at most
/// `MAX_MATCHES_PER_PASS` matches and the search at most `MAX_MATCHES`, with
/// `truncated` set when that budget is hit. Cancellation is checked between
/// every candidate (including masked-out ones), line and match; a cancelled
/// search returns the hits found so far with `status ==
/// SearchStatus::Cancelled`.
pub fn search_files(
    files: &[WorkspaceFile],
    scope_dir: Option<&Path>,
    query: &str,
    options: &SearchOptions<'_>,
    cancel: &CancelToken,
) -> Result<SearchOutcome, SearchError> {
    search_with_backend(
        files,
        scope_dir,
        query,
        options,
        cancel,
        Some(Path::new("rg")),
    )
}

fn search_with_backend(
    files: &[WorkspaceFile],
    scope_dir: Option<&Path>,
    query: &str,
    options: &SearchOptions<'_>,
    cancel: &CancelToken,
    ripgrep: Option<&Path>,
) -> Result<SearchOutcome, SearchError> {
    let Some(mut scope) = SearchScope::new(scope_dir, options.mask)? else {
        return Ok(SearchOutcome::empty());
    };
    if query.is_empty() {
        return Ok(SearchOutcome::empty());
    }
    let re = build_regex(query, options)?;
    let multiline = query.contains('\n');
    let mut hits: Vec<SearchHit> = Vec::new();
    if cancel.is_cancelled() {
        return Ok(SearchOutcome::cancelled(hits));
    }
    // For literal queries the rg pattern and in-process match use the same
    // escaped text. Regex queries use the line and EOF semantics of scan().
    let mut rg = ripgrep.filter(|_| !options.regex);
    for chunk in files.chunks(128) {
        let mut resolved = Vec::new();
        for wf in chunk {
            if cancel.is_cancelled() {
                return Ok(SearchOutcome::cancelled(hits));
            }
            if let Some(path) = scope.resolve(wf, &candidate_path(wf)) {
                resolved.push((wf, path));
            }
        }
        let matching = rg.and_then(|binary| {
            let paths: Vec<_> = resolved
                .iter()
                .filter(|(_, path)| {
                    fs::metadata(path)
                        .is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES)
                })
                .map(|(_, path)| path.clone())
                .collect();
            match ripgrep::matching_files(
                binary,
                &paths,
                re.as_str(),
                options.case_sensitive,
                multiline,
                cancel,
            ) {
                Ok(matches) => Some(matches),
                Err(_) => {
                    rg = None;
                    None
                }
            }
        });
        for (wf, path) in resolved {
            if cancel.is_cancelled() {
                return Ok(SearchOutcome::cancelled(hits));
            }
            if matching
                .as_ref()
                .is_some_and(|matches| !matches.contains(&path))
            {
                continue;
            }
            let Some(text) = read_text(&path) else {
                continue;
            };
            let first_hit = hits.len();
            let result = scan(
                wf,
                &path.to_string_lossy(),
                &text,
                &re,
                multiline,
                cancel,
                &mut hits,
            );
            let new_hits = &mut hits[first_hit..];
            if !new_hits.is_empty() {
                let digest = crate::local_history::sha256_hex(text.as_bytes());
                for hit in new_hits {
                    hit.raw_sha256.clone_from(&digest);
                }
            }
            match result {
                ScanResult::Finished => {}
                ScanResult::Cancelled => return Ok(SearchOutcome::cancelled(hits)),
                ScanResult::Truncated => return Ok(SearchOutcome::completed(hits, true)),
            }
        }
    }
    Ok(SearchOutcome::completed(hits, false))
}

enum ScanResult {
    Finished,
    Cancelled,
    Truncated,
}

fn scan(
    wf: &WorkspaceFile,
    abs_path: &str,
    text: &str,
    re: &Regex,
    multiline: bool,
    cancel: &CancelToken,
    hits: &mut Vec<SearchHit>,
) -> ScanResult {
    if multiline {
        for found in re.find_iter(text).take(MAX_MATCHES_PER_PASS) {
            if cancel.is_cancelled() {
                return ScanResult::Cancelled;
            }
            hits.push(multiline_hit(
                wf,
                abs_path,
                text,
                found.start(),
                found.end(),
            ));
            if hits.len() >= MAX_MATCHES {
                return ScanResult::Truncated;
            }
        }
        return ScanResult::Finished;
    }
    let mut line_offset = 0usize;
    for (idx, raw) in text.split('\n').enumerate() {
        if cancel.is_cancelled() {
            return ScanResult::Cancelled;
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let mut byte_cursor = 0usize;
        let mut char_cursor = 0usize;
        for found in re.find_iter(line).take(MAX_MATCHES_PER_PASS) {
            if cancel.is_cancelled() {
                return ScanResult::Cancelled;
            }
            let start = found.start();
            let end = found.end();
            if start >= byte_cursor && start <= line.len() {
                char_cursor += line[byte_cursor..start].chars().count();
                byte_cursor = start;
            }
            hits.push(make_hit(
                wf,
                abs_path,
                idx + 1,
                char_cursor + 1,
                line,
                start,
                end,
                ((line_offset + start) as u64)..((line_offset + end) as u64),
            ));
            if hits.len() >= MAX_MATCHES {
                return ScanResult::Truncated;
            }
        }
        line_offset += raw.len() + 1;
    }
    ScanResult::Finished
}

pub(crate) fn build_regex(query: &str, options: &SearchOptions<'_>) -> Result<Regex, SearchError> {
    let pattern = if options.regex {
        query.to_string()
    } else {
        literal_pattern(query)
    };
    let make_pattern = |end_of_pattern: &str| {
        let pattern = if options.whole_word {
            format!(r"(?u:\b{{start-half}})(?:{pattern}{end_of_pattern})(?u:\b{{end-half}})")
        } else {
            pattern.clone()
        };
        if options.case_sensitive {
            pattern
        } else {
            format!("(?i){pattern}")
        }
    };
    let compiled = Regex::new(&make_pattern(""));
    // An end-of-pattern comment in (?x) mode otherwise eats the closing group.
    let compiled = match compiled {
        Err(_) if options.whole_word && options.regex && Regex::new(query).is_ok() => {
            Regex::new(&make_pattern("\n"))
        }
        other => other,
    };
    compiled.map_err(|err| SearchError::InvalidQuery {
        message: err.to_string(),
    })
}

fn literal_pattern(query: &str) -> String {
    let normalized = query.replace("\r\n", "\n");
    normalized
        .split('\n')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(r"\r?\n")
}

fn candidate_path(wf: &WorkspaceFile) -> PathBuf {
    if !wf.abs_path.is_empty() {
        return PathBuf::from(&wf.abs_path);
    }
    let rel = if std::path::MAIN_SEPARATOR == '/' {
        wf.rel_path.clone()
    } else {
        wf.rel_path.replace('/', std::path::MAIN_SEPARATOR_STR)
    };
    Path::new(&wf.repo_path).join(rel)
}

struct SearchScope {
    directory: Option<PathBuf>,
    mask: Option<SearchMask>,
    roots: HashMap<String, Option<PathBuf>>,
    seen: HashSet<PathBuf>,
}

impl SearchScope {
    fn new(directory: Option<&Path>, mask: Option<&str>) -> Result<Option<Self>, SearchError> {
        let mask = mask
            .filter(|mask| !mask.trim().is_empty())
            .map(SearchMask::parse)
            .transpose()?;
        let directory = if let Some(dir) = directory.filter(|dir| !dir.as_os_str().is_empty()) {
            let Ok(dir) = fs::canonicalize(dir) else {
                return Ok(None);
            };
            Some(dir)
        } else {
            None
        };
        Ok(Some(Self {
            directory,
            mask,
            roots: HashMap::new(),
            seen: HashSet::new(),
        }))
    }

    fn resolve(&mut self, file: &WorkspaceFile, path: &Path) -> Option<PathBuf> {
        if path.as_os_str().is_empty()
            || path.as_os_str() == OsStr::new(".")
            || self.mask.as_ref().is_some_and(|mask| !mask.matches(path))
        {
            return None;
        }
        let resolved = resolve_candidate(file, path, &mut self.roots)?;
        if self
            .directory
            .as_ref()
            .is_some_and(|dir| !resolved.starts_with(dir))
            || !self.seen.insert(resolved.clone())
        {
            return None;
        }
        Some(resolved)
    }
}

pub(crate) fn files_in_scope(
    files: &[WorkspaceFile],
    directory: Option<&Path>,
    mask: Option<&str>,
) -> Result<Vec<WorkspaceFile>, SearchError> {
    let Some(mut scope) = SearchScope::new(directory, mask)? else {
        return Ok(Vec::new());
    };
    let mut scoped = Vec::new();
    for file in files {
        let path = candidate_path(file);
        if !fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file())
            || scope.resolve(file, &path).is_none()
        {
            continue;
        }
        let mut file = file.clone();
        file.abs_path = path.to_string_lossy().into_owned();
        scoped.push(file);
    }
    Ok(scoped)
}

fn resolve_candidate(
    wf: &WorkspaceFile,
    raw: &Path,
    roots: &mut HashMap<String, Option<PathBuf>>,
) -> Option<PathBuf> {
    let resolved = fs::canonicalize(raw).ok()?;
    let root = if wf.repo_path.is_empty() {
        workspace_root_from(raw)?
    } else {
        match roots.get(&wf.repo_path).cloned() {
            Some(root) => root?,
            None => {
                let root = fs::canonicalize(Path::new(&wf.repo_path)).ok();
                roots.insert(wf.repo_path.clone(), root.clone());
                root?
            }
        }
    };
    if !resolved.starts_with(&root) {
        return None;
    }
    Some(resolved)
}

fn workspace_root_from(raw: &Path) -> Option<PathBuf> {
    let parent = match raw.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::canonicalize(parent).ok()
}

fn read_text(path: &Path) -> Option<String> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut data = Vec::with_capacity(meta.len().min(64 * 1024) as usize);
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut data).ok()?;
    if data.len() as u64 > MAX_FILE_BYTES || data.contains(&0) {
        return None;
    }
    String::from_utf8(data).ok()
}

fn multiline_hit(
    wf: &WorkspaceFile,
    abs_path: &str,
    text: &str,
    start: usize,
    end: usize,
) -> SearchHit {
    let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(text.len());
    let raw = &text[line_start..line_end];
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    let line_no = 1 + text[..line_start].matches('\n').count();
    let local_start = (start - line_start).min(line.len());
    let local_end = (end - line_start).min(line.len()).max(local_start);
    let column = line[..local_start].chars().count() + 1;
    make_hit(
        wf,
        abs_path,
        line_no,
        column,
        line,
        local_start,
        local_end,
        (start as u64)..(end as u64),
    )
}

fn make_hit(
    wf: &WorkspaceFile,
    abs_path: &str,
    line_no: usize,
    column: usize,
    line: &str,
    match_start: usize,
    match_end: usize,
    file_range: Range<u64>,
) -> SearchHit {
    let start = floor_boundary(line, match_start.min(line.len()));
    let end = ceil_boundary(line, match_end.max(start).min(line.len()));
    let (snippet, match_in_snippet, snippet_in_line) = build_snippet(line, start, end);
    SearchHit {
        repo_id: wf.repo_id.clone(),
        repo_name: wf.repo_name.clone(),
        repo_path: wf.repo_path.clone(),
        rel_path: wf.rel_path.clone(),
        abs_path: abs_path.to_string(),
        color_idx: wf.color_idx,
        line: line_no,
        column,
        // 只做 UTF-16 码元计数，不复制整行。
        column_utf16: line[..start].encode_utf16().count(),
        snippet,
        match_in_snippet,
        snippet_in_line,
        match_in_line: start..end,
        file_range,
        raw_sha256: String::new(),
    }
}

fn floor_boundary(text: &str, mut at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil_boundary(text: &str, mut at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

fn build_snippet(line: &str, start: usize, end: usize) -> (String, Range<usize>, Range<usize>) {
    let from = floor_boundary(line, start.saturating_sub(SNIPPET_BEFORE));
    let budget_to = (from + MAX_SNIPPET_BYTES).min(line.len());
    let mut to = end.saturating_add(SNIPPET_AFTER).min(budget_to);
    if to <= start {
        to = budget_to;
    }
    to = floor_snippet_end(line, to, from);
    let prefix = if from > 0 { "…" } else { "" };
    let suffix = if to < line.len() { "…" } else { "" };
    let mut snippet = String::with_capacity(prefix.len() + (to - from) + suffix.len());
    snippet.push_str(prefix);
    snippet.push_str(&line[from..to]);
    snippet.push_str(suffix);
    let shown_end = end.min(to).max(start);
    let match_from = prefix.len() + start - from;
    let match_to = prefix.len() + shown_end - from;
    (snippet, match_from..match_to, from..to)
}

fn floor_snippet_end(line: &str, mut to: usize, from: usize) -> usize {
    if to >= line.len() {
        return line.len();
    }
    while to > from && !line.is_char_boundary(to) {
        to -= 1;
    }
    to
}

#[derive(Debug, Clone, Default)]
struct SearchMask {
    include: Vec<Regex>,
    exclude: Vec<Regex>,
}

impl SearchMask {
    fn parse(input: &str) -> Result<Self, SearchError> {
        let mut mask = Self::default();
        for raw in input.split(',') {
            let part = raw.trim();
            let (negative, pattern) = match part.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, part),
            };
            if pattern.is_empty() {
                continue;
            }
            let re = compile_glob(pattern).map_err(|message| SearchError::InvalidMask {
                pattern: pattern.to_string(),
                message,
            })?;
            if negative {
                mask.exclude.push(re);
            } else {
                mask.include.push(re);
            }
        }
        Ok(mask)
    }

    fn matches(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return self.include.is_empty();
        };
        if self.exclude.iter().any(|pattern| pattern.is_match(name)) {
            return false;
        }
        if self.include.is_empty() {
            return true;
        }
        self.include.iter().any(|pattern| pattern.is_match(name))
    }
}

/// Compiles one Go-style basename glob (`*`, `?`, `[...]`, `\` escapes) to an
/// anchored regex. Matching a basename means `/` never occurs; `*`/`?` still
/// exclude it. The regex crate keeps matching linear-time for adversarial
/// patterns, and patterns are compiled once per search.
fn compile_glob(glob: &str) -> Result<Regex, String> {
    let chars: Vec<char> = glob.chars().collect();
    let mut pattern = String::with_capacity(glob.len() + 16);
    pattern.push_str("(?s)^(?:");
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                pattern.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                pattern.push_str("[^/]");
                i += 1;
            }
            '\\' => {
                if i + 1 >= chars.len() {
                    return Err("dangling escape".to_string());
                }
                pattern.push_str(&regex::escape(&chars[i + 1].to_string()));
                i += 2;
            }
            '[' => {
                i = push_class(&chars, i, &mut pattern)?;
            }
            ch => {
                pattern.push_str(&regex::escape(&ch.to_string()));
                i += 1;
            }
        }
    }
    pattern.push_str(")$");
    Regex::new(&pattern).map_err(|err| err.to_string())
}

fn push_class(chars: &[char], open: usize, out: &mut String) -> Result<usize, String> {
    let mut i = open + 1;
    let mut body = String::new();
    if i < chars.len() && (chars[i] == '^' || chars[i] == '!') {
        body.push('^');
        i += 1;
    }
    let body_start = i;
    while i < chars.len() && chars[i] != ']' {
        if chars[i] == '\\' {
            if i + 1 >= chars.len() {
                return Err("dangling escape in character class".to_string());
            }
            push_class_char(&mut body, chars[i + 1]);
            i += 2;
        } else if chars[i] == '-' && i > body_start && i + 1 < chars.len() && chars[i + 1] != ']' {
            body.push('-');
            i += 1;
        } else {
            push_class_char(&mut body, chars[i]);
            i += 1;
        }
    }
    if i >= chars.len() {
        return Err("unterminated character class".to_string());
    }
    if body.is_empty() || body.as_str() == "^" {
        return Err("empty character class".to_string());
    }
    out.push('[');
    out.push_str(&body);
    out.push(']');
    Ok(i + 1)
}

fn push_class_char(out: &mut String, ch: char) {
    if matches!(ch, '\\' | ']' | '^' | '-' | '[') {
        out.push('\\');
    }
    out.push(ch);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "omp-search-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(root: &Path, rel: &str, data: &[u8]) -> PathBuf {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, data).expect("write fixture");
        path
    }

    fn workspace_file(root: &Path, rel: &str) -> WorkspaceFile {
        let abs = root.join(rel);
        WorkspaceFile {
            repo_id: "repo-1".to_string(),
            repo_name: "repo".to_string(),
            repo_path: root.display().to_string(),
            rel_path: rel.to_string(),
            abs_path: abs.display().to_string(),
            color_idx: 0,
        }
    }

    fn run(
        files: &[WorkspaceFile],
        scope: Option<&Path>,
        query: &str,
        options: &SearchOptions<'_>,
    ) -> SearchOutcome {
        search_files(files, scope, query, options, &CancelToken::new()).expect("search succeeds")
    }

    #[test]
    fn ripgrep_and_fallback_preserve_multiline_crlf_and_unicode_hits() {
        let dir = TempDir::new("ripgrep-parity");
        let root = dir.path().canonicalize().unwrap();
        let matching = write_file(&root, "src/甲.go", "😀 éneedle\r\nNEXT\r\n".as_bytes());
        let unmatched = write_file(&root, "src/.skip.go", b"needle\nnot the next line\n");
        write_file(&root, "src/ignore.txt", b"needle\nNEXT\n");
        write_file(&root, "other.go", b"needle\nNEXT\n");
        let files = ["src/.skip.go", "src/甲.go", "src/ignore.txt", "other.go"]
            .into_iter()
            .map(|path| workspace_file(&root, path))
            .collect::<Vec<_>>();
        let options = SearchOptions {
            mask: Some("*.go"),
            ..SearchOptions::default()
        };
        let scope = root.join("src");
        let query = "needle\nnext";
        let cancel = CancelToken::new();
        let builtin =
            search_with_backend(&files, Some(&scope), query, &options, &cancel, None).unwrap();
        assert_eq!(builtin.hits.len(), 1);
        assert_eq!(builtin.hits[0].column_utf16, 4);
        assert_eq!(
            builtin.hits[0].file_range,
            "😀 é".len() as u64.."😀 éneedle\r\nNEXT".len() as u64,
        );

        let absent = root.join("no-ripgrep-here");
        let recovered = search_with_backend(
            &files,
            Some(&scope),
            query,
            &options,
            &cancel,
            Some(&absent),
        )
        .unwrap();
        assert_eq!(recovered, builtin);

        let binary = Path::new("rg");
        let found = ripgrep::matching_files(
            binary,
            &[unmatched, matching.clone()],
            build_regex(query, &options).unwrap().as_str(),
            false,
            true,
            &cancel,
        );
        if found
            .as_ref()
            .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
            && std::env::var_os("NEMOROUS_REQUIRE_RG").is_none()
        {
            return;
        }
        assert_eq!(found.unwrap(), HashSet::from([matching]));
        let accelerated =
            search_with_backend(&files, Some(&scope), query, &options, &cancel, Some(binary))
                .unwrap();
        assert_eq!(accelerated, builtin);
    }

    #[test]
    fn literal_case_and_regex_modes() {
        let dir = TempDir::new("modes");
        let root = dir.path();
        write_file(root, "a.go", b"package main\nfunc Needle() {}\n");
        write_file(root, "b.txt", b"first\nneedle here\nlast\n");
        let files = vec![workspace_file(root, "a.go"), workspace_file(root, "b.txt")];

        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.status, SearchStatus::Completed);
        assert!(!out.truncated);
        assert_eq!(
            out.hits.len(),
            2,
            "case-insensitive literal hits: {:?}",
            out.hits
        );
        assert_eq!(out.hits[0].rel_path, "a.go");
        assert_eq!(out.hits[0].line, 2);
        assert_eq!(out.hits[0].column, 6);
        assert_eq!(out.hits[1].rel_path, "b.txt");
        assert_eq!(out.hits[1].line, 2);

        let sensitive = SearchOptions {
            case_sensitive: true,
            ..SearchOptions::default()
        };
        let out = run(&files, None, "needle", &sensitive);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].rel_path, "b.txt");

        let regex = SearchOptions {
            case_sensitive: true,
            regex: true,
            ..SearchOptions::default()
        };
        let out = run(&files, None, r"func\s+Needle", &regex);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].rel_path, "a.go");
        assert_eq!(out.hits[0].snippet, "func Needle() {}");
        assert_eq!(
            &out.hits[0].snippet[out.hits[0].match_in_snippet.clone()],
            "func Needle"
        );
        assert_eq!(out.hits[0].match_in_line, 0..11);

        let invalid = SearchOptions {
            regex: true,
            ..SearchOptions::default()
        };
        match search_files(&files, None, "(", &invalid, &CancelToken::new()) {
            Err(SearchError::InvalidQuery { .. }) => {}
            other => panic!("expected invalid query, got {other:?}"),
        }
    }

    #[test]
    fn excluded_basename_and_mask_errors() {
        let dir = TempDir::new("mask");
        let root = dir.path();
        write_file(root, "src/main.rs", b"needle\n");
        write_file(root, "src/debug.log", b"needle\n");
        write_file(root, "src/notes.txt", b"needle\n");
        let files = vec![
            workspace_file(root, "src/main.rs"),
            workspace_file(root, "src/debug.log"),
            workspace_file(root, "src/notes.txt"),
        ];

        let excluded = SearchOptions {
            mask: Some("!*.log"),
            ..SearchOptions::default()
        };
        let out = run(&files, None, "needle", &excluded);
        let names: Vec<&str> = out.hits.iter().map(|hit| hit.rel_path.as_str()).collect();
        assert_eq!(names, ["src/main.rs", "src/notes.txt"]);

        let only_rust = SearchOptions {
            mask: Some("*.rs"),
            ..SearchOptions::default()
        };
        let out = run(&files, None, "needle", &only_rust);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].rel_path, "src/main.rs");

        let none = SearchOptions {
            mask: Some("*.rs, !main.rs"),
            ..SearchOptions::default()
        };
        assert!(run(&files, None, "needle", &none).hits.is_empty());

        let broken = SearchOptions {
            mask: Some("*.rs,["),
            ..SearchOptions::default()
        };
        match search_files(&files, None, "needle", &broken, &CancelToken::new()) {
            Err(SearchError::InvalidMask { pattern, .. }) => assert_eq!(pattern, "["),
            other => panic!("expected invalid mask, got {other:?}"),
        }
    }

    #[test]
    fn nested_scope_is_component_based() {
        let dir = TempDir::new("scope");
        let root = dir.path();
        write_file(root, "a/one.txt", b"needle\n");
        write_file(root, "a/nested/two.txt", b"needle\n");
        write_file(root, "ab/three.txt", b"needle\n");
        let files = vec![
            workspace_file(root, "a/one.txt"),
            workspace_file(root, "a/nested/two.txt"),
            workspace_file(root, "ab/three.txt"),
        ];

        let scope = root.join("a");
        let out = run(&files, Some(&scope), "needle", &SearchOptions::default());
        let rels: Vec<&str> = out.hits.iter().map(|hit| hit.rel_path.as_str()).collect();
        assert_eq!(rels, ["a/one.txt", "a/nested/two.txt"]);

        let nested = root.join("a/nested");
        let out = run(&files, Some(&nested), "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].rel_path, "a/nested/two.txt");

        let sibling = root.join("ab");
        let out = run(&files, Some(&sibling), "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].rel_path, "ab/three.txt");
    }

    #[test]
    fn unicode_columns_and_snippet_ranges() {
        let dir = TempDir::new("unicode");
        let root = dir.path();
        write_file(root, "u.txt", "héllo 🌍 needle\n".as_bytes());
        let files = vec![workspace_file(root, "u.txt")];
        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.line, 1);
        assert_eq!(hit.column, 9, "column counts Unicode scalar values");
        assert_eq!(hit.match_in_line, 12..18);
        assert_eq!(hit.file_range, 12..18);
        assert_eq!(&hit.snippet[hit.match_in_snippet.clone()], "needle");

        let line = format!("{}needle{}", "前".repeat(60), "後".repeat(60));
        write_file(root, "wide.txt", line.as_bytes());
        let files = vec![workspace_file(root, "wide.txt")];
        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert!(hit.snippet.starts_with('…'), "snippet: {:?}", hit.snippet);
        assert!(hit.snippet.ends_with('…'), "snippet: {:?}", hit.snippet);
        assert_eq!(
            hit.snippet.get(hit.match_in_snippet.clone()),
            Some("needle")
        );
        assert_eq!(hit.snippet_in_line, 102..306);
        assert!(hit.snippet.is_char_boundary(hit.match_in_snippet.start));
        assert!(hit.snippet.is_char_boundary(hit.match_in_snippet.end));
    }

    #[test]
    fn crlf_multiline_literal_and_line_scrubbing() {
        let dir = TempDir::new("crlf");
        let root = dir.path();
        write_file(root, "docs/report.md", b"first\r\nneedle\r\nsecond\r\n");
        let files = vec![workspace_file(root, "docs/report.md")];

        let out = run(
            &files,
            None,
            "first\nneedle\nsecond",
            &SearchOptions::default(),
        );
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.line, 1);
        assert_eq!(hit.column, 1);
        assert_eq!(hit.match_in_line, 0..5);
        assert_eq!(hit.file_range, 0..21);
        assert_eq!(hit.snippet, "first");

        let out = run(&files, None, "first\r\nneedle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].line, 1);

        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].line, 2);
        assert_eq!(out.hits[0].snippet, "needle");
        assert!(!out.hits[0].snippet.contains('\r'));
        assert_eq!(out.hits[0].file_range, 7..13);
    }

    #[test]
    fn cancellation_is_reported_explicitly() {
        let dir = TempDir::new("cancel");
        let root = dir.path();
        write_file(root, "a.txt", b"needle\n");
        write_file(root, "b.txt", b"needle\n");
        let files = vec![workspace_file(root, "a.txt"), workspace_file(root, "b.txt")];

        let cancel = CancelToken::new();
        let clone = cancel.clone();
        clone.cancel();
        let out = search_files(&files, None, "needle", &SearchOptions::default(), &cancel)
            .expect("cancelled search is not an error");
        assert_eq!(out.status, SearchStatus::Cancelled);
        assert!(out.is_cancelled());
        assert!(out.hits.is_empty());

        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.status, SearchStatus::Completed);
        assert_eq!(out.hits.len(), 2);
    }

    #[test]
    fn skips_binary_and_oversized_files() {
        let dir = TempDir::new("caps");
        let root = dir.path();
        write_file(root, "small.txt", b"needle\n");
        write_file(root, "binary.dat", b"needle\0binary\n");
        let mut big = vec![b'a'; MAX_FILE_BYTES as usize + 1];
        big[..6].copy_from_slice(b"needle");
        write_file(root, "big.txt", &big);
        let files = vec![
            workspace_file(root, "small.txt"),
            workspace_file(root, "binary.dat"),
            workspace_file(root, "big.txt"),
        ];

        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].rel_path, "small.txt");
    }

    #[test]
    fn global_match_cap_and_stable_order() {
        let dir = TempDir::new("cap");
        let root = dir.path();
        let mut content = String::new();
        for i in 0..350 {
            content.push_str(&format!("line {i} needle\n"));
        }
        write_file(root, "many.txt", content.as_bytes());
        let wf = workspace_file(root, "many.txt");
        let files = vec![wf.clone(), wf];

        let out = run(&files, None, "needle", &SearchOptions::default());
        assert!(out.truncated);
        assert_eq!(out.hits.len(), MAX_MATCHES);
        for (idx, hit) in out.hits.iter().enumerate() {
            assert_eq!(hit.line, idx + 1);
            assert_eq!(hit.rel_path, "many.txt");
        }

        write_file(root, "dense.txt", "needle ".repeat(50).as_bytes());
        let files = vec![workspace_file(root, "dense.txt")];
        let out = run(&files, None, "needle", &SearchOptions::default());
        assert!(!out.truncated);
        assert_eq!(out.hits.len(), MAX_MATCHES_PER_PASS);
    }

    #[test]
    fn exact_file_ranges_across_crlf_and_multiline() {
        let dir = TempDir::new("ranges");
        let root = dir.path();
        let data = b"aa\r\nneedle\r\nbb\r\n";
        write_file(root, "crlf.txt", data);
        let files = vec![workspace_file(root, "crlf.txt")];

        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.line, 2);
        assert_eq!(hit.column, 1);
        assert_eq!(hit.file_range, 4..10);
        assert_eq!(&data[4..10], b"needle");

        let out = run(&files, None, "needle\r\nbb", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.line, 2);
        assert_eq!(hit.column, 1);
        assert_eq!(hit.file_range, 4..14);
        assert_eq!(&data[4..14], b"needle\r\nbb");
        assert_eq!(&hit.snippet[hit.match_in_snippet.clone()], "needle");
    }

    #[test]
    fn snippet_is_bounded_for_long_lines_and_huge_matches() {
        let dir = TempDir::new("bounded");
        let root = dir.path();
        let long = format!("{}needle{}", "a".repeat(900_000), "b".repeat(900_000));
        write_file(root, "long.txt", long.as_bytes());
        let files = vec![workspace_file(root, "long.txt")];
        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.file_range, 900_000..900_006);
        assert_eq!(
            hit.snippet.get(hit.match_in_snippet.clone()),
            Some("needle")
        );
        assert!(hit.snippet.len() <= MAX_SNIPPET_BYTES + 2 * '…'.len_utf8());
        assert!(hit.snippet.starts_with('…'));
        assert!(hit.snippet.is_char_boundary(hit.match_in_snippet.start));
        assert!(hit.snippet.is_char_boundary(hit.match_in_snippet.end));

        write_file(root, "huge.txt", "a".repeat(1_000_000).as_bytes());
        let files = vec![workspace_file(root, "huge.txt")];
        let opts = SearchOptions {
            case_sensitive: true,
            regex: true,
            ..SearchOptions::default()
        };
        let out = run(&files, None, "a+", &opts);
        assert_eq!(out.hits.len(), 1);
        let hit = &out.hits[0];
        assert_eq!(hit.file_range, 0..1_000_000);
        assert_eq!(hit.match_in_line, 0..1_000_000);
        assert!(hit.snippet.len() <= MAX_SNIPPET_BYTES + '…'.len_utf8());
        assert!(hit.snippet.ends_with('…'));
        assert!(hit.match_in_snippet.end <= MAX_SNIPPET_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_resolved_and_cannot_escape_the_workspace() {
        let dir = TempDir::new("symlink");
        let root = dir.path();
        let outside = TempDir::new("symlink-outside");
        write_file(root, "real.txt", b"needle\n");
        write_file(outside.path(), "secret.txt", b"needle\n");
        std::os::unix::fs::symlink(root.join("real.txt"), root.join("inside-link.txt"))
            .expect("symlink inside");
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), root.join("escape.txt"))
            .expect("symlink escape");

        let files = vec![
            workspace_file(root, "inside-link.txt"),
            workspace_file(root, "escape.txt"),
        ];
        let out = run(&files, None, "needle", &SearchOptions::default());
        assert_eq!(out.hits.len(), 1);
        let expected = std::fs::canonicalize(root.join("real.txt")).expect("canonical target");
        assert_eq!(Path::new(&out.hits[0].abs_path), expected.as_path());
    }
}
