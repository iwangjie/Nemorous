use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    pub path: String,
    pub rel_path: String,
    pub desc: String,
    pub monogram: String,
    pub color_idx: usize,
    pub is_root: bool,
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
    pub branch: String,
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
    pub detached: bool,
    pub files: Vec<ChangedFile>,
    pub error: Option<String>,
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
    pub binary: bool,
    pub too_large: bool,
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
    let mut cmd = Command::new("git");
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

    let mut cmd = Command::new("git");
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

fn is_git_dir(dir: &Path) -> bool {
    dir.join(".git").exists()
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
    let trimmed = workspace_path.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let ws = Path::new(trimmed);
    if !ws.exists() {
        return Vec::new();
    }

    let hidden: HashSet<&str> = hidden_repos.iter().map(|s| s.as_str()).collect();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut repos = Vec::new();

    // 1. mani.yaml 声明的仓库
    for (name, rel, desc) in parse_mani_projects(ws) {
        let full = ws.join(&rel);
        if !is_git_dir(&full) {
            continue;
        }
        let canon = fs::canonicalize(&full).unwrap_or_else(|_| full.clone());
        if !seen_paths.insert(canon) {
            continue;
        }
        let id = name.clone();
        if hidden.contains(id.as_str()) {
            continue;
        }
        let is_root = rel == "." || full == ws;
        repos.push(RepoInfo {
            id,
            monogram: make_monogram(&name),
            name,
            path: full.to_string_lossy().to_string(),
            rel_path: rel,
            desc,
            color_idx: 0,
            is_root,
        });
    }

    // 2. 工作区根目录本身是否是 Git 仓
    if is_git_dir(ws) {
        let canon = fs::canonicalize(ws).unwrap_or_else(|_| ws.to_path_buf());
        if seen_paths.insert(canon) {
            let name = ws
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("workspace")
                .to_string();
            let id = name.clone();
            if !hidden.contains(id.as_str()) {
                repos.push(RepoInfo {
                    id,
                    monogram: make_monogram(&name),
                    name,
                    path: ws.to_string_lossy().to_string(),
                    rel_path: ".".to_string(),
                    desc: String::new(),
                    color_idx: 0,
                    is_root: true,
                });
            }
        }
    }

    // 3. 直接子目录中的 Git 仓
    if let Ok(entries) = fs::read_dir(ws) {
        let mut subdirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .filter(|p| {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                !name.starts_with('.') && name != "node_modules" && name != "target" && name != "build"
            })
            .collect();
        subdirs.sort();

        for sub in subdirs {
            if !is_git_dir(&sub) {
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
            let id = name.clone();
            if hidden.contains(id.as_str()) {
                continue;
            }
            repos.push(RepoInfo {
                id,
                monogram: make_monogram(&name),
                name: name.clone(),
                path: sub.to_string_lossy().to_string(),
                rel_path: name,
                desc: String::new(),
                color_idx: 0,
                is_root: false,
            });
        }
    }

    for (idx, repo) in repos.iter_mut().enumerate() {
        repo.color_idx = idx % 6;
    }
    repos
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
        branch: String::new(),
        upstream: String::new(),
        ahead: 0,
        behind: 0,
        detached: false,
        files: Vec::new(),
        error: None,
    };

    let raw = match run_git_bytes(dir, &["status", "--porcelain=v1", "-z", "-uall", "--branch"]) {
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

    let records: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    let mut i = 0;
    while i < records.len() {
        let rec = records[i];
        if rec.is_empty() {
            i += 1;
            continue;
        }
        let s = String::from_utf8_lossy(rec);
        if let Some(branch_header) = s.strip_prefix("## ") {
            parse_branch_header(branch_header, &mut status);
            i += 1;
            continue;
        }
        if rec.len() < 4 {
            i += 1;
            continue;
        }
        let x = rec[0] as char;
        let y = rec[1] as char;
        let path = String::from_utf8_lossy(&rec[3..]).to_string();
        let mut orig_path = String::new();
        if (x == 'R' || x == 'C' || y == 'R' || y == 'C') && i + 1 < records.len() {
            orig_path = String::from_utf8_lossy(records[i + 1]).to_string();
            i += 1;
        }

        let untracked = x == '?' && y == '?';
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
            repo_name: repo.name.clone(),
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
        i += 1;
    }

    if status.branch.is_empty() {
        status.branch = "HEAD".to_string();
    }
    status
}

fn parse_branch_header(header: &str, status: &mut RepoStatus) {
    let h = header.trim();
    if let Some(rest) = h.strip_prefix("No commits yet on ") {
        status.branch = rest.trim().to_string();
        return;
    }
    if let Some(rest) = h.strip_prefix("Initial commit on ") {
        status.branch = rest.trim().to_string();
        return;
    }
    if h.starts_with("HEAD (no branch)") {
        status.branch = "HEAD".to_string();
        status.detached = true;
        return;
    }

    let (main_part, bracket_part) = match h.split_once(" [") {
        Some((l, r)) => (l, Some(r.trim_end_matches(']'))),
        None => (h, None),
    };
    if let Some((branch, upstream)) = main_part.split_once("...") {
        status.branch = branch.trim().to_string();
        status.upstream = upstream.trim().to_string();
    } else {
        status.branch = main_part.trim().to_string();
    }

    if let Some(info) = bracket_part {
        for token in info.split(',') {
            let t = token.trim();
            if let Some(n) = t.strip_prefix("ahead ") {
                status.ahead = n.trim().parse().unwrap_or(0);
            } else if let Some(n) = t.strip_prefix("behind ") {
                status.behind = n.trim().parse().unwrap_or(0);
            }
        }
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

/// 读取工作树文件用于审阅与轻编辑（严格执行 2 MiB 限制、二进制/UTF-8 检验、只读检验与行尾保真）。
pub fn read_review_worktree(abs_path: &Path) -> WorktreeRead {
    let meta = match fs::symlink_metadata(abs_path) {
        Ok(m) => m,
        Err(_) => {
            return WorktreeRead {
                text: String::new(),
                line_ending: LineEnding::Lf,
                version: ReviewVersion::default(),
                editable: false,
                readonly_reason: Some("文件不存在或已删除".to_string()),
                binary: false,
                too_large: false,
            };
        }
    };

    if meta.file_type().is_symlink() {
        // 跟随符号链接做只读读取，避免误覆盖符号链接目标
        let target_meta = match fs::metadata(abs_path) {
            Ok(m) if m.is_file() => m,
            _ => {
                return WorktreeRead {
                    text: String::new(),
                    line_ending: LineEnding::Lf,
                    version: ReviewVersion::default(),
                    editable: false,
                    readonly_reason: Some("符号链接或非常规文件仅只读".to_string()),
                    binary: false,
                    too_large: false,
                };
            }
        };
        if target_meta.len() as usize > MAX_REVIEW_BYTES {
            return WorktreeRead {
                text: String::new(),
                line_ending: LineEnding::Lf,
                version: ReviewVersion::default(),
                editable: false,
                readonly_reason: Some("文件超过 2 MiB 审阅上限，已只读拦截".to_string()),
                binary: false,
                too_large: true,
            };
        }
        let bytes = fs::read(abs_path).unwrap_or_default();
        let version = metadata_to_version(&target_meta, &bytes);
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            return WorktreeRead {
                text: String::new(),
                line_ending: LineEnding::Lf,
                version,
                editable: false,
                readonly_reason: Some("二进制或非 UTF-8 文件不可编辑".to_string()),
                binary: true,
                too_large: false,
            };
        }
        let raw = String::from_utf8_lossy(&bytes);
        return WorktreeRead {
            text: LineEnding::normalize_to_lf(&raw),
            line_ending: LineEnding::detect(&bytes),
            version,
            editable: false,
            readonly_reason: Some("符号链接文件只读展示".to_string()),
            binary: false,
            too_large: false,
        };
    }

    if !meta.is_file() {
        return WorktreeRead {
            text: String::new(),
            line_ending: LineEnding::Lf,
            version: ReviewVersion::default(),
            editable: false,
            readonly_reason: Some("非常规文件不可编辑".to_string()),
            binary: false,
            too_large: false,
        };
    }

    if meta.len() as usize > MAX_REVIEW_BYTES {
        return WorktreeRead {
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
        };
    }

    let bytes = match fs::read(abs_path) {
        Ok(b) => b,
        Err(err) => {
            return WorktreeRead {
                text: String::new(),
                line_ending: LineEnding::Lf,
                version: ReviewVersion::default(),
                editable: false,
                readonly_reason: Some(format!("读取失败: {err}")),
                binary: false,
                too_large: false,
            };
        }
    };

    let version = metadata_to_version(&meta, &bytes);
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return WorktreeRead {
            text: String::new(),
            line_ending: LineEnding::Lf,
            version,
            editable: false,
            readonly_reason: Some("二进制或非 UTF-8 文件不可编辑".to_string()),
            binary: true,
            too_large: false,
        };
    }

    let mode = review_mode(&meta);
    let writable = (mode & 0o222) != 0;
    let line_ending = LineEnding::detect(&bytes);
    let text = LineEnding::normalize_to_lf(std::str::from_utf8(&bytes).unwrap_or(""));

    WorktreeRead {
        text,
        line_ending,
        version,
        editable: writable,
        readonly_reason: if writable {
            None
        } else {
            Some("磁盘文件为只读权限".to_string())
        },
        binary: false,
        too_large: false,
    }
}

/// 读取 Git 对象内容（如 `HEAD:path` 或 `:path`），使用 `git cat-file blob` 流式 2 MiB+1 限流与二进制校验。
/// 对象缺失返回空文本；真实 Git 错误或超时向上返回 `Err`，绝不静默伪装为空文件。
pub fn read_git_object_text(repo_path: &Path, spec: &str) -> Result<(String, bool, bool)> {
    let bytes = match run_git_blob_limited(repo_path, spec, MAX_REVIEW_BYTES)? {
        LimitedGitOutput::Ok(b) => b,
        LimitedGitOutput::TooLarge => return Ok((String::new(), false, true)),
        LimitedGitOutput::Missing => return Ok((String::new(), false, false)),
    };
    if bytes.len() > MAX_REVIEW_BYTES {
        return Ok((String::new(), false, true));
    }
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        return Ok((String::new(), true, false));
    }
    let text = LineEnding::normalize_to_lf(std::str::from_utf8(&bytes).unwrap_or(""));
    Ok((text, false, false))
}

/// 读取工作区变更 Diff（严格对齐旧版 `gitops.ReadReviewChange`）：
/// - 暂存区：`HEAD:<orig_path or path>` vs `:<path>`，只读
/// - 未暂存：`""`(未跟踪) / `HEAD:<path>`(冲突) / `:<path>`(其余) vs 工作树文件，可编辑
pub fn read_review_change(cf: &ChangedFile, staged_only: bool) -> ReviewChangeData {
    let repo_dir = Path::new(&cf.repo_path);
    let old_path = cf.old_path_or_current().to_string();
    let new_path = cf.path.clone();
    let index_snapshot = crate::git_index::read_index_entry(repo_dir, &new_path)
        .map_err(|err| err.to_string());
    let head_snapshot = staged_only.then(|| crate::git_commit::head_oid(repo_dir).ok().flatten()).flatten();

    if staged_only {
        let head_spec = format!("HEAD:{old_path}");
        let idx_spec = format!(":{new_path}");
        let mut git_err: Option<String> = None;
        let (old_text, old_bin, old_large) = match read_git_object_text(repo_dir, &head_spec) {
            Ok(v) => v,
            Err(e) => {
                git_err = Some(e.to_string());
                (String::new(), false, false)
            }
        };
        let (new_text, new_bin, new_large) = match read_git_object_text(repo_dir, &idx_spec) {
            Ok(v) => v,
            Err(e) => {
                if git_err.is_none() {
                    git_err = Some(e.to_string());
                }
                (String::new(), false, false)
            }
        };
        return ReviewChangeData {
            index_snapshot,
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
            readonly_reason: git_err.or_else(|| Some("暂存区差异为只读视图".to_string())),
            line_ending: LineEnding::Lf,
            version: ReviewVersion::default(),
            binary: old_bin || new_bin,
            too_large: old_large || new_large,
        };
    }

    let mut old_err: Option<String> = None;
    let (old_text, old_bin, old_large, old_title) = if cf.untracked {
        (String::new(), false, false, "未跟踪".to_string())
    } else if cf.conflict {
        let spec = format!("HEAD:{new_path}");
        let (t, b, l) = match read_git_object_text(repo_dir, &spec) {
            Ok(v) => v,
            Err(e) => {
                old_err = Some(e.to_string());
                (String::new(), false, false)
            }
        };
        (t, b, l, "HEAD".to_string())
    } else {
        let spec = format!(":{new_path}");
        let (t, b, l) = match read_git_object_text(repo_dir, &spec) {
            Ok(v) => v,
            Err(e) => {
                old_err = Some(e.to_string());
                (String::new(), false, false)
            }
        };
        (t, b, l, "暂存区".to_string())
    };

    let wt = read_review_worktree(&PathBuf::from(cf.abs_path()));
    ReviewChangeData {
        index_snapshot,
        head_snapshot,
        repo_id: cf.repo_id.clone(),
        repo_name: cf.repo_name.clone(),
        repo_path: cf.repo_path.clone(),
        old_path,
        new_path,
        old_title,
        new_title: "工作区".to_string(),
        old_text,
        new_text: wt.text,
        staged_only: false,
        editable: wt.editable && !old_bin && !old_large && old_err.is_none(),
        readonly_reason: old_err.or(wt.readonly_reason),
        line_ending: wt.line_ending,
        version: wt.version,
        binary: old_bin || wt.binary,
        too_large: old_large || wt.too_large,
    }
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
            Err(_) => {
                return Err(SaveReviewOutcome::Conflict {
                    disk_text: String::new(),
                    disk_version: ReviewVersion::default(),
                    disk_deleted: true,
                });
            }
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
            repo_name: repo.name.clone(),
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

/// 读取指定提交的变更文件列表（支持重命名、新增、删除、根提交）。
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
        read_git_object_text(dir, &format!("{parent_hash}:{old_path}"))?
    };

    let (new_text, new_bin, new_large) = if file.status == "D" {
        (String::new(), false, false)
    } else {
        read_git_object_text(dir, &format!("{}:{new_path}", commit.hash))?
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
            repo_name: repo.name.clone(),
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
    let mut out = Vec::new();
    for repo in repos {
        let dir = Path::new(&repo.path);
        let bytes = run_git_bytes(dir, &["ls-files", "-co", "--exclude-standard", "-z"]);
        let Ok(raw) = bytes else {
            continue;
        };
        let mut seen = HashSet::new();
        for part in raw.split(|&b| b == 0) {
            if part.is_empty() {
                continue;
            }
            let rel = String::from_utf8_lossy(part).to_string();
            if !seen.insert(rel.clone()) {
                continue;
            }
            let abs = dir.join(&rel);
            if !abs.exists() {
                continue;
            }
            out.push(WorkspaceFile {
                repo_id: repo.id.clone(),
                repo_name: repo.name.clone(),
                repo_path: repo.path.clone(),
                rel_path: rel,
                abs_path: abs.to_string_lossy().to_string(),
                color_idx: repo.color_idx,
            });
        }
    }
    out.sort_by(|a, b| a.repo_name.cmp(&b.repo_name).then(a.rel_path.cmp(&b.rel_path)));
    out
}

/// 暂存或取消暂存指定文件列表（支持混合暂存与重命名）。
pub fn stage_or_unstage_files(repo_path: &str, paths: &[String], stage: bool) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let dir = Path::new(repo_path);
    if stage {
        let mut args: Vec<&str> = vec!["add", "--"];
        for p in paths {
            args.push(p.as_str());
        }
        run_git_bytes(dir, &args)?;
    } else {
        // 检查是否存在 HEAD（首提交无 HEAD 时用 rm --cached）
        let has_head = run_git_bytes(dir, &["rev-parse", "--verify", "HEAD"]).is_ok();
        let mut args: Vec<&str> = if has_head {
            vec!["reset", "HEAD", "--"]
        } else {
            vec!["rm", "--cached", "-q", "--"]
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
    let mut out = Vec::new();
    for repo in repos {
        let dir = Path::new(&repo.path);
        if let Ok(text) = run_git_string(dir, &["stash", "list", "--format=%gd%x1f%gs"]) {
            for line in text.lines() {
                if let Some((sel, msg)) = line.split_once('\x1f') {
                    out.push(StashEntry {
                        repo_id: repo.id.clone(),
                        repo_name: repo.name.clone(),
                        repo_path: repo.path.clone(),
                        selector: sel.to_string(),
                        message: msg.to_string(),
                    });
                }
            }
        }
    }
    out
}
