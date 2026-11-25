use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::SystemTime;

use anyhow::{Context, Result, bail, ensure};

use crate::git::{RepoInfo, WorkspaceFile, discover_workspace_repos, run_git_string};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsEntry {
    pub name: String,
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub is_dir: bool,
    pub is_git_repo: bool,
    pub ignored: bool,
}

#[derive(Clone)]
struct IgnoreCacheEntry {
    mtime: Option<SystemTime>,
    ignored: HashSet<String>,
}

fn ignore_cache() -> &'static Mutex<HashMap<PathBuf, IgnoreCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, IgnoreCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn clear_ignore_cache() {
    if let Ok(mut guard) = ignore_cache().lock() {
        guard.clear();
    }
}

fn canonical_or(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn path_eq(a: &Path, b: &Path) -> bool {
    a == b || canonical_or(a) == canonical_or(b)
}

pub(crate) fn workspace_rel_path(workspace_root: &Path, abs_path: &Path) -> Option<String> {
    if let Ok(rel) = abs_path.strip_prefix(workspace_root) {
        return Some(crate::paths::normalize_slashes(&rel.to_string_lossy()));
    }
    let canon_ws = canonical_or(workspace_root);
    if let Ok(rel) = abs_path.strip_prefix(&canon_ws) {
        return Some(crate::paths::normalize_slashes(&rel.to_string_lossy()));
    }
    let canon_abs = canonical_or(abs_path);
    canon_abs
        .strip_prefix(&canon_ws)
        .ok()
        .map(|rel| crate::paths::normalize_slashes(&rel.to_string_lossy()))
}

pub(crate) fn find_exact_repo<'a>(repos: &'a [RepoInfo], abs_path: &Path) -> Option<&'a RepoInfo> {
    repos.iter().find(|repo| path_eq(Path::new(&repo.path), abs_path))
}

fn default_ignored_name(name: &str) -> bool {
    matches!(
        name,
        ".DS_Store" | "Thumbs.db" | "node_modules" | "target" | ".idea" | ".vscode"
    )
}

fn default_ignored(names: &[String]) -> HashSet<String> {
    names
        .iter()
        .filter(|name| default_ignored_name(name))
        .cloned()
        .collect()
}

fn ignored_names_in_dir(dir: &Path, git_repo_root: Option<&Path>, names: &[String]) -> HashSet<String> {
    if names.is_empty() {
        return HashSet::new();
    }
    let Some(repo_root) = git_repo_root else {
        return default_ignored(names);
    };
    // 工作区路径可能来自符号链接（例如 macOS 的 /var → /private/var），
    // git 输出与缓存键统一使用真实路径。
    let repo_root = canonical_or(repo_root);
    let dir = canonical_or(dir);
    let mtime = fs::metadata(&dir).ok().and_then(|m| m.modified().ok());
    if let Ok(guard) = ignore_cache().lock()
        && let Some(cached) = guard.get(&dir)
        && cached.mtime == mtime
    {
        return cached.ignored.clone();
    }

    let mut ignored = HashSet::new();
    if let Ok(rel_dir) = dir.strip_prefix(&repo_root) {
        let mut child = match crate::tool_command::command("git")
            .args(["check-ignore", "--stdin", "-z"])
            .current_dir(&repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return default_ignored(names),
        };
        if let Some(mut stdin) = child.stdin.take() {
            for name in names {
                let candidate = if rel_dir.as_os_str().is_empty() {
                    name.clone()
                } else {
                    crate::paths::normalize_slashes(&rel_dir.join(name).to_string_lossy())
                };
                let _ = stdin.write_all(candidate.as_bytes());
                let _ = stdin.write_all(b"\0");
            }
        }
        if let Ok(output) = child.wait_with_output() {
            for raw in output.stdout.split(|b| *b == 0) {
                if raw.is_empty() {
                    continue;
                }
                if let Ok(path_str) = std::str::from_utf8(raw) {
                    let base = path_str
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or(path_str);
                    ignored.insert(base.to_owned());
                }
            }
        }
    }
    if let Ok(mut guard) = ignore_cache().lock() {
        if guard.len() > 512 {
            guard.clear();
        }
        guard.insert(
            dir.clone(),
            IgnoreCacheEntry {
                mtime,
                ignored: ignored.clone(),
            },
        );
    }
    ignored
}

pub(crate) fn find_owning_repo<'a>(repos: &'a [RepoInfo], abs_path: &Path) -> Option<&'a RepoInfo> {
    let mut best: Option<(&RepoInfo, usize)> = None;
    let mut canon_abs: Option<PathBuf> = None;
    for repo in repos {
        let repo_path = Path::new(&repo.path);
        if abs_path == repo_path || abs_path.starts_with(repo_path) {
            let depth = repo.path.len();
            if best.is_none_or(|(_, best_len)| depth > best_len) {
                best = Some((repo, depth));
            }
            continue;
        }
        // 仓库路径与实际路径可能经过不同的符号链接（例如 /var → /private/var）。
        let canon_repo = canonical_or(repo_path);
        let canon_abs = canon_abs.get_or_insert_with(|| canonical_or(abs_path));
        if canon_abs.as_path() == canon_repo.as_path() || canon_abs.starts_with(&canon_repo) {
            let depth = canon_repo.as_os_str().len();
            if best.is_none_or(|(_, best_len)| depth > best_len) {
                best = Some((repo, depth));
            }
        }
    }
    best.map(|(repo, _)| repo)
}

pub(crate) fn resolve_workspace_file(
    workspace_root: &Path,
    repos: &[RepoInfo],
    abs_path: &Path,
) -> Option<WorkspaceFile> {
    if let Some(repo) = find_owning_repo(repos, abs_path)
        && let Some(wf) = project_file(repo, abs_path)
    {
        return Some(wf);
    }
    let root_str = workspace_root.to_str()?;
    let folder = folder_project(root_str);
    project_file(&folder, abs_path)
}

pub(crate) fn read_dir_entries(
    workspace_root: &Path,
    dir: &Path,
    repos: &[RepoInfo],
) -> Vec<FsEntry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let canon_ws = canonical_or(workspace_root);
    let mut raw_items = Vec::new();
    let mut names = Vec::new();
    for entry in read_dir.flatten() {
        let os_name = entry.file_name();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if name == ".git" {
            continue;
        }
        let abs_path = entry.path();
        let is_dir = entry
            .file_type()
            .map(|ft| ft.is_dir() || (ft.is_symlink() && abs_path.is_dir()))
            .unwrap_or(false);
        let rel_path = abs_path
            .strip_prefix(workspace_root)
            .ok()
            .or_else(|| abs_path.strip_prefix(&canon_ws).ok())
            .map(|p| crate::paths::normalize_slashes(&p.to_string_lossy()))
            .or_else(|| {
                let canon_abs = canonical_or(&abs_path);
                canon_abs
                    .strip_prefix(&canon_ws)
                    .ok()
                    .map(|p| crate::paths::normalize_slashes(&p.to_string_lossy()))
            })
            .unwrap_or_else(|| name.to_owned());
        let is_git_repo = is_dir && abs_path.join(".git").exists();
        names.push(name.to_owned());
        raw_items.push((name.to_owned(), rel_path, abs_path, is_dir, is_git_repo));
    }

    let owning_repo = find_owning_repo(repos, dir).map(|r| Path::new(&r.path));
    let ignored_set = ignored_names_in_dir(dir, owning_repo, &names);

    let mut entries: Vec<FsEntry> = raw_items
        .into_iter()
        .map(|(name, rel_path, abs_path, is_dir, is_git_repo)| {
            let ignored = !is_git_repo && ignored_set.contains(&name);
            FsEntry {
                name,
                rel_path,
                abs_path,
                is_dir,
                is_git_repo,
                ignored,
            }
        })
        .collect();

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    entries
}

fn ensure_strictly_inside(workspace_root: &Path, target: &Path) -> Result<(PathBuf, PathBuf)> {
    let root = workspace_path(workspace_root)?;
    let parent = target
        .parent()
        .context("无法确定目标的上级目录")?;
    let canon_parent = workspace_path(parent)?;
    let name = target
        .file_name()
        .context("无效的目标名称")?;
    let resolved = canon_parent.join(name);
    ensure!(
        resolved.starts_with(&root) && resolved != root,
        "不能对工作区根目录或工作区之外的路径执行此操作"
    );
    Ok((root, resolved))
}

pub(crate) fn rename_project_item(
    workspace_root: &Path,
    target: &Path,
    new_name: &str,
) -> Result<PathBuf> {
    let (root, target) = ensure_strictly_inside(workspace_root, target)?;
    let parent = target.parent().context("无法确定目标目录")?;
    let dest = new_item_path(parent, new_name)?;
    ensure!(dest.starts_with(&root), "目标路径超出工作区范围");
    if target == dest {
        return Ok(dest);
    }
    let case_only = target
        .file_name()
        .and_then(|s| s.to_str())
        .zip(dest.file_name().and_then(|s| s.to_str()))
        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
    if !case_only && fs::symlink_metadata(&dest).is_ok() {
        bail!("目标名称已存在");
    }
    fs::rename(&target, &dest).context("重命名失败")?;
    clear_ignore_cache();
    Ok(dest)
}

pub(crate) fn delete_project_item(workspace_root: &Path, target: &Path) -> Result<()> {
    let (_root, target) = ensure_strictly_inside(workspace_root, target)?;
    let meta = fs::symlink_metadata(&target).context("目标不存在")?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        fs::remove_file(&target).context("删除文件失败")?;
    } else {
        fs::remove_dir_all(&target).context("删除目录失败")?;
    }
    clear_ignore_cache();
    Ok(())
}

pub(crate) fn move_project_item(
    workspace_root: &Path,
    src: &Path,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let (root, src) = ensure_strictly_inside(workspace_root, src)?;
    let dest_dir = workspace_path(dest_dir)?;
    ensure!(dest_dir.starts_with(&root), "目标目录超出工作区范围");
    ensure!(dest_dir.is_dir(), "目标必须是目录");
    ensure!(
        dest_dir != src && !dest_dir.starts_with(&src),
        "不能将目录移动到自身或其子目录中"
    );
    if src.parent() == Some(dest_dir.as_path()) {
        return Ok(src);
    }
    let name = src.file_name().context("无效的源文件名")?;
    let dest = dest_dir.join(name);
    ensure!(
        fs::symlink_metadata(&dest).is_err(),
        "目标目录中已存在同名项：{}",
        name.to_string_lossy()
    );
    fs::rename(&src, &dest).context("移动失败")?;
    clear_ignore_cache();
    Ok(dest)
}

fn unique_copy_dest(dest_dir: &Path, src_name: &str, is_dir: bool) -> PathBuf {
    let direct = dest_dir.join(src_name);
    if fs::symlink_metadata(&direct).is_err() {
        return direct;
    }
    let (stem, ext) = if is_dir {
        (src_name, "")
    } else if let Some((s, e)) = src_name.rsplit_once('.')
        && !s.is_empty()
    {
        (s, e)
    } else {
        (src_name, "")
    };
    for index in 1..10_000 {
        let suffix = if index == 1 {
            " copy".to_owned()
        } else {
            format!(" copy {index}")
        };
        let candidate_name = if ext.is_empty() {
            format!("{stem}{suffix}")
        } else {
            format!("{stem}{suffix}.{ext}")
        };
        let candidate = dest_dir.join(candidate_name);
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    dest_dir.join(format!("{src_name}_copy"))
}

fn copy_recursive(src: &Path, dest: &Path, depth: usize) -> Result<()> {
    ensure!(depth <= 64, "目录层级过深，无法复制");
    let meta = fs::symlink_metadata(src).context("无法读取源项")?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dest).context("复制符号链接失败")?;
        #[cfg(windows)]
        {
            if src.is_dir() {
                std::os::windows::fs::symlink_dir(&target, dest).context("复制符号链接失败")?;
            } else {
                std::os::windows::fs::symlink_file(&target, dest).context("复制符号链接失败")?;
            }
        }
        return Ok(());
    }
    if meta.is_file() {
        fs::copy(src, dest).context("复制文件失败")?;
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir(dest).context("创建目标目录失败")?;
        for entry in fs::read_dir(src).context("读取源目录失败")? {
            let entry = entry?;
            let child_src = entry.path();
            let child_dest = dest.join(entry.file_name());
            copy_recursive(&child_src, &child_dest, depth + 1)?;
        }
    }
    Ok(())
}

pub(crate) fn copy_project_item(
    workspace_root: &Path,
    src: &Path,
    dest_dir: &Path,
) -> Result<PathBuf> {
    let (root, src) = ensure_strictly_inside(workspace_root, src)?;
    let dest_dir = workspace_path(dest_dir)?;
    ensure!(dest_dir.starts_with(&root), "目标目录超出工作区范围");
    ensure!(dest_dir.is_dir(), "目标必须是目录");
    ensure!(
        !dest_dir.starts_with(&src),
        "不能将目录复制到自身或其子目录中"
    );
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .context("无效的源文件名")?;
    let is_dir = fs::symlink_metadata(&src).map(|m| m.is_dir()).unwrap_or(false);
    let dest = unique_copy_dest(&dest_dir, name, is_dir);
    copy_recursive(&src, &dest, 0)?;
    clear_ignore_cache();
    Ok(dest)
}

pub(crate) fn workspace_path(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize().context("无法打开工作区目录")?;
    ensure!(path.is_dir(), "请选择工作区目录");
    ensure!(path.to_str().is_some(), "工作区路径必须是 UTF-8");
    Ok(path)
}

pub(crate) fn new_item_path(parent: &Path, name: &str) -> Result<PathBuf> {
    let mut components = Path::new(name).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(value)) if value == name)
            && components.next().is_none()
            && !name.contains(['\r', '\n', '\0']),
        "请输入单个有效的名称"
    );
    Ok(workspace_path(parent)?.join(name))
}

fn target(workspace: &Path, name: &str) -> Result<(PathBuf, PathBuf)> {
    ensure!(!name.starts_with('.'), "请输入有效的仓库目录名");
    let workspace = workspace_path(workspace)?;
    let path = new_item_path(&workspace, name)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => bail!("目标目录已经存在"),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).context("无法检查目标目录"),
    }
    Ok((workspace, path))
}

pub(crate) fn create_project(parent: &Path, name: &str, branch: Option<&str>) -> Result<PathBuf> {
    if let Some(branch) = branch {
        return init_repo(parent, name, branch);
    }
    let path = new_item_path(parent, name)?;
    fs::create_dir(&path).context("无法创建项目目录")?;
    Ok(path)
}

pub(crate) fn create_project_item(
    root: &Path, parent: &Path, name: &str, directory: bool,
) -> Result<PathBuf> {
    let root = workspace_path(root)?;
    let path = new_item_path(parent, name)?;
    ensure!(path.starts_with(root), "请选择项目内的目录");
    if directory {
        fs::create_dir(&path).context("无法创建目录")?;
    } else {
        OpenOptions::new().write(true).create_new(true).open(&path).context("无法创建文件")?;
    }
    Ok(path)
}

pub(crate) fn folder_project(path: &str) -> RepoInfo {
    use sha1::{Digest, Sha1};

    let name = crate::paths::basename(path);
    RepoInfo {
        id: format!("folder-{:x}", Sha1::digest(path.as_bytes())),
        monogram: name.chars().take(2).flat_map(char::to_uppercase).collect(),
        name,
        display_name: None,
        path: path.to_owned(),
        rel_path: ".".into(),
        desc: String::new(),
        color_idx: 0,
        is_root: true,
    }
}

pub(crate) fn project_file(repo: &RepoInfo, path: &Path) -> Option<WorkspaceFile> {
    let rel_path = crate::paths::normalize_slashes(path.strip_prefix(&repo.path).ok()?.to_str()?);
    Some(WorkspaceFile {
        repo_id: repo.id.clone(), repo_name: repo.label().to_owned(), repo_path: repo.path.clone(),
        rel_path, abs_path: path.to_str()?.to_owned(), color_idx: repo.color_idx,
    })
}

pub(crate) fn read_project_folder(repo: &RepoInfo, cancelled: &AtomicBool) -> Result<(Vec<WorkspaceFile>, Vec<String>)> {
    const MAX_ENTRIES: usize = 50_000;
    let mut pending = vec![PathBuf::from(&repo.path)];
    let mut files = Vec::new();
    let mut directories = Vec::new();
    let mut scanned = 0;
    while let Some(parent) = pending.pop() {
        for entry in fs::read_dir(&parent)? {
            if cancelled.load(Ordering::Relaxed) {
                return Ok((files, directories));
            }
            scanned += 1;
            ensure!(scanned <= MAX_ENTRIES, "目录内容超过 {MAX_ENTRIES} 项，请打开更小的项目目录");
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let path = entry.path();
            if kind.is_dir() && !matches!(name, ".git" | "node_modules" | "target" | "build") {
                if let Some(item) = project_file(repo, &path) {
                    directories.push(item.rel_path);
                    pending.push(path);
                }
            } else if kind.is_file() && name != ".git" {
                files.extend(project_file(repo, &path));
            }
        }
    }
    directories.sort_unstable();
    Ok((files, directories))
}

pub(crate) fn clone_repo(workspace: &Path, source: &str, name: &str) -> Result<PathBuf> {
    ensure!(!source.trim().is_empty(), "请输入仓库 URL 或本地路径");
    let (workspace, path) = target(workspace, name)?;
    // The Git client can echo URL userinfo on failure; surface a status without the URL.
    let outcome = crate::tool_command::command("git")
        .args(["-c", "protocol.ext.allow=never", "clone", "--", source])
        .arg(&path)
        .current_dir(&workspace)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("无法启动 Git 克隆")?;
    ensure!(
        outcome.success(),
        "Git 克隆失败（退出码 {:?}）",
        outcome.code()
    );
    Ok(path)
}

pub(crate) fn init_repo(workspace: &Path, name: &str, branch: &str) -> Result<PathBuf> {
    let (workspace, path) = target(workspace, name)?;
    let branch = format!("--initial-branch={branch}");
    run_git_string(
        &workspace,
        &[
            "init",
            &branch,
            "--",
            path.to_str().context("无效仓库路径")?,
        ],
    )?;
    Ok(path)
}

pub(crate) fn import_repo(workspace: &Path, repo: &Path) -> Result<PathBuf> {
    let workspace = workspace_path(workspace)?;
    let repo = workspace_path(repo)?;
    let root = run_git_string(&repo, &["rev-parse", "--show-toplevel"])?;
    ensure!(
        Path::new(root.strip_suffix('\n').unwrap_or(&root)).canonicalize()? == repo,
        "请选择 Git 仓库的根目录"
    );
    let workspace_str = workspace.to_str().context("无效工作区路径")?;
    let existing = discover_workspace_repos(workspace_str, &[]);
    if existing
        .iter()
        .any(|item| Path::new(&item.path).canonicalize().ok().as_ref() == Some(&repo))
    {
        return Ok(repo);
    }
    let rel = repo
        .strip_prefix(&workspace)
        .unwrap_or(&repo)
        .to_str()
        .context("无效仓库路径")?;
    let name = repo
        .file_name()
        .and_then(|part| part.to_str())
        .context("无效仓库名")?;
    ensure!(
        !existing.iter().any(|item| item.name == name),
        "已有同名项目，请改用其他目录名"
    );
    let valid_scalar =
        |value: &str| !value.contains(['\r', '\n', '\0', '"', '\\']) && !value.contains('\'');
    ensure!(
        valid_scalar(name) && valid_scalar(rel),
        "mani 项目路径含无法安全表示的字符"
    );

    let yaml = workspace.join("mani.yaml");
    let yml = workspace.join("mani.yml");
    let file = if !yaml.exists() && yml.exists() {
        yml
    } else {
        yaml
    };
    let original = match fs::symlink_metadata(&file) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.is_symlink() && meta.len() < 2 * 1024 * 1024,
                "无法编辑现有 mani 清单"
            );
            Some(fs::read_to_string(&file)?)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).context("无法检查 mani 清单"),
    };
    let next = append_project(original.as_deref().unwrap_or(""), name, rel)?;
    let tmp = workspace.join(".mani.nemorous.lock");
    let mut write = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .context("另一个 mani 清单写入正在进行")?;
    let installed = (|| -> Result<()> {
        write.write_all(next.as_bytes())?;
        if let Ok(meta) = fs::symlink_metadata(&file) {
            ensure!(meta.is_file() && !meta.is_symlink(), "mani 清单类型已变化");
            write.set_permissions(meta.permissions())?;
        }
        write.sync_all()?;
        drop(write);
        let current = match fs::read_to_string(&file) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        ensure!(current == original, "mani 清单已在外部修改，请刷新后再导入");
        fs::rename(&tmp, &file)?;
        Ok(())
    })();
    if installed.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    installed?;
    Ok(repo)
}

fn append_project(original: &str, name: &str, path: &str) -> Result<String> {
    let eol = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut projects = false;
    let mut encountered = false;
    let mut insertion = original.len();
    let mut segment = None;
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        let trimmed = line.trim();
        let starts_at_root =
            !line.starts_with([' ', '\t']) && !trimmed.is_empty() && !trimmed.starts_with('#');
        if starts_at_root {
            if projects && insertion == original.len() {
                insertion = offset;
            }
            projects = trimmed.starts_with("projects:");
            if projects {
                ensure!(!encountered, "mani 清单有重复 projects 段");
                encountered = true;
                let value = trimmed
                    .strip_prefix("projects:")
                    .unwrap_or_default()
                    .trim_start();
                ensure!(
                    value.is_empty() || value.starts_with('#') || value == "{}",
                    "mani 的 projects 格式无法安全追加"
                );
                if value == "{}" {
                    let begin = offset + line.find("{}").context("无效 mani 清单")?;
                    segment = Some(begin..begin + 2);
                }
            }
        } else if projects {
            let raw = line.trim_end();
            if raw.starts_with("  ") && !raw.starts_with("   ") && trimmed.ends_with(':') {
                let key = trimmed
                    .trim_end_matches(':')
                    .trim()
                    .trim_matches(['\'', '"']);
                ensure!(key != name, "mani 清单已有同名项目");
            }
        }
        offset += line.len();
    }
    let mut next = String::with_capacity(original.len() + name.len() + path.len() + 30);
    next.push_str(&original[..insertion]);
    if let Some(segment) = segment {
        next.replace_range(segment, "");
    }
    if !next.is_empty() && !next.ends_with('\n') {
        next.push_str(eol);
    }
    if !encountered {
        next.push_str("projects:");
        next.push_str(eol);
    }
    next.push_str(&format!("  \"{name}\":{eol}    path: \"{path}\"{eol}"));
    next.push_str(&original[insertion..]);
    Ok(next)
}

pub(crate) fn launch_window(workspace: &Path) -> Result<()> {
    let workspace = workspace_path(workspace)?;
    let mut instance = Command::new(std::env::current_exe()?)
        .current_dir(&workspace)
        .env("NEMOROUS_WORKSPACE", &workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法打开 Nemorous 新窗口")?;
    std::thread::spawn(move || {
        let _ = instance.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_init_clone_import_preserve_manifest_and_find_nested_root() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("nemorous-workspaces-{}-{id}", std::process::id()));
        fs::create_dir(&root).unwrap();
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(root.clone());
        let workspace = init_repo(&root, "工作区", "main").unwrap();
        let source = init_repo(&root, "源仓", "main").unwrap();
        fs::create_dir(workspace.join("nested")).unwrap();
        let nested = init_repo(&workspace.join("nested"), "仓", "main").unwrap();
        let cloned = clone_repo(&workspace, source.to_str().unwrap(), "cloned").unwrap();
        assert!(cloned.join(".git").exists());
        let original = "# keep comment\r\nprojects:\r\n  root:\r\n    path: .\r\n    desc: keep me\r\n    url: keep URL\r\ntasks:\r\n  check: git status\r\n";
        let manifest = workspace.join("mani.yml");
        fs::write(&manifest, original).unwrap();
        import_repo(&workspace, &nested).unwrap();
        import_repo(&workspace, &source).unwrap();
        let changed = fs::read_to_string(&manifest).unwrap();
        assert!(changed.starts_with("# keep comment\r\nprojects:\r\n  root:\r\n    path: .\r\n    desc: keep me\r\n    url: keep URL\r\n"));
        assert!(changed.ends_with("tasks:\r\n  check: git status\r\n"));
        assert!(changed.contains("path: \"nested/仓\""));
        let repos = discover_workspace_repos(workspace.to_str().unwrap(), &[]);
        assert_eq!(repos.len(), 4);
        assert!(
            repos
                .iter()
                .any(|repo| repo.is_root && repo.id == "root" && repo.desc == "keep me")
        );
        import_repo(&workspace, &source).unwrap();
        assert_eq!(fs::read_to_string(manifest).unwrap(), changed);
        assert!(init_repo(&workspace, "../outside", "main").is_err());
    }

    #[test]
    fn real_file_tree_crud_rename_copy_move_delete_and_git_repo_detection() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("nemorous-fsops-{}-{id}", std::process::id()));
        fs::create_dir(&root).unwrap();
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(root.clone());

        let repo_dir = init_repo(&root, "app-repo", "main").unwrap();
        fs::write(repo_dir.join(".gitignore"), "*.log\nignored_dir/\n").unwrap();
        fs::write(repo_dir.join("debug.log"), "ignored").unwrap();
        fs::create_dir(repo_dir.join("ignored_dir")).unwrap();
        fs::write(repo_dir.join("main.rs"), "fn main() {}").unwrap();
        fs::create_dir(root.join("plain-docs")).unwrap();
        fs::write(root.join("plain-docs").join("guide.md"), "# Guide").unwrap();
        fs::write(root.join("notes.txt"), "root note").unwrap();

        let repos = discover_workspace_repos(root.to_str().unwrap(), &[]);
        let root_entries = read_dir_entries(&root, &root, &repos);
        // Directories first, then files; Git repo detected
        assert_eq!(root_entries.len(), 3);
        assert!(root_entries[0].is_dir && root_entries[0].name == "app-repo" && root_entries[0].is_git_repo);
        assert!(root_entries[1].is_dir && root_entries[1].name == "plain-docs" && !root_entries[1].is_git_repo);
        assert!(!root_entries[2].is_dir && root_entries[2].name == "notes.txt");

        // Ignored detection inside git repo
        let repo_entries = read_dir_entries(&root, &repo_dir, &repos);
        assert!(repo_entries.iter().any(|e| e.name == "debug.log" && e.ignored));
        assert!(repo_entries.iter().any(|e| e.name == "ignored_dir" && e.ignored));
        assert!(repo_entries.iter().any(|e| e.name == "main.rs" && !e.ignored));

        // Copy into same directory generates unique name
        let copied = copy_project_item(&root, &root.join("notes.txt"), &root).unwrap();
        assert_eq!(copied.file_name().unwrap(), "notes copy.txt");
        assert_eq!(fs::read_to_string(&copied).unwrap(), "root note");

        // Rename
        let renamed = rename_project_item(&root, &copied, "renamed.txt").unwrap();
        assert_eq!(renamed.file_name().unwrap(), "renamed.txt");
        assert!(!copied.exists());

        // Move into plain-docs
        let moved = move_project_item(&root, &renamed, &root.join("plain-docs")).unwrap();
        assert_eq!(moved, root.canonicalize().unwrap().join("plain-docs").join("renamed.txt"));
        assert!(!renamed.exists());

        // Cannot move dir into itself
        assert!(move_project_item(&root, &root.join("plain-docs"), &root.join("plain-docs")).is_err());

        // Permanent delete file & dir
        delete_project_item(&root, &moved).unwrap();
        assert!(!moved.exists());
        delete_project_item(&root, &root.join("plain-docs")).unwrap();
        assert!(!root.join("plain-docs").exists());

        // Cannot delete workspace root
        assert!(delete_project_item(&root, &root).is_err());
    }
}
