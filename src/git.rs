use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::local_history::{LocalHistoryStore, sha256_hex};

/// 与旧版 `internal/gitops/review.go` 一致：单文件审阅与轻编辑上限 2 MiB。
pub const MAX_REVIEW_BYTES: usize = 2 * 1024 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn detect(bytes: &[u8]) -> Self {
        if bytes.windows(2).any(|w| w == b"\r\n") {
            Self::CrLf
        } else {
            Self::Lf
        }
    }

    pub fn normalize_to_lf(text: &str) -> String {
        if text.contains('\r') {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text.to_string()
        }
    }

    pub fn restore_bytes(self, lf_text: &str) -> Vec<u8> {
        match self {
            Self::Lf => lf_text.as_bytes().to_vec(),
            Self::CrLf => lf_text.replace('\n', "\r\n").into_bytes(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewVersion {
    pub exists: bool,
    pub sha256: String,
    pub size: u64,
    pub mtime_nanos: i128,
    pub ino: u64,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRead {
    pub text: String,
    pub line_ending: LineEnding,
    pub version: ReviewVersion,
    pub editable: bool,
    pub readonly_reason: Option<String>,
    pub binary: bool,
    pub too_large: bool,
}

#[derive(Debug, Clone)]
pub enum SaveReviewOutcome {
    Saved {
        version: ReviewVersion,
    },
    Conflict {
        disk_text: String,
        disk_version: ReviewVersion,
        disk_deleted: bool,
    },
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub path: String,
    pub rel_path: String,
    pub desc: String,
    pub monogram: String,
    pub color_idx: usize,
    pub is_root: bool,
}

impl RepoInfo {
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_path: String,
    pub path: String,
    pub orig_path: String,
    pub kind: String,
    pub index_status: char,
    pub worktree_status: char,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflict: bool,
    pub added: usize,
    pub deleted: usize,
}

impl ChangedFile {
    pub fn key(&self) -> String {
        format!("{}::{}", self.repo_id, self.path)
    }

    pub fn abs_path(&self) -> String {
        Path::new(&self.repo_path)
            .join(&self.path)
            .to_string_lossy()
            .to_string()
    }

    pub fn old_path_or_current(&self) -> &str {
        if !self.orig_path.is_empty() {
            &self.orig_path
        } else {
            &self.path
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoStatus {
    pub repo: RepoInfo,
    pub(crate) head: RepositoryHead,
    pub branch: String,
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
    pub detached: bool,
    pub files: Vec<ChangedFile>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum RepositoryHead {
    #[default]
    Unavailable,
    Unborn,
    Commit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFile {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_path: String,
    pub rel_path: String,
    pub abs_path: String,
    pub color_idx: usize,
}

impl WorkspaceFile {
    pub fn key(&self) -> String {
        format!("{}::{}", self.repo_id, self.rel_path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewChangeData {
    pub(crate) index_snapshot: std::result::Result<Option<crate::git_index::IndexEntry>, String>,
    pub(crate) head_snapshot: Option<String>,
    pub repo_id: String,
    pub repo_name: String,
    pub repo_path: String,
    pub old_path: String,
    pub new_path: String,
    pub old_title: String,
    pub new_title: String,
    pub old_text: String,
    pub new_text: String,
    pub staged_only: bool,
    pub editable: bool,
    pub readonly_reason: Option<String>,
    pub line_ending: LineEnding,
    pub version: ReviewVersion,
    pub(crate) old_text_block: Option<ReviewTextBlock>,
    pub(crate) new_text_block: Option<ReviewTextBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewTextBlock {
    Binary,
    TooLarge,
}

impl ReviewTextBlock {
    pub(crate) fn from_flags(binary: bool, too_large: bool) -> Option<Self> {
        if too_large {
            Some(Self::TooLarge)
        } else {
            binary.then_some(Self::Binary)
        }
    }

    pub(crate) fn comparison_notice(self) -> &'static str {
        match self {
            Self::Binary => "二进制或非 UTF-8 文件，无法以文本对比",
            Self::TooLarge => "文件超过 2 MiB 审阅上限",
        }
    }
}

impl ReviewChangeData {
    pub fn can_compare_text(&self) -> bool {
        self.old_text_block.is_none() && self.new_text_block.is_none()
    }

    pub(crate) fn text_block(&self) -> Option<ReviewTextBlock> {
        if [self.old_text_block, self.new_text_block].contains(&Some(ReviewTextBlock::TooLarge)) {
            Some(ReviewTextBlock::TooLarge)
        } else {
            self.old_text_block.or(self.new_text_block)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEntry {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_path: String,
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub refs: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFileEntry {
    pub status: String,
    pub path: String,
    pub orig_path: String,
}

impl CommitFileEntry {
    pub fn old_path_or_current(&self) -> &str {
        if !self.orig_path.is_empty() {
            &self.orig_path
        } else {
            &self.path
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDiffData {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_path: String,
    pub commit_hash: String,
    pub short_hash: String,
    pub parent_hash: String,
    pub status: String,
    pub old_path: String,
    pub new_path: String,
    pub old_title: String,
    pub new_title: String,
    pub old_text: String,
    pub new_text: String,
    pub binary: bool,
    pub too_large: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    pub repo_id: String,
    pub repo_name: String,
    pub repo_path: String,
    pub selector: String,
    pub message: String,
}

/// 运行系统 git 命令，强制 `-c core.quotepath=false`、`LC_ALL=C` 与 `GIT_OPTIONAL_LOCKS=0`。
/// 使用独立后台线程并发排空 stdout/stderr，彻底避免输出 >64 KiB 时的管道死锁，并保留超时保护。
pub fn run_git_bytes(repo_path: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = crate::tool_command::command("git");
    cmd.arg("-c")
        .arg("core.quotepath=false")
        .args(args)
        .current_dir(repo_path)
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("启动 git 失败 (dir={})", repo_path.display()))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let out_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stdout_pipe {
            pipe.read_to_end(&mut buf)?;
        }
        Ok(buf)
    });
    let err_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stderr_pipe {
            pipe.read_to_end(&mut buf)?;
        }
        Ok(buf)
    });

    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > GIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_handle.join();
            let _ = err_handle.join();
            return Err(anyhow!("git {:?} 超时", args));
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    let stdout = out_handle
        .join()
        .map_err(|_| anyhow!("读取 git stdout 线程异常"))??;
    let stderr = err_handle
        .join()
        .map_err(|_| anyhow!("读取 git stderr 线程异常"))??;

    if !status.success() {
        let stderr_msg = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(anyhow!(
            "git {} 失败 (exit {:?}): {}",
            args.join(" "),
            status.code(),
            stderr_msg
        ));
    }
    Ok(stdout)
}

enum LimitedGitOutput {
    Ok(Vec<u8>),
    TooLarge,
    Missing,
}

pub(crate) enum BoundedGitOutput {
    Ok(Vec<u8>),
    TooLarge,
    Failed { stderr: String, exit: Option<i32> },
}

/// 流式读取 Git 命令输出，最多保留 `max_bytes` 字节；超限时立即终止子进程避免大对象撑爆内存。
pub(crate) fn run_git_limited(
    repo_path: &Path,
    args: &[&str],
    max_bytes: usize,
    input: Option<Vec<u8>>,
) -> Result<BoundedGitOutput> {
    run_git_limited_with_index(repo_path, args, max_bytes, input, None)
}

pub(crate) fn run_git_limited_with_index(
    repo_path: &Path,
    args: &[&str],
    max_bytes: usize,
    input: Option<Vec<u8>>,
    index: Option<&Path>,
) -> Result<BoundedGitOutput> {
    run_git_limited_with_env(repo_path, args, max_bytes, input, index, &[])
}

pub(crate) fn run_git_limited_with_env(
    repo_path: &Path,
    args: &[&str],
    max_bytes: usize,
    input: Option<Vec<u8>>,
    index: Option<&Path>,
    env: &[(&str, &str)],
) -> Result<BoundedGitOutput> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let subcommand = args
        .iter()
        .copied()
        .find(|arg| !arg.starts_with('-'))
        .unwrap_or("git");

    let mut cmd = crate::tool_command::command("git");
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index).args(["-c", "core.splitIndex=false"]);
    }
    cmd.arg("-c")
        .arg("core.quotepath=false")
        .args(args)
        .current_dir(repo_path)
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    cmd.envs(env.iter().copied());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("启动 git {subcommand} 失败 (dir={})", repo_path.display()))?;

    let writer = input.zip(child.stdin.take()).map(|(bytes, mut pipe)| {
        std::thread::spawn(move || std::io::Write::write_all(&mut pipe, &bytes))
    });
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let hit_limit = Arc::new(AtomicBool::new(false));
    let hit_limit_bg = hit_limit.clone();

    let out_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stdout_pipe {
            let mut chunk = [0u8; 16 * 1024];
            loop {
                let n = pipe.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                if buf.len().saturating_add(n) > max_bytes {
                    hit_limit_bg.store(true, Ordering::SeqCst);
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        }
        Ok(buf)
    });
    let err_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stderr_pipe {
            pipe.by_ref().take(64 * 1024).read_to_end(&mut buf)?;
            std::io::copy(pipe, &mut std::io::sink())?;
        }
        Ok(buf)
    });

    let start = Instant::now();
    let status = loop {
        if hit_limit.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_handle.join();
            let _ = err_handle.join();
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            return Ok(BoundedGitOutput::TooLarge);
        }
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > GIT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_handle.join();
            let _ = err_handle.join();
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            return Err(anyhow!("git {} 超时", args.join(" ")));
        }
        std::thread::sleep(Duration::from_millis(4));
    };

    let stdout = out_handle
        .join()
        .map_err(|_| anyhow!("读取 git {subcommand} stdout 线程异常"))??;
    let stderr = err_handle
        .join()
        .map_err(|_| anyhow!("读取 git {subcommand} stderr 线程异常"))??;

    let written = writer.map(|writer| writer.join());
    if hit_limit.load(Ordering::SeqCst) {
        return Ok(BoundedGitOutput::TooLarge);
    }

    if !status.success() {
        let msg = String::from_utf8_lossy(&stderr).trim().to_string();
        return Ok(BoundedGitOutput::Failed {
            stderr: msg,
            exit: status.code(),
        });
    }

    if let Some(written) = written {
        written.map_err(|_| anyhow!("写入 git {subcommand} stdin 线程异常"))??;
    }
    Ok(BoundedGitOutput::Ok(stdout))
}

fn run_git_blob_limited(
    repo_path: &Path,
    spec: &str,
    max_bytes: usize,
) -> Result<LimitedGitOutput> {
    match run_git_limited(repo_path, &["cat-file", "blob", spec], max_bytes, None)? {
        BoundedGitOutput::Ok(bytes) => Ok(LimitedGitOutput::Ok(bytes)),
        BoundedGitOutput::TooLarge => Ok(LimitedGitOutput::TooLarge),
        BoundedGitOutput::Failed { stderr, exit } => {
            let lower = stderr.to_ascii_lowercase();
        if lower.contains("not in '")
            || lower.contains("does not exist")
            || lower.contains("invalid object name")
            || lower.contains("not a valid object name")
            || lower.contains("not at stage 0")
            || lower.contains("exists on disk, but not in")
        {
            return Ok(LimitedGitOutput::Missing);
        }
            Err(anyhow!("读取 Git 文件版本失败 ({spec}): {stderr} (exit {exit:?})"))
        }
    }
}

pub fn run_git_string(repo_path: &Path, args: &[&str]) -> Result<String> {
    let bytes = run_git_bytes(repo_path, args)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn is_git_dir(dir: &Path) -> Result<bool> {
    match dir.join(".git").try_exists() {
        Err(err) if err.kind() == std::io::ErrorKind::NotADirectory => Ok(false),
        result => result.with_context(|| format!("无法检查仓库目录 {}", dir.display())),
    }
}

fn make_monogram(name: &str) -> String {
    let chars: Vec<char> = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect();
    if chars.is_empty() {
        "R".to_string()
    } else {
        chars
            .into_iter()
            .collect::<String>()
            .to_uppercase()
    }
}

/// 极简解析 `mani.yaml` / `mani.yml` 中的 `projects` 段，避免引入重型 YAML 依赖。
fn parse_mani_projects(workspace: &Path) -> Vec<(String, String, String)> {
    let mani_path = if workspace.join("mani.yaml").exists() {
        workspace.join("mani.yaml")
    } else if workspace.join("mani.yml").exists() {
        workspace.join("mani.yml")
    } else {
        return Vec::new();
    };
    let Ok(content) = fs::read_to_string(&mani_path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_projects = false;
    let mut cur_name: Option<String> = None;
    let mut cur_path = String::new();
    let mut cur_desc = String::new();

    let flush = |name: &mut Option<String>,
                 path: &mut String,
                 desc: &mut String,
                 out: &mut Vec<(String, String, String)>| {
        if let Some(n) = name.take() {
            let p = if path.is_empty() { n.clone() } else { path.clone() };
            out.push((n, p, desc.clone()));
            path.clear();
            desc.clear();
        }
    };

    for raw_line in content.lines() {
        let trimmed_end = raw_line.trim_end();
        let trimmed = trimmed_end.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = trimmed_end.len() - trimmed_end.trim_start().len();
        if indent == 0 {
            flush(&mut cur_name, &mut cur_path, &mut cur_desc, &mut out);
            in_projects = trimmed == "projects:" || trimmed.starts_with("projects:");
            continue;
        }
        if !in_projects {
            continue;
        }
        if indent == 2 && trimmed.ends_with(':') {
            flush(&mut cur_name, &mut cur_path, &mut cur_desc, &mut out);
            let proj = trimmed.trim_end_matches(':').trim().trim_matches('"').trim_matches('\'');
            if !proj.is_empty() {
                cur_name = Some(proj.to_string());
            }
        } else if indent >= 4 && cur_name.is_some() {
            if let Some((k, v)) = trimmed.split_once(':') {
                let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                match k.trim() {
                    "path" => cur_path = val,
                    "desc" => cur_desc = val,
                    _ => {}
                }
            }
        }
    }
    flush(&mut cur_name, &mut cur_path, &mut cur_desc, &mut out);
    out
}

/// 发现工作区内的所有 Git 仓库（支持 mani.yaml、单根仓、多子仓混合模式）。
pub fn discover_workspace_repos(workspace_path: &str, hidden_repos: &[String]) -> Vec<RepoInfo> {
    let mut repos = discover_workspace_repository_inventory(workspace_path, &[], &mut BTreeMap::new())
        .unwrap_or_default();
    repos.retain(|repo| !hidden_repos.contains(&repo.id));
    repos
}

pub(crate) fn discover_workspace_repository_inventory(
    workspace_path: &str,
    previous: &[RepoInfo],
    bindings: &mut BTreeMap<String, String>,
) -> Result<Vec<RepoInfo>> {
    let trimmed = workspace_path.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let ws = Path::new(trimmed);
    let entries = fs::read_dir(ws).with_context(|| format!("无法读取工作区目录 {}", ws.display()))?;

    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut repos = Vec::new();

    // 1. mani.yaml 声明的仓库
    for (name, rel, desc) in parse_mani_projects(ws) {
        let full = ws.join(&rel);
        if !is_git_dir(&full)? {
            continue;
        }
        let canon = fs::canonicalize(&full).unwrap_or_else(|_| full.clone());
        if !seen_paths.insert(canon) {
            continue;
        }
        let is_root = rel == "." || full == ws;
        repos.push(RepoInfo {
            id: name.clone(),
            monogram: make_monogram(&name),
            name,
            display_name: None,
            path: full.to_string_lossy().to_string(),
            rel_path: rel,
            desc,
            color_idx: 0,
            is_root,
        });
    }

    // 2. 工作区根目录本身是否是 Git 仓
    if is_git_dir(ws)? {
        let canon = fs::canonicalize(ws).unwrap_or_else(|_| ws.to_path_buf());
        if seen_paths.insert(canon) {
            let name = ws
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("workspace")
                .to_string();
            repos.push(RepoInfo {
                id: name.clone(),
                monogram: make_monogram(&name),
                name,
                display_name: None,
                path: ws.to_string_lossy().to_string(),
                rel_path: ".".to_string(),
                desc: String::new(),
                color_idx: 0,
                is_root: true,
            });
        }
    }

    // 3. 直接子目录中的 Git 仓
    let mut subdirs = Vec::new();
    for entry in entries {
        let path = entry.with_context(|| format!("无法读取工作区目录 {}", ws.display()))?.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.starts_with('.') && !matches!(name, "node_modules" | "target" | "build") && path.is_dir() {
            subdirs.push(path);
        }
    }
    subdirs.sort();
    for sub in subdirs {
        if !is_git_dir(&sub)? {
            continue;
        }
        let canon = fs::canonicalize(&sub).unwrap_or_else(|_| sub.clone());
        if !seen_paths.insert(canon) {
            continue;
        }
        let name = sub
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("repo")
            .to_string();
        repos.push(RepoInfo {
            id: name.clone(),
            monogram: make_monogram(&name),
            name: name.clone(),
            display_name: None,
            path: sub.to_string_lossy().to_string(),
            rel_path: name,
            desc: String::new(),
            color_idx: 0,
            is_root: false,
        });
    }

    assign_repository_ids(&mut repos, previous, bindings);
    for (idx, repo) in repos.iter_mut().enumerate() {
        repo.color_idx = idx % 6;
    }
    Ok(repos)
}

fn assign_repository_ids(
    repos: &mut [RepoInfo],
    previous: &[RepoInfo],
    bindings: &mut BTreeMap<String, String>,
) {
    let mut names = HashSet::new();
    let mut duplicates = HashSet::new();
    for repo in repos.iter() {
        if !names.insert(repo.name.clone()) {
            duplicates.insert(repo.name.clone());
        }
    }
    if duplicates.is_empty() && bindings.is_empty() {
        return;
    }
    let paths: Vec<_> = repos.iter().map(repository_path_key).collect();

    // Reserve remembered IDs even while a repository is absent. A newly
    // discovered repository must not inherit the absent repository's state.
    let mut claimed = HashSet::new();
    bindings.retain(|_, id| !id.is_empty() && claimed.insert(id.clone()));
    if !duplicates.is_empty() {
        let existing: BTreeMap<_, _> = previous
            .iter()
            .map(|repo| (repository_path_key(repo), &repo.id))
            .collect();
        for (repo, path) in repos.iter().zip(&paths) {
            if duplicates.contains(&repo.name)
                && !bindings.contains_key(path)
                && let Some(&id) = existing.get(path)
                && claimed.insert(id.clone())
            {
                bindings.insert(path.clone(), id.clone());
            }
        }
    }

    let mut labels = HashSet::new();
    for (repo, path) in repos.iter_mut().zip(paths) {
        if let Some(id) = bindings.get(&path) {
            repo.id.clone_from(id);
        } else if !claimed.insert(repo.id.clone()) {
            let hash = sha256_hex(path.as_bytes());
            repo.id = unique_repository_value(format!("repo-{}", &hash[..16]), &names, &mut claimed);
        }
        if duplicates.contains(&repo.name) || repo.id != repo.name {
            bindings.entry(path).or_insert_with(|| repo.id.clone());
            let base = format!("{} · {}", repo.name, repo.rel_path);
            repo.display_name = Some(unique_repository_value(base, &names, &mut labels));
        }
    }
}

fn repository_path_key(repo: &RepoInfo) -> String {
    fs::canonicalize(&repo.path)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| repo.path.clone())
}

fn unique_repository_value(
    base: String,
    reserved: &HashSet<String>,
    claimed: &mut HashSet<String>,
) -> String {
    for index in 1.. {
        let candidate = if index == 1 {
            base.clone()
        } else {
            format!("{base} ({index})")
        };
        if !reserved.contains(&candidate) && claimed.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("finite repository inventory cannot claim every suffix")
}

#[cfg(test)]
mod repository_identity_tests {
    use super::{RepoInfo, assign_repository_ids, sha256_hex};
    use std::collections::BTreeMap;

    fn repository(name: &str, path: &str) -> RepoInfo {
        RepoInfo {
            id: name.to_owned(),
            name: name.to_owned(),
            display_name: None,
            path: path.to_owned(),
            rel_path: path.to_owned(),
            desc: String::new(),
            monogram: String::new(),
            color_idx: 0,
            is_root: false,
        }
    }

    #[test]
    fn newly_colliding_alias_keeps_the_open_repository_id_with_its_path() {
        let original = repository("shared", "fixture-repos/shared");
        let introduced = repository("shared", "fixture-repos/projects/nested");
        let hash = sha256_hex(introduced.path.as_bytes());
        let reserved_name = format!("repo-{}", &hash[..16]);
        let reserved = repository(&reserved_name, "fixture-repos/reserved");
        let mut bindings = BTreeMap::new();
        let mut repos = [introduced.clone(), original.clone(), reserved];
        assign_repository_ids(&mut repos, &[original], &mut bindings);
        assert_eq!(repos[1].id, "shared");
        assert_eq!(repos[2].id, reserved_name);
        assert_ne!(repos[0].id, "shared");
        assert_ne!(repos[0].id, reserved_name);
        assert_ne!(repos[0].label(), repos[1].label());

        let mut after_restart = [introduced];
        assign_repository_ids(&mut after_restart, &[], &mut bindings);
        assert_eq!(after_restart[0].id, repos[0].id);
    }
}

fn parse_numstat_z(bytes: &[u8]) -> BTreeMap<String, (usize, usize)> {
    let mut map = BTreeMap::new();
    let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        if part.is_empty() {
            i += 1;
            continue;
        }
        let s = String::from_utf8_lossy(part);
        let mut fields = s.split('\t');
        let Some(a_str) = fields.next() else {
            i += 1;
            continue;
        };
        let Some(d_str) = fields.next() else {
            i += 1;
            continue;
        };
        let Some(path_field) = fields.next() else {
            i += 1;
            continue;
        };
        let added = a_str.parse::<usize>().unwrap_or(0);
        let deleted = d_str.parse::<usize>().unwrap_or(0);
        let path = if path_field.is_empty() {
            // 重命名在 -z 模式下占额外两个 NUL 项：old\0new\0
            if i + 2 < parts.len() {
                let new_p = String::from_utf8_lossy(parts[i + 2]).to_string();
                i += 2;
                new_p
            } else {
                i += 1;
                continue;
            }
        } else {
            path_field.to_string()
        };
        let entry = map.entry(path).or_insert((0, 0));
        entry.0 += added;
        entry.1 += deleted;
        i += 1;
    }
    map
}

/// 检查单个仓库的分支与变更列表（保留系统 git 原生行为）。
pub fn inspect_repo(repo: &RepoInfo) -> RepoStatus {
    let dir = Path::new(&repo.path);
    let mut status = RepoStatus {
        repo: repo.clone(),
        head: RepositoryHead::Unavailable,
        branch: String::new(),
        upstream: String::new(),
        ahead: 0,
        behind: 0,
        detached: false,
        files: Vec::new(),
        error: None,
    };

    let raw = match run_git_bytes(dir, &["status", "--porcelain=v2", "-z", "-uall", "--branch"]) {
        Ok(b) => b,
        Err(err) => {
            status.error = Some(err.to_string());
            return status;
        }
    };

    let staged_stats = run_git_bytes(dir, &["diff", "--cached", "--numstat", "-z"])
        .map(|b| parse_numstat_z(&b))
        .unwrap_or_default();
    let unstaged_stats = run_git_bytes(dir, &["diff", "--numstat", "-z"])
        .map(|b| parse_numstat_z(&b))
        .unwrap_or_default();

    let mut records = raw.split(|&b| b == 0);
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix(b"# ") {
            parse_status_header(&String::from_utf8_lossy(header), &mut status);
            continue;
        }
        let fields = match record.first() {
            Some(b'1') => 9,
            Some(b'2') => 10,
            Some(b'u') => 11,
            Some(b'?') => 2,
            _ => continue,
        };
        let orig_path = if record.first() == Some(&b'2') {
            String::from_utf8_lossy(records.next().unwrap_or_default()).into_owned()
        } else {
            String::new()
        };
        let Some(path) = record.splitn(fields, |&byte| byte == b' ').nth(fields - 1) else {
            continue;
        };
        let untracked = record.first() == Some(&b'?');
        let (x, y) = if untracked {
            ('?', '?')
        } else {
            let Some([x, y]) = record.get(2..4) else { continue };
            (*x as char, *y as char)
        };
        let x = if x == '.' { ' ' } else { x };
        let y = if y == '.' { ' ' } else { y };
        let path = String::from_utf8_lossy(path).into_owned();

        let conflict = matches!(
            (x, y),
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')
        );
        let staged = !untracked && x != ' ' && x != '?';
        let unstaged = untracked || (y != ' ' && y != '?');

        let kind = if conflict {
            "U".to_string()
        } else if untracked {
            "?".to_string()
        } else if x == 'R' || y == 'R' {
            "R".to_string()
        } else if x == 'C' || y == 'C' {
            "C".to_string()
        } else if x == 'A' || y == 'A' {
            "A".to_string()
        } else if x == 'D' || y == 'D' {
            "D".to_string()
        } else if x == 'T' || y == 'T' {
            "T".to_string()
        } else {
            "M".to_string()
        };

        let (sa, sd) = staged_stats.get(&path).copied().unwrap_or((0, 0));
        let (ua, ud) = unstaged_stats.get(&path).copied().unwrap_or((0, 0));

        status.files.push(ChangedFile {
            repo_id: repo.id.clone(),
            repo_name: repo.label().to_owned(),
            repo_path: repo.path.clone(),
            path,
            orig_path,
            kind,
            index_status: x,
            worktree_status: y,
            staged,
            unstaged,
            untracked,
            conflict,
            added: sa + ua,
            deleted: sd + ud,
        });
    }

    if status.branch.is_empty() {
        status.branch = "HEAD".to_string();
    }
    status
}

fn parse_status_header(header: &str, status: &mut RepoStatus) {
    let Some((key, value)) = header.split_once(' ') else { return };
    match key {
        "branch.oid" if value == "(initial)" => status.head = RepositoryHead::Unborn,
        "branch.oid" if crate::git_index::oid_ok(value) => {
            status.head = RepositoryHead::Commit(value.to_owned());
        }
        "branch.head" if value == "(detached)" => status.detached = true,
        "branch.head" => status.branch = value.to_owned(),
        "branch.upstream" => status.upstream = value.to_owned(),
        "branch.ab" => {
            if let Some((ahead, behind)) = value.split_once(' ') {
                status.ahead = ahead.strip_prefix('+').and_then(|n| n.parse().ok()).unwrap_or(0);
                status.behind = behind.strip_prefix('-').and_then(|n| n.parse().ok()).unwrap_or(0);
            }
        }
        _ => {}
    }
}

#[cfg(unix)]
fn review_mode(meta: &fs::Metadata) -> u32 {
    meta.permissions().mode()
}

#[cfg(windows)]
fn review_mode(meta: &fs::Metadata) -> u32 {
    if meta.permissions().readonly() {
        0o444
    } else {
        0o666
    }
}

#[cfg(unix)]
fn review_inode(meta: &fs::Metadata) -> u64 {
    meta.ino()
}

#[cfg(windows)]
fn review_inode(meta: &fs::Metadata) -> u64 {
    // Portable metadata exposes the creation timestamp. Saving also checks
    // the actual file ID from the open handle at both checkpoints.
    meta.creation_time()
}

#[cfg(unix)]
fn review_file_identity(file: &fs::File) -> std::io::Result<(u64, u64)> {
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn review_file_identity(file: &fs::File) -> std::io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // The File owns the live handle and info is a valid writable out parameter.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok((u64::from(info.dwVolumeSerialNumber), index))
}

pub(crate) fn metadata_to_version(meta: &fs::Metadata, bytes: &[u8]) -> ReviewVersion {
    let mtime_nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0);
    ReviewVersion {
        exists: true,
        sha256: sha256_hex(bytes),
        size: meta.len(),
        mtime_nanos,
        ino: review_inode(meta),
        mode: review_mode(meta),
    }
}

/// 读取工作树文件用于审阅与轻编辑。差异仍需要将缺失的一侧表示为空文本。
pub fn read_review_worktree(abs_path: &Path) -> WorktreeRead {
    match read_worktree_source(abs_path) {
        Ok(Some(read)) => read,
        Ok(None) => unavailable_worktree("文件不存在或已删除".to_owned()),
        Err(err) => unavailable_worktree(format!("读取失败：{err}")),
    }
}

fn unavailable_worktree(reason: String) -> WorktreeRead {
    WorktreeRead {
        text: String::new(),
        line_ending: LineEnding::Lf,
        version: ReviewVersion::default(),
        editable: false,
        readonly_reason: Some(reason),
        binary: false,
        too_large: false,
    }
}

/// Review documents distinguish an empty file from a missing or unreadable path.
pub(crate) fn read_worktree_source(abs_path: &Path) -> Result<Option<WorktreeRead>> {
    let link_meta = match fs::symlink_metadata(abs_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let symlink = link_meta.is_symlink();
    anyhow::ensure!(symlink || link_meta.is_file(), "目标不是普通文件");

    let mut options = fs::OpenOptions::new();
    options.read(true);
    // A concurrently replaced path can be a FIFO even after lstat reports a file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(abs_path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let meta = file.metadata()?;
    anyhow::ensure!(meta.is_file(), "目标不是普通文件");
    let max_bytes = MAX_REVIEW_BYTES as u64;
    let mut bytes = Vec::new();
    if meta.len() <= max_bytes {
        file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    }
    let current_link = match fs::symlink_metadata(abs_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    anyhow::ensure!(current_link.is_symlink() == symlink, "文件在读取期间发生变化，请重试");
    let current = if symlink {
        match fs::metadata(abs_path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        }
    } else {
        current_link
    };
    anyhow::ensure!(current.is_file()
        && review_inode(&current) == review_inode(&meta)
        && review_mode(&current) == review_mode(&meta)
        && current.len() == meta.len()
        && current.modified().ok() == meta.modified().ok(), "文件在读取期间发生变化，请重试");

    let too_large = meta.len() > max_bytes || bytes.len() > MAX_REVIEW_BYTES;
    if too_large {
        return Ok(Some(WorktreeRead {
            text: String::new(),
            line_ending: LineEnding::Lf,
            version: ReviewVersion {
                exists: true,
                size: meta.len(),
                ino: review_inode(&meta),
                mode: review_mode(&meta),
                ..Default::default()
            },
            editable: false,
            readonly_reason: Some("文件超过 2 MiB 审阅上限".to_string()),
            binary: false,
            too_large: true,
        }));
    }
    let text = std::str::from_utf8(&bytes).ok().filter(|_| !bytes.contains(&0));
    let reason = if text.is_none() {
        Some("二进制或非 UTF-8 文件不可编辑")
    } else if symlink {
        Some("符号链接文件只读展示")
    } else if review_mode(&meta) & 0o222 == 0 {
        Some("磁盘文件为只读权限")
    } else {
        None
    };
    Ok(Some(WorktreeRead {
        text: text.map_or_else(String::new, LineEnding::normalize_to_lf),
        line_ending: LineEnding::detect(&bytes),
        version: metadata_to_version(&meta, &bytes),
        editable: reason.is_none(),
        readonly_reason: reason.map(str::to_owned),
        binary: text.is_none(),
        too_large: false,
    }))
}

/// 读取 Git 对象内容（如 `HEAD:path` 或 `:path`），使用 `git cat-file blob` 流式 2 MiB+1 限流与二进制校验。
/// 对象缺失返回空文本；真实 Git 错误或超时向上返回 `Err`，绝不静默伪装为空文件。
pub fn read_git_object_text(repo_path: &Path, spec: &str) -> Result<(String, bool, bool)> {
    Ok(read_optional_git_object_text(repo_path, spec)?.unwrap_or_default())
}

pub(crate) fn read_required_git_object_text(repo_path: &Path, spec: &str) -> Result<(String, bool, bool)> {
    read_optional_git_object_text(repo_path, spec)?
        .with_context(|| format!("无法读取 Git 文件对象 {spec}"))
}

pub(crate) fn read_tree_version_text(
    repo: &Path,
    commit: &str,
    path: &str,
) -> Result<(String, bool, bool)> {
    let entry = crate::git_index::tree_file(repo, commit, path)?;
    read_review_entry_text(repo, entry.as_ref())
}

fn read_review_entry_text(
    repo: &Path,
    entry: Option<&crate::git_index::IndexEntry>,
) -> Result<(String, bool, bool)> {
    entry.map_or_else(|| Ok(Default::default()), |entry| read_required_git_object_text(repo, &entry.oid))
}

pub(crate) fn read_optional_git_object_text(repo_path: &Path, spec: &str) -> Result<Option<(String, bool, bool)>> {
    let bytes = match run_git_blob_limited(repo_path, spec, MAX_REVIEW_BYTES)? {
        LimitedGitOutput::Ok(b) => b,
        LimitedGitOutput::TooLarge => return Ok(Some((String::new(), false, true))),
        LimitedGitOutput::Missing => return Ok(None),
    };
    if bytes.len() > MAX_REVIEW_BYTES {
        return Ok(Some((String::new(), false, true)));
    }
    match std::str::from_utf8(&bytes) {
        Ok(text) if !bytes.contains(&0) => Ok(Some((LineEnding::normalize_to_lf(text), false, false))),
        _ => Ok(Some((String::new(), true, false))),
    }
}

#[derive(Debug)]
pub(crate) struct WorktreeDiffReadError;

impl std::fmt::Display for WorktreeDiffReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("无法读取工作区文件")
    }
}

/// Compare pinned Git objects and the worktree. Empty sides represent missing
/// entries. A failed read of an existing entry aborts the comparison.
pub fn read_review_change(cf: &ChangedFile, staged_only: bool) -> Result<ReviewChangeData> {
    use crate::git_index::{ReviewIndexEntry, read_review_index_entry};

    let repo_dir = Path::new(&cf.repo_path);
    let scope_status = if staged_only { cf.index_status } else { cf.worktree_status };
    let old_path = if matches!(scope_status, 'R' | 'C') {
        cf.old_path_or_current().to_owned()
    } else {
        cf.path.clone()
    };
    let new_path = cf.path.clone();
    let index = read_review_index_entry(repo_dir, &new_path)?;
    let unmerged = matches!(index, ReviewIndexEntry::Unmerged);
    let compare_head = staged_only || cf.conflict || unmerged;
    let head_snapshot = if compare_head { crate::git_commit::head_oid(repo_dir)? } else { None };
    let head_text = |path: &str| {
        head_snapshot.as_deref().map_or_else(
            || Ok(Default::default()),
            |head| read_tree_version_text(repo_dir, head, path),
        )
    };

    if staged_only {
        anyhow::ensure!(!unmerged, "文件存在未合并的索引条目");
        let (old_text, old_bin, old_large) = head_text(&old_path)?;
        let (new_text, new_bin, new_large) = read_review_entry_text(repo_dir, index.file())?;
        return Ok(ReviewChangeData {
            index_snapshot: index.into_checked_entry().map_err(|err| err.to_string()),
            head_snapshot,
            repo_id: cf.repo_id.clone(),
            repo_name: cf.repo_name.clone(),
            repo_path: cf.repo_path.clone(),
            old_path,
            new_path,
            old_title: "HEAD".to_string(),
            new_title: "暂存区".to_string(),
            old_text,
            new_text,
            staged_only: true,
            editable: false,
            readonly_reason: Some("暂存区差异为只读视图".to_string()),
            line_ending: LineEnding::Lf,
            version: ReviewVersion::default(),
            old_text_block: ReviewTextBlock::from_flags(old_bin, old_large),
            new_text_block: ReviewTextBlock::from_flags(new_bin, new_large),
        });
    }

    let ((old_text, old_bin, old_large), old_title) = if compare_head {
        (head_text(&new_path)?, "HEAD")
    } else if cf.untracked && matches!(index, ReviewIndexEntry::Missing) {
        (Default::default(), "未跟踪")
    } else {
        let contents = if old_path == new_path {
            read_review_entry_text(repo_dir, index.file())?
        } else {
            let previous = read_review_index_entry(repo_dir, &old_path)?.into_checked_entry()?;
            read_review_entry_text(repo_dir, previous.as_ref())?
        };
        (contents, "暂存区")
    };

    let wt = read_worktree_source(Path::new(&cf.abs_path()))
        .context(WorktreeDiffReadError)?
        .unwrap_or_else(|| unavailable_worktree("文件不存在或已删除".to_owned()));
    Ok(ReviewChangeData {
        index_snapshot: index.into_checked_entry().map_err(|err| err.to_string()),
        head_snapshot,
        repo_id: cf.repo_id.clone(),
        repo_name: cf.repo_name.clone(),
        repo_path: cf.repo_path.clone(),
        old_path,
        new_path,
        old_title: old_title.to_owned(),
        new_title: "工作区".to_string(),
        old_text,
        new_text: wt.text,
        staged_only: false,
        editable: wt.editable && !old_bin && !old_large,
        readonly_reason: wt.readonly_reason,
        line_ending: wt.line_ending,
        version: wt.version,
        old_text_block: ReviewTextBlock::from_flags(old_bin, old_large),
        new_text_block: ReviewTextBlock::from_flags(wt.binary, wt.too_large),
    })
}

static TEMP_SAVE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 校验目标路径是否严格位于仓库根目录之内，防御相对路径穿越（`..`）与父目录符号链接逃逸。
pub(crate) fn validate_path_within_repo(repo_root: Option<&Path>, rel_path: &str, abs_path: &Path) -> Result<()> {
    for comp in Path::new(rel_path).components() {
        match comp {
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(anyhow!("非法相对路径（越出仓库边界）: {rel_path}"));
            }
            _ => {}
        }
    }
    let Some(root) = repo_root else {
        return Ok(());
    };
    let canon_root = fs::canonicalize(root)
        .with_context(|| format!("无法规范化仓库根目录: {}", root.display()))?;
    let Some(parent) = abs_path.parent() else {
        return Err(anyhow!("无法定位目标文件父目录"));
    };
    let canon_parent = fs::canonicalize(parent)
        .with_context(|| format!("无法规范化目标父目录: {}", parent.display()))?;
    if !canon_parent.starts_with(&canon_root) {
        return Err(anyhow!(
            "目标路径越出仓库边界（可能存在父目录符号链接逃逸）: {}",
            abs_path.display()
        ));
    }
    Ok(())
}

pub(crate) fn read_review_worktree_bytes(
    repo: &Path,
    rel: &str,
) -> Result<(Option<(Vec<u8>, ReviewVersion)>, &'static str)> {
    let path = repo.join(rel);
    validate_path_within_repo(Some(repo), rel, &path)?;
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((None, "100644")),
        Err(err) => return Err(err).context("读取工作区文件失败"),
    };
    anyhow::ensure!(meta.is_file() && !meta.is_symlink(), "仅支持普通文件");
    anyhow::ensure!(meta.len() <= MAX_REVIEW_BYTES as u64, "文件超过审阅上限");
    let mut bytes = Vec::new();
    fs::File::open(&path)?.take(MAX_REVIEW_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= MAX_REVIEW_BYTES && !bytes.contains(&0), "文件不是可审阅文本");
    std::str::from_utf8(&bytes).context("仅支持 UTF-8 文本")?;
    let version = metadata_to_version(&meta, &bytes);
    let mode = if version.mode & 0o111 != 0 { "100755" } else { "100644" };
    Ok((Some((bytes, version)), mode))
}

/// 受检原子保存（带仓库根目录边界校验，严格对齐旧版 `internal/gitops/review.go::SaveReviewText`）：
/// 1. 拒绝超过 2 MiB（`MAX_REVIEW_BYTES`）的写入；
/// 2. 校验相对路径与规范化父目录严格位于仓库根内；
/// 3. 绝不静默重建已删除文件；拒绝符号链接、非常规文件与只读文件；
/// 4. 核对磁盘 SHA-256 与预期版本一致，否则返回 Conflict（附带最新磁盘内容）；
/// 5. 保存前写入本地历史快照；
/// 6. 以 `O_EXCL`（`create_new(true)`）创建唯一临时文件写入、同步并恢复原权限；
/// 7. `fs::rename` 替换前二次校验目标文件仍存在、非符号链接、inode/mode 与 SHA-256 均未变化。
pub fn save_review_text_in_repo(
    repo_id: &str,
    repo_root: Option<&Path>,
    rel_path: &str,
    abs_path: &Path,
    expected: &ReviewVersion,
    line_ending: LineEnding,
    new_lf_text: &str,
    history: Option<&LocalHistoryStore>,
) -> SaveReviewOutcome {
    if let Err(err) = validate_path_within_repo(repo_root, rel_path, abs_path) {
        return SaveReviewOutcome::Rejected {
            reason: err.to_string(),
        };
    }

    let out_bytes = line_ending.restore_bytes(new_lf_text);
    if out_bytes.len() > MAX_REVIEW_BYTES {
        return SaveReviewOutcome::Rejected {
            reason: format!(
                "待保存内容超过审阅体积上限（{} 字节 > {} 字节），已拒绝保存",
                out_bytes.len(),
                MAX_REVIEW_BYTES
            ),
        };
    }

    type CheckedFile = (fs::Permissions, (u64, u64), Vec<u8>, ReviewVersion);
    let check_target = || -> Result<CheckedFile, SaveReviewOutcome> {
        let meta = match fs::symlink_metadata(abs_path) {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(SaveReviewOutcome::Conflict {
                    disk_text: String::new(),
                    disk_version: ReviewVersion::default(),
                    disk_deleted: true,
                });
            }
            Err(err) => return Err(SaveReviewOutcome::Rejected {
                reason: format!("无法核对磁盘文件：{err}"),
            }),
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(SaveReviewOutcome::Rejected {
                reason: "目标不是常规文件，已拒绝保存".to_string(),
            });
        }
        let mode = review_mode(&meta);
        if (mode & 0o222) == 0 {
            return Err(SaveReviewOutcome::Rejected {
                reason: "磁盘文件为只读，无法保存".to_string(),
            });
        }
        let (opened, identity, bytes) = (|| -> Result<_> {
            let file = fs::File::open(abs_path)?;
            let opened = file.metadata()?;
            anyhow::ensure!(
                opened.is_file()
                    && opened.len() == meta.len()
                    && opened.modified()? == meta.modified()?
                    && review_inode(&opened) == review_inode(&meta)
                    && review_mode(&opened) == mode,
                "打开期间工作区版本已变化"
            );
            let identity = review_file_identity(&file)?;
            let mut bytes = Vec::new();
            file.take(MAX_REVIEW_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() <= MAX_REVIEW_BYTES, "文件超过审阅上限");
            Ok((opened, identity, bytes))
        })()
        .map_err(|err| SaveReviewOutcome::Rejected {
            reason: format!("读取当前磁盘文件失败: {err}"),
        })?;
        let ver = metadata_to_version(&opened, &bytes);
        if expected.exists && ver.sha256 != expected.sha256 {
            let disk_text = if bytes.contains(&0) {
                String::new()
            } else {
                LineEnding::normalize_to_lf(&String::from_utf8_lossy(&bytes))
            };
            return Err(SaveReviewOutcome::Conflict {
                disk_text,
                disk_version: ver,
                disk_deleted: false,
            });
        }
        Ok((opened.permissions(), identity, bytes, ver))
    };

    let (permissions, identity, current_bytes, current_ver) = match check_target() {
        Ok(v) => v,
        Err(outcome) => return outcome,
    };

    // 保存前写入本地历史快照（失败不阻塞主保存，仅记日志）
    if let Some(store) = history {
        if let Err(err) = store.record(repo_id, rel_path, "保存前", &current_bytes) {
            log::warn!("记录本地历史失败: {err}");
        }
    }

    let Some(parent) = abs_path.parent() else {
        return SaveReviewOutcome::Rejected {
            reason: "无法定位目标文件父目录".to_string(),
        };
    };

    // 使用进程 PID + 纳秒时间戳 + 原子递增序号 + O_EXCL 创建唯一临时文件
    let seq = TEMP_SAVE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(
        ".nemorous-save-{}-{}-{}.tmp",
        std::process::id(),
        nanos,
        seq
    ));

    let prep_res = (|| -> Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        f.write_all(&out_bytes)?;
        f.sync_all()?;
        drop(f);
        fs::set_permissions(&tmp_path, permissions)?;
        Ok(())
    })();

    if let Err(err) = prep_res {
        let _ = fs::remove_file(&tmp_path);
        return SaveReviewOutcome::Rejected {
            reason: format!("写入临时文件失败: {err}"),
        };
    }

    // rename 替换前二次核对目标文件（防并发外部修改/删除/替换 inode 或权限变化）
    let (_, latest_identity, latest_bytes, latest_ver) = match check_target() {
        Ok(checked) => checked,
        Err(outcome) => {
            let _ = fs::remove_file(&tmp_path);
            return outcome;
        }
    };
    if latest_identity != identity
        || latest_ver.mode != current_ver.mode
        || latest_ver.sha256 != current_ver.sha256
    {
        let _ = fs::remove_file(&tmp_path);
        let disk_text = if latest_bytes.contains(&0) {
            String::new()
        } else {
            LineEnding::normalize_to_lf(&String::from_utf8_lossy(&latest_bytes))
        };
        return SaveReviewOutcome::Conflict {
            disk_text,
            disk_version: latest_ver,
            disk_deleted: false,
        };
    }

    match fs::rename(&tmp_path, abs_path) {
        Ok(()) => match fs::symlink_metadata(abs_path) {
            Ok(after_meta) => SaveReviewOutcome::Saved {
                version: metadata_to_version(&after_meta, &out_bytes),
            },
            Err(err) => SaveReviewOutcome::Rejected {
                reason: format!("读取保存后文件信息失败: {err}"),
            },
        },
        Err(err) => {
            let _ = fs::remove_file(&tmp_path);
            SaveReviewOutcome::Rejected {
                reason: format!("保存文件失败: {err}"),
            }
        }
    }
}

pub fn save_review_text(
    repo_id: &str,
    rel_path: &str,
    abs_path: &Path,
    expected: &ReviewVersion,
    line_ending: LineEnding,
    new_lf_text: &str,
    history: Option<&LocalHistoryStore>,
) -> SaveReviewOutcome {
    save_review_text_in_repo(
        repo_id,
        None,
        rel_path,
        abs_path,
        expected,
        line_ending,
        new_lf_text,
        history,
    )
}

/// 列出仓库提交历史。
pub fn repo_log(repo: &RepoInfo, limit: usize) -> Result<Vec<CommitEntry>> {
    let dir = Path::new(&repo.path);
    let limit_arg = format!("-n{}", limit.max(1));
    let out = run_git_string(
        dir,
        &[
            "log",
            &limit_arg,
            "--format=%H%x1f%h%x1f%P%x1f%an%x1f%aI%x1f%s%x1f%D%x1e",
        ],
    )?;
    let mut commits = Vec::new();
    for rec in out.split('\x1e') {
        let rec = rec.trim();
        if rec.is_empty() {
            continue;
        }
        let cols: Vec<&str> = rec.split('\x1f').collect();
        if cols.len() < 6 {
            continue;
        }
        let parents = cols[2]
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        commits.push(CommitEntry {
            repo_id: repo.id.clone(),
            repo_name: repo.label().to_owned(),
            repo_path: repo.path.clone(),
            hash: cols[0].to_string(),
            short_hash: cols[1].to_string(),
            parents,
            author: cols[3].to_string(),
            date: cols[4].to_string(),
            subject: cols[5].to_string(),
            refs: cols.get(6).copied().unwrap_or("").to_string(),
        });
    }
    Ok(commits)
}

/// 读取指定提交的变更文件列表。合并提交与第一父提交比较，与历史 Diff 一致。
pub fn commit_files(repo_path: &str, hash: &str) -> Result<Vec<CommitFileEntry>> {
    let dir = Path::new(repo_path);
    let bytes = run_git_bytes(
        dir,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            "--root",
            "-M",
            "--diff-merges=first-parent",
            "-z",
            hash,
        ],
    )?;
    let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let tok = parts[i];
        if tok.is_empty() {
            i += 1;
            continue;
        }
        let status_raw = String::from_utf8_lossy(tok).to_string();
        let code = status_raw.chars().next().unwrap_or('M');
        if (code == 'R' || code == 'C') && i + 2 < parts.len() {
            let orig_path = String::from_utf8_lossy(parts[i + 1]).to_string();
            let path = String::from_utf8_lossy(parts[i + 2]).to_string();
            out.push(CommitFileEntry {
                status: code.to_string(),
                path,
                orig_path,
            });
            i += 3;
        } else if i + 1 < parts.len() {
            let path = String::from_utf8_lossy(parts[i + 1]).to_string();
            out.push(CommitFileEntry {
                status: code.to_string(),
                path,
                orig_path: String::new(),
            });
            i += 2;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

/// 读取历史提交中单个文件的双侧 Diff（双侧只读，保留重命名旧/新路径）。
pub fn read_commit_diff(
    commit: &CommitEntry,
    file: &CommitFileEntry,
) -> Result<CommitDiffData> {
    let dir = Path::new(&commit.repo_path);
    let parent_hash = commit.parents.first().cloned().unwrap_or_default();
    let old_path = file.old_path_or_current().to_string();
    let new_path = file.path.clone();

    let (old_text, old_bin, old_large) = if parent_hash.is_empty() || file.status == "A" {
        (String::new(), false, false)
    } else {
        read_required_git_object_text(dir, &format!("{parent_hash}:{old_path}"))?
    };

    let (new_text, new_bin, new_large) = if file.status == "D" {
        (String::new(), false, false)
    } else {
        read_required_git_object_text(dir, &format!("{}:{new_path}", commit.hash))?
    };

    let parent_short = if parent_hash.is_empty() {
        "空".to_string()
    } else {
        parent_hash.chars().take(7).collect()
    };

    Ok(CommitDiffData {
        repo_id: commit.repo_id.clone(),
        repo_name: commit.repo_name.clone(),
        repo_path: commit.repo_path.clone(),
        commit_hash: commit.hash.clone(),
        short_hash: commit.short_hash.clone(),
        parent_hash,
        status: file.status.clone(),
        old_path,
        new_path,
        old_title: parent_short,
        new_title: commit.short_hash.clone(),
        old_text,
        new_text,
        binary: old_bin || new_bin,
        too_large: old_large || new_large,
    })
}

/// 列出指定仓库中某个文件截至某版本的提交历史（`--follow` 追踪重命名）。
pub fn file_history_at_revision(
    repo: &RepoInfo,
    rev: &str,
    rel_path: &str,
    limit: usize,
) -> Result<Vec<CommitEntry>> {
    let dir = Path::new(&repo.path);
    let limit_arg = format!("-n{}", limit.max(1));
    let target_rev = if rev.is_empty() { "HEAD" } else { rev };
    let out = run_git_string(
        dir,
        &[
            "--literal-pathspecs",
            "log",
            &limit_arg,
            "--follow",
            "--format=%H%x1f%h%x1f%P%x1f%an%x1f%aI%x1f%s%x1f%D%x1e",
            "--end-of-options",
            target_rev,
            "--",
            rel_path,
        ],
    )?;
    let mut commits = Vec::new();
    for rec in out.split('\x1e') {
        let rec = rec.trim();
        if rec.is_empty() {
            continue;
        }
        let cols: Vec<&str> = rec.split('\x1f').collect();
        if cols.len() < 6 {
            continue;
        }
        let parents = cols[2]
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        commits.push(CommitEntry {
            repo_id: repo.id.clone(),
            repo_name: repo.label().to_owned(),
            repo_path: repo.path.clone(),
            hash: cols[0].to_string(),
            short_hash: cols[1].to_string(),
            parents,
            author: cols[3].to_string(),
            date: cols[4].to_string(),
            subject: cols[5].to_string(),
            refs: cols.get(6).copied().unwrap_or("").to_string(),
        });
    }
    Ok(commits)
}

/// 列出所有仓库的工作区文件（用于 Project 树与转到文件）。
pub fn list_workspace_files(repos: &[RepoInfo]) -> Vec<WorkspaceFile> {
    let mut out: Vec<_> = repos.iter()
        .flat_map(|repo| read_repository_files(repo).unwrap_or_default())
        .collect();
    sort_workspace_files(&mut out);
    out
}

pub(crate) fn sort_workspace_files(files: &mut [WorkspaceFile]) {
    files.sort_by(|a, b| a.repo_name.cmp(&b.repo_name).then(a.rel_path.cmp(&b.rel_path)));
}

pub(crate) fn read_repository_files(repo: &RepoInfo) -> Result<Vec<WorkspaceFile>> {
    let dir = Path::new(&repo.path);
    let raw = run_git_bytes(dir, &["ls-files", "-co", "--exclude-standard", "-z"])?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for part in raw.split(|&b| b == 0).filter(|part| !part.is_empty()) {
        let rel = String::from_utf8_lossy(part).into_owned();
        if !seen.insert(rel.clone()) {
            continue;
        }
        let abs = dir.join(&rel);
        if !abs.exists() {
            continue;
        }
        out.push(WorkspaceFile {
            repo_id: repo.id.clone(),
            repo_name: repo.label().to_owned(),
            repo_path: repo.path.clone(),
            rel_path: rel,
            abs_path: abs.to_string_lossy().into_owned(),
            color_idx: repo.color_idx,
        });
    }
    Ok(out)
}

/// 暂存或取消暂存指定文件列表（支持混合暂存与重命名）。
pub fn stage_or_unstage_files(repo_path: &str, paths: &[String], stage: bool) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let dir = Path::new(repo_path);
    if stage {
        let mut args: Vec<&str> = vec!["--literal-pathspecs", "add", "--"];
        for p in paths {
            args.push(p.as_str());
        }
        run_git_bytes(dir, &args)?;
    } else {
        // 检查是否存在 HEAD（首提交无 HEAD 时用 rm --cached）
        let has_head = crate::git_commit::head_oid(dir)?.is_some();
        let mut args: Vec<&str> = if has_head {
            vec!["--literal-pathspecs", "reset", "HEAD", "--"]
        } else {
            vec!["--literal-pathspecs", "rm", "--cached", "-q", "--"]
        };
        for p in paths {
            args.push(p.as_str());
        }
        run_git_bytes(dir, &args)?;
    }
    Ok(())
}

/// 列出全部仓库的 Stash 条目。
pub fn list_stashes(repos: &[RepoInfo]) -> Vec<StashEntry> {
    repos.iter()
        .flat_map(|repo| read_repository_stashes(repo).unwrap_or_default())
        .collect()
}

pub(crate) fn read_repository_stashes(repo: &RepoInfo) -> Result<Vec<StashEntry>> {
    let args = ["stash", "list", "--format=%gd%x1f%gs"];
    let output = run_git_string(Path::new(&repo.path), &args)?;
    Ok(output.lines().filter_map(|line| {
        let (selector, message) = line.split_once('\x1f')?;
        Some(StashEntry {
            repo_id: repo.id.clone(),
            repo_name: repo.label().to_owned(),
            repo_path: repo.path.clone(),
            selector: selector.to_owned(),
            message: message.to_owned(),
        })
    }).collect())
}
