use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use regex::Regex;

use crate::git::{self, LineEnding, ReviewVersion, SaveReviewOutcome, WorkspaceFile};
use crate::local_history::LocalHistoryStore;
use crate::search::{self, SearchHit, SearchOptions};

/// 替换结果超过该字节数即中止，避免一次性分配/写出超大文本。
const MAX_REPLACE_OUTPUT_BYTES: usize = search::MAX_FILE_BYTES as usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceQuery {
    pub pattern: String,
    pub replacement: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub mask: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Preview {
    pub file: WorkspaceFile,
    pub expected: ReviewVersion,
    pub line_ending: LineEnding,
    pub matches: usize,
    /// (one-based line, zero-based UTF-16 column) of the selected match.
    pub chosen: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    query: ReplaceQuery,
    previews: Vec<Preview>,
}

#[derive(Debug, Clone)]
pub struct FileResult {
    pub file: WorkspaceFile,
    pub changed: usize,
    pub error: Option<String>,
}

impl Plan {
    pub fn file_count(&self) -> usize {
        self.previews.len()
    }

    pub fn match_count(&self) -> usize {
        self.previews.iter().map(Preview::planned).sum()
    }

    pub fn previews(&self) -> &[Preview] {
        &self.previews
    }

    pub fn query(&self) -> &ReplaceQuery {
        &self.query
    }
}

impl Preview {
    /// 计划实际替换的次数：单选命中只替换一次，全量替换等于匹配数。
    pub fn planned(&self) -> usize {
        if self.chosen.is_some() {
            1
        } else {
            self.matches
        }
    }
}

pub fn prepare_all(
    files: &[WorkspaceFile],
    scope: Option<&Path>,
    query: ReplaceQuery,
    dirty: &HashSet<String>,
) -> Result<Plan> {
    if query.pattern.is_empty() {
        bail!("搜索内容为空，未生成替换计划");
    }
    let candidates = search::files_in_scope(files, scope, query.mask.as_deref())
        .map_err(|err| anyhow!("筛选替换范围失败: {err}"))?;
    let mut previews = Vec::new();
    {
        let matcher = Matcher::new(&query)?;
        for file in candidates {
            if dirty.contains(&file.key()) {
                continue;
            }
            // 预检必须统计全部候选文件，不能沿用搜索结果条数上限。
            let read = match read_worktree(&file) {
                Ok(read) => read,
                Err(_) => continue,
            };
            if read.binary || read.too_large || !read.editable {
                continue;
            }
            let matches = matcher.count(&read.text);
            if matches == 0 {
                continue;
            }
            previews.push(Preview {
                file,
                expected: read.version,
                line_ending: read.line_ending,
                matches,
                chosen: None,
            });
        }
    }
    Ok(Plan { query, previews })
}

pub fn prepare_one(
    files: &[WorkspaceFile],
    scope: Option<&Path>,
    query: ReplaceQuery,
    hit: &SearchHit,
    dirty: &HashSet<String>,
) -> Result<Plan> {
    if query.pattern.is_empty() {
        bail!("搜索内容为空，未生成替换计划");
    }
    let key = format!("{}::{}", hit.repo_id, hit.rel_path);
    if dirty.contains(&key) {
        bail!("文件有未保存的修改，无法替换");
    }
    let file = search::files_in_scope(files, scope, query.mask.as_deref())
        .map_err(|err| anyhow!("筛选替换范围失败: {err}"))?
        .into_iter()
        .find(|file| {
            file.key() == key
                && std::fs::canonicalize(&file.abs_path)
                    .is_ok_and(|path| path == Path::new(&hit.abs_path))
        })
        .ok_or_else(|| anyhow!("命中文件不在当前替换范围内"))?;
    let read = read_worktree(&file)?;
    if read.version.sha256 != hit.raw_sha256 {
        bail!("文件内容自搜索后已变化，请重新搜索");
    }
    if read.binary || read.too_large || !read.editable {
        bail!("当前文件不可编辑，无法替换");
    }
    let matcher = Matcher::new(&query)?;
    let target = (hit.line, hit.column_utf16);
    if !matcher.has_match_at(&read.text, target) {
        bail!("命中位置与当前文件内容不一致，请重新搜索");
    }
    Ok(Plan {
        query,
        previews: vec![Preview {
            file,
            expected: read.version,
            line_ending: read.line_ending,
            matches: 1,
            chosen: Some(target),
        }],
    })
}

pub fn execute(
    plan: &Plan,
    dirty: &HashSet<String>,
    history: Option<&LocalHistoryStore>,
) -> Vec<FileResult> {
    let matcher = Matcher::new(&plan.query);
    let mut results = Vec::with_capacity(plan.previews.len());
    for preview in &plan.previews {
        let mut result = FileResult {
            file: preview.file.clone(),
            changed: 0,
            error: None,
        };
        let matcher = match &matcher {
            Ok(matcher) => matcher,
            Err(err) => {
                result.error = Some(err.to_string());
                results.push(result);
                continue;
            }
        };
        if dirty.contains(&preview.file.key()) {
            result.error = Some("文件有未保存的修改，已跳过".into());
            results.push(result);
            continue;
        }
        let read = match read_worktree(&preview.file) {
            Ok(read) => read,
            Err(err) => {
                result.error = Some(err.to_string());
                results.push(result);
                continue;
            }
        };
        if read.version.sha256 != preview.expected.sha256 {
            result.error = Some("文件内容自预检后已变化，已跳过".into());
            results.push(result);
            continue;
        }
        if read.version != preview.expected {
            result.error = Some("文件元数据自预检后已变化，已跳过".into());
            results.push(result);
            continue;
        }
        let (new_text, replaced) = match matcher.replace(&read.text, preview.chosen) {
            Ok(out) => out,
            Err(err) => {
                result.error = Some(err.to_string());
                results.push(result);
                continue;
            }
        };
        if replaced == 0 {
            result.error = Some("未找到可替换的内容，已跳过".into());
            results.push(result);
            continue;
        }
        if new_text == read.text {
            results.push(result);
            continue;
        }
        let outcome = git::save_review_text_in_repo(
            &preview.file.repo_id,
            repo_root(&preview.file),
            &preview.file.rel_path,
            Path::new(&preview.file.abs_path),
            &preview.expected,
            preview.line_ending,
            &new_text,
            history,
        );
        match outcome {
            SaveReviewOutcome::Saved { .. } => result.changed = replaced,
            SaveReviewOutcome::Conflict { .. } => {
                result.error = Some("文件已在磁盘上发生变化，已跳过".into())
            }
            SaveReviewOutcome::Rejected { reason } => result.error = Some(reason),
        }
        results.push(result);
    }
    results
}

fn repo_root(file: &WorkspaceFile) -> Option<&Path> {
    if file.repo_path.is_empty() {
        None
    } else {
        Some(Path::new(&file.repo_path))
    }
}

fn read_worktree(file: &WorkspaceFile) -> Result<git::WorktreeRead> {
    let read = git::read_review_worktree(Path::new(&file.abs_path));
    if !read.editable {
        let reason = read.readonly_reason.as_deref().unwrap_or("文件不可编辑");
        bail!("{}: {reason}", file.rel_path);
    }
    Ok(read)
}

struct Matcher<'q> {
    query: &'q ReplaceQuery,
    regex: Regex,
    multiline: bool,
}

impl<'q> Matcher<'q> {
    fn new(query: &'q ReplaceQuery) -> Result<Self> {
        if query.pattern.is_empty() {
            bail!("搜索内容为空");
        }
        let options = SearchOptions {
            case_sensitive: query.case_sensitive,
            regex: query.regex,
            mask: None,
        };
        let regex = search::build_regex(&query.pattern, &options)
            .map_err(|err| anyhow!("构建匹配模式失败: {err}"))?;
        let multiline = query.pattern.contains('\n');
        Ok(Self {
            query,
            regex,
            multiline,
        })
    }

    fn segments<'t>(&self, text: &'t str) -> Segments<'t> {
        if self.multiline {
            Segments::Whole(std::iter::once(text))
        } else {
            Segments::Lines(text.split('\n'))
        }
    }

    fn count(&self, text: &str) -> usize {
        self.segments(text)
            .map(|segment| self.regex.find_iter(segment).count())
            .sum()
    }

    fn has_match_at(&self, text: &str, (line_number, column): (usize, usize)) -> bool {
        if self.multiline {
            let Some(offset) = offset_at(text, line_number, column) else {
                return false;
            };
            return self.regex.find_iter(text).any(|m| m.start() == offset);
        }
        let Some(line) = line_number
            .checked_sub(1)
            .and_then(|i| text.split('\n').nth(i))
        else {
            return false;
        };
        let Some(offset) = utf16_offset(line, column) else {
            return false;
        };
        self.regex.find_iter(line).any(|m| m.start() == offset)
    }

    fn replace(&self, text: &str, chosen: Option<(usize, usize)>) -> Result<(String, usize)> {
        let mut out = String::with_capacity(text.len().min(MAX_REPLACE_OUTPUT_BYTES));
        let mut changed = 0usize;
        if self.multiline {
            let target = chosen.and_then(|(line, column)| offset_at(text, line, column));
            if chosen.is_some() && target.is_none() {
                return Ok((text.to_string(), 0));
            }
            let mut last = 0usize;
            for caps in self.regex.captures_iter(text) {
                let m = caps.get(0).expect("整体匹配一定存在");
                if target.is_some_and(|offset| offset != m.start()) {
                    continue;
                }
                push_bounded(&mut out, &text[last..m.start()], MAX_REPLACE_OUTPUT_BYTES)?;
                self.push_replacement(&mut out, &caps)?;
                last = m.end();
                changed += 1;
                if chosen.is_some() {
                    break;
                }
            }
            push_bounded(&mut out, &text[last..], MAX_REPLACE_OUTPUT_BYTES)?;
        } else {
            for (index, line) in text.split('\n').enumerate() {
                if index > 0 {
                    push_bounded(&mut out, "\n", MAX_REPLACE_OUTPUT_BYTES)?;
                }
                let target = if let Some((row, column)) = chosen {
                    let offset = (row == index + 1)
                        .then(|| utf16_offset(line, column))
                        .flatten();
                    let Some(offset) = offset else {
                        push_bounded(&mut out, line, MAX_REPLACE_OUTPUT_BYTES)?;
                        continue;
                    };
                    Some(offset)
                } else {
                    None
                };
                let mut last = 0usize;
                for caps in self.regex.captures_iter(line) {
                    let m = caps.get(0).expect("整体匹配一定存在");
                    if target.is_some_and(|offset| offset != m.start()) {
                        continue;
                    }
                    push_bounded(&mut out, &line[last..m.start()], MAX_REPLACE_OUTPUT_BYTES)?;
                    self.push_replacement(&mut out, &caps)?;
                    last = m.end();
                    changed += 1;
                    if chosen.is_some() {
                        break;
                    }
                }
                push_bounded(&mut out, &line[last..], MAX_REPLACE_OUTPUT_BYTES)?;
            }
        }
        Ok((out, changed))
    }

    fn push_replacement(&self, out: &mut String, caps: &regex::Captures<'_>) -> Result<()> {
        if self.query.regex {
            push_expanded(out, &self.query.replacement, caps, MAX_REPLACE_OUTPUT_BYTES)
        } else {
            push_bounded(out, &self.query.replacement, MAX_REPLACE_OUTPUT_BYTES)
        }
    }
}

enum Segments<'t> {
    Whole(std::iter::Once<&'t str>),
    Lines(std::str::Split<'t, char>),
}

impl<'t> Iterator for Segments<'t> {
    type Item = &'t str;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Segments::Whole(iter) => iter.next(),
            Segments::Lines(iter) => iter.next(),
        }
    }
}

fn utf16_offset(line: &str, col: usize) -> Option<usize> {
    let mut utf16 = 0;
    for (offset, ch) in line.char_indices() {
        if utf16 == col {
            return Some(offset);
        }
        utf16 += ch.len_utf16();
    }
    (utf16 == col).then_some(line.len())
}

fn offset_at(text: &str, line_number: usize, col: usize) -> Option<usize> {
    let mut lines = text.split('\n');
    let mut start = 0;
    for _ in 1..line_number {
        start += lines.next()?.len() + 1;
    }
    Some(start + utf16_offset(lines.next()?, col)?)
}

fn push_bounded(out: &mut String, text: &str, limit: usize) -> Result<()> {
    if out.len() + text.len() > limit {
        bail!("替换结果超过输出上限（{limit} 字节），已中止");
    }
    out.push_str(text);
    Ok(())
}

fn push_expanded(
    out: &mut String,
    replacement: &str,
    caps: &regex::Captures<'_>,
    limit: usize,
) -> Result<()> {
    let mut rest = replacement;
    while let Some(dollar) = rest.find('$') {
        push_bounded(out, &rest[..dollar], limit)?;
        let after = &rest[dollar + 1..];
        if after.is_empty() {
            push_bounded(out, "$", limit)?;
            return Ok(());
        }
        if let Some(tail) = after.strip_prefix('$') {
            push_bounded(out, "$", limit)?;
            rest = tail;
            continue;
        }
        let (name, consumed) = if let Some(tail) = after.strip_prefix('{') {
            match tail.find('}') {
                Some(end) => (&tail[..end], end + 2),
                None => {
                    push_bounded(out, "$", limit)?;
                    rest = after;
                    continue;
                }
            }
        } else {
            let mut end = 0usize;
            for ch in after.chars() {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    end += ch.len_utf8();
                } else {
                    break;
                }
            }
            if end == 0 {
                push_bounded(out, "$", limit)?;
                rest = after;
                continue;
            }
            (&after[..end], end)
        };
        if name.is_empty() {
            push_bounded(out, "$", limit)?;
            rest = after;
            continue;
        }
        let matched = match name.parse::<usize>() {
            Ok(index) => caps.get(index),
            Err(_) => caps.name(name),
        };
        if let Some(m) = matched {
            push_bounded(out, m.as_str(), limit)?;
        }
        rest = &rest[dollar + 1 + consumed..];
    }
    push_bounded(out, rest, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = format!(
                "omp-replace-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let root = std::env::temp_dir().join(unique);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn write(&self, name: &str, bytes: &[u8]) -> WorkspaceFile {
            let path = self.root.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, bytes).unwrap();
            WorkspaceFile {
                repo_id: "repo".into(),
                repo_name: "repo".into(),
                repo_path: self.root.to_string_lossy().into_owned(),
                rel_path: name.into(),
                abs_path: path.to_string_lossy().into_owned(),
                color_idx: 0,
            }
        }

        fn read(&self, name: &str) -> Vec<u8> {
            fs::read(self.root.join(name)).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn query(pattern: &str, replacement: &str, regex: bool) -> ReplaceQuery {
        ReplaceQuery {
            pattern: pattern.into(),
            replacement: replacement.into(),
            regex,
            case_sensitive: true,
            mask: None,
        }
    }

    fn hit_for(file: &WorkspaceFile, line: usize, column_utf16: usize) -> SearchHit {
        search::search_files(
            std::slice::from_ref(file),
            None,
            "foo",
            &SearchOptions {
                case_sensitive: true,
                ..Default::default()
            },
            &search::CancelToken::new(),
        )
        .unwrap()
        .hits
        .into_iter()
        .find(|hit| hit.line == line && hit.column_utf16 == column_utf16)
        .expect("fixture must contain the requested match")
    }

    #[test]
    fn rejects_same_length_edits_both_before_and_after_confirmation() {
        let fixture = Fixture::new();
        let file = fixture.write("concurrent.txt", b"foo unchanged\nfoo\n");
        let files = std::slice::from_ref(&file);
        let original = hit_for(&file, 2, 0);
        let dirty = HashSet::new();
        let replacement = query("foo", "bar", false);
        let plan = prepare_one(files, None, replacement.clone(), &original, &dirty).unwrap();
        let changed = b"foo competing\nfoo\n";
        assert_eq!(changed.len(), fixture.read("concurrent.txt").len());
        fs::write(fixture.root.join("concurrent.txt"), changed).unwrap();
        assert!(prepare_one(files, None, replacement, &original, &dirty).is_err());
        let reports = execute(&plan, &dirty, None);
        assert_eq!(reports[0].changed, 0);
        assert!(reports[0].error.is_some());
        assert_eq!(fixture.read("concurrent.txt"), changed);
    }

    #[test]
    fn replaces_only_second_match_in_crlf_file() {
        let fixture = Fixture::new();
        let file = fixture.write("crlf.txt", b"foo\r\nfoo\r\n");
        let read = read_worktree(&file).unwrap();
        assert_eq!(read.line_ending, LineEnding::CrLf);
        let hit = hit_for(&file, 2, 0);
        let files = vec![file];
        let dirty = HashSet::new();
        let plan = prepare_one(&files, None, query("foo", "bar", false), &hit, &dirty).unwrap();
        assert_eq!(plan.file_count(), 1);
        assert_eq!(plan.match_count(), 1);
        let results = execute(&plan, &dirty, None);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.is_none(), "{:?}", results[0].error);
        assert_eq!(results[0].changed, 1);
        assert_eq!(fixture.read("crlf.txt"), &b"foo\r\nbar\r\n"[..]);
    }

    #[test]
    fn prepares_and_replaces_every_match_beyond_display_limit() {
        let fixture = Fixture::new();
        let mut content = String::new();
        for _ in 0..350 {
            content.push_str("x foo\n");
        }
        let file = fixture.write("many.txt", content.as_bytes());
        let files = vec![file];
        let dirty = HashSet::new();
        let plan = prepare_all(&files, None, query("foo", "bar", false), &dirty).unwrap();
        assert_eq!(plan.file_count(), 1);
        assert_eq!(plan.match_count(), 350);
        let results = execute(&plan, &dirty, None);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.is_none(), "{:?}", results[0].error);
        assert_eq!(results[0].changed, 350);
        let text = String::from_utf8(fixture.read("many.txt")).unwrap();
        assert_eq!(text.matches("bar").count(), 350);
        assert!(!text.contains("foo"));
    }

    #[test]
    fn regex_replacement_expands_groups_and_dollar_escapes() {
        let fixture = Fixture::new();
        let file = fixture.write("caps.txt", b"a=1\nb=22\n");
        let files = std::slice::from_ref(&file);
        let dirty = HashSet::new();
        let plan = prepare_all(
            files,
            None,
            query(r"([a-z])=(\d+)", "$$${2}-$1", true),
            &dirty,
        )
        .unwrap();
        assert_eq!(plan.match_count(), 2);
        let results = execute(&plan, &dirty, None);
        assert!(results[0].error.is_none(), "{:?}", results[0].error);
        assert_eq!(results[0].changed, 2);
        assert_eq!(fixture.read("caps.txt"), &b"$1-a\n$22-b\n"[..]);
    }

    #[test]
    fn dirty_files_are_never_planned_or_written() {
        let fixture = Fixture::new();
        let file = fixture.write("dirty.txt", b"foo\n");
        let files = vec![file.clone()];
        let mut dirty = HashSet::new();
        dirty.insert(file.key());
        let plan = prepare_all(&files, None, query("foo", "bar", false), &dirty).unwrap();
        assert_eq!(plan.file_count(), 0);
        let hit = hit_for(&file, 1, 0);
        assert!(prepare_one(&files, None, query("foo", "bar", false), &hit, &dirty).is_err());
        assert_eq!(fixture.read("dirty.txt"), &b"foo\n"[..]);
    }

    #[test]
    fn execute_rechecks_dirty_set_before_writing() {
        let fixture = Fixture::new();
        let file = fixture.write("recheck.txt", b"foo\n");
        let files = std::slice::from_ref(&file);
        let clean = HashSet::new();
        let plan = prepare_all(files, None, query("foo", "bar", false), &clean).unwrap();
        let mut dirty = HashSet::new();
        dirty.insert(file.key());
        let results = execute(&plan, &dirty, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].changed, 0);
        assert!(results[0].error.is_some());
        assert_eq!(fixture.read("recheck.txt"), &b"foo\n"[..]);
    }

    #[test]
    fn empty_query_never_writes() {
        let fixture = Fixture::new();
        let file = fixture.write("empty.txt", b"foo\n");
        let files = std::slice::from_ref(&file);
        let dirty = HashSet::new();
        assert!(prepare_all(files, None, query("", "bar", false), &dirty).is_err());
        let read = read_worktree(&file).unwrap();
        let plan = Plan {
            query: query("", "bar", false),
            previews: vec![Preview {
                file: file.clone(),
                expected: read.version,
                line_ending: read.line_ending,
                matches: 1,
                chosen: None,
            }],
        };
        let results = execute(&plan, &dirty, None);
        assert_eq!(results[0].changed, 0);
        assert!(results[0].error.is_some());
        assert_eq!(fixture.read("empty.txt"), &b"foo\n"[..]);
    }
}
