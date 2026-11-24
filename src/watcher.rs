use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::git::{RepoInfo, run_git_string};

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(180);

/// 监听提示：映射变更事件到受影响的仓库及刷新范围（对齐旧版 `classifyWatchHints`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchHint {
    pub repo_ids: HashSet<String>,
    pub history_repo_ids: HashSet<String>,
    pub worktree_changed: bool,
    pub git_meta_changed: bool,
    pub mani_changed: bool,
    pub changed_paths: Vec<PathBuf>,
}

impl WatchHint {
    pub fn is_empty(&self) -> bool {
        !self.worktree_changed && !self.git_meta_changed && !self.mani_changed
    }

    pub fn merge(&mut self, other: WatchHint) {
        self.repo_ids.extend(other.repo_ids);
        self.history_repo_ids.extend(other.history_repo_ids);
        self.worktree_changed |= other.worktree_changed;
        self.git_meta_changed |= other.git_meta_changed;
        self.mani_changed |= other.mani_changed;
        self.changed_paths.extend(other.changed_paths);
    }
}

/// 判断路径是否应被文件监听忽略（与旧版 `mygo.WatchFiles` 保持一致）。
pub fn should_ignore_watch_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let norm = s.replace('\\', "/");
    for seg in norm.split('/') {
        if matches!(
            seg,
            "node_modules"
                | "target"
                | "dist"
                | "build"
                | ".idea"
                | ".gradle"
                | ".next"
                | ".nuxt"
                | "__pycache__"
                | ".venv"
                | "rust-target"
                | ".cargo-home"
        ) {
            return true;
        }
    }
    if norm.contains("/.git/objects")
        || norm.starts_with(".git/objects")
        || norm.contains("/.git/logs")
        || norm.starts_with(".git/logs")
        || norm.contains("/.git/lfs")
        || norm.starts_with(".git/lfs")
        || norm.ends_with(".tmp")
        || norm.ends_with(".lock")
    {
        return true;
    }
    false
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    let mut cur = path;
    let mut suffix = Vec::new();
    while let Some(parent) = cur.parent() {
        if let Some(name) = cur.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(mut base) = std::fs::canonicalize(parent) {
            for seg in suffix.into_iter().rev() {
                base.push(seg);
            }
            return base;
        }
        cur = parent;
    }
    path.to_path_buf()
}

/// 将一批变更路径分类为按仓刷新提示。
pub fn classify_watch_hints(
    workspace_path: &Path,
    repos: &[RepoInfo],
    paths: &[PathBuf],
) -> WatchHint {
    classify_paths(workspace_path, repos, &git_directories(repos), paths)
}

const SHARED_GIT_DIRECTORIES: &[&str] = &["refs", "reftable", "info"];

#[derive(Debug, PartialEq, Eq)]
struct GitDirectories {
    repo_id: String,
    private: PathBuf,
    common: PathBuf,
}

fn git_directories(repos: &[RepoInfo]) -> Vec<GitDirectories> {
    repos.iter().filter_map(|repo| {
        let root = Path::new(&repo.path);
        let marker = root.join(".git");
        let (private, common) = if marker.is_dir() && !marker.join("commondir").is_file() {
            let path = canonicalize_best_effort(&marker);
            (path.clone(), path)
        } else {
            let resolve = |flag: &str| {
                let output = run_git_string(root, &["rev-parse", "--path-format=absolute", flag])
                    .map_err(|err| log::debug!("无法解析 Git 元数据目录 {}: {err}", root.display()))
                    .ok()?;
                let path = Path::new(output.strip_suffix('\n').unwrap_or(&output));
                path.is_absolute().then(|| canonicalize_best_effort(path))
            };
            (resolve("--git-dir")?, resolve("--git-common-dir")?)
        };
        Some(GitDirectories { repo_id: repo.id.clone(), private, common })
    }).collect()
}

impl GitDirectories {
    // None means the event is outside this repository's Git metadata.
    fn affected_by(&self, path: &Path) -> Option<bool> {
        if let Ok(relative) = path.strip_prefix(&self.private) {
            let group = relative.iter().next().and_then(OsStr::to_str);
            return Some(!matches!(group, Some("objects" | "logs" | "lfs" | "worktrees")));
        }
        let relative = path.strip_prefix(&self.common).ok()?;
        if relative.as_os_str().is_empty() {
            return Some(true);
        }
        let Some(group) = relative.iter().next().and_then(OsStr::to_str) else {
            return Some(false);
        };
        Some(SHARED_GIT_DIRECTORIES.contains(&group)
            || (relative.parent() == Some(Path::new(""))
                && matches!(group, "packed-refs" | "config" | "shallow")))
    }

    fn affects_history(&self, path: &Path) -> bool {
        path.strip_prefix(&self.private)
            .is_ok_and(|relative| relative == Path::new("HEAD") || history_ref_path(relative))
            || path.strip_prefix(&self.common).is_ok_and(history_ref_path)
    }
}

fn history_ref_path(relative: &Path) -> bool {
    relative.starts_with("refs")
        || relative.starts_with("reftable")
        || relative == Path::new("packed-refs")
        || relative == Path::new("shallow")
        || relative.as_os_str().is_empty()
}

fn classify_paths(
    workspace_path: &Path,
    repos: &[RepoInfo],
    git_dirs: &[GitDirectories],
    paths: &[PathBuf],
) -> WatchHint {
    let mut hint = WatchHint::default();
    let ws_canon = canonicalize_best_effort(workspace_path);
    let repo_canons: Vec<(&RepoInfo, PathBuf)> = repos
        .iter()
        .map(|r| (r, canonicalize_best_effort(Path::new(&r.path))))
        .collect();

    for raw_path in paths {
        let canon = canonicalize_best_effort(raw_path);
        if canon.extension().is_some_and(|ext| ext == "lock") {
            continue;
        }
        let mut git_metadata = false;
        let mut affected = false;
        for dirs in git_dirs {
            if let Some(matches) = dirs.affected_by(&canon) {
                git_metadata = true;
                if matches {
                    hint.repo_ids.insert(dirs.repo_id.clone());
                    if dirs.affects_history(&canon) {
                        hint.history_repo_ids.insert(dirs.repo_id.clone());
                    }
                    affected = true;
                }
            }
        }
        if git_metadata {
            if affected {
                hint.git_meta_changed = true;
                hint.changed_paths.push(canon);
            }
            continue;
        }
        // 仅按工作区相对路径判断构建忽略目录，避免工作区自身位于 `build/` 或 `target/` 等父目录下时误过滤全部事件
        let Ok(relative) = canon.strip_prefix(&ws_canon) else { continue };
        if should_ignore_watch_path(relative) {
            continue;
        }
        let file_name = canon.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if (file_name == "mani.yaml" || file_name == "mani.yml")
            && canon.parent().map(|p| p == ws_canon).unwrap_or(false)
        {
            hint.mani_changed = true;
        }

        let is_git_meta = relative.iter().any(|component| component == ".git");
        if is_git_meta {
            hint.git_meta_changed = true;
        } else {
            hint.worktree_changed = true;
        }
        hint.changed_paths.push(canon.clone());

        // 最长前缀匹配所属仓库（防止根仓吞掉子仓事件）
        let mut best: Option<(&RepoInfo, usize)> = None;
        for (repo, rc) in &repo_canons {
            if canon.starts_with(rc) {
                let depth = rc.as_os_str().len();
                if best.map(|(_, d)| depth > d).unwrap_or(true) {
                    best = Some((repo, depth));
                }
            }
        }
        if let Some((repo, _)) = best {
            hint.repo_ids.insert(repo.id.clone());
            if is_git_meta && relative.ends_with(".git") {
                hint.history_repo_ids.insert(repo.id.clone());
            }
        }
    }

    hint
}

enum Message {
    Event(notify::Result<Event>),
    Repos(Vec<RepoInfo>),
    Stop,
}

type WatchPaths = HashMap<PathBuf, RecursiveMode>;

fn external_git_paths(workspace: &Path, git_dirs: &[GitDirectories]) -> WatchPaths {
    let mut paths = WatchPaths::new();
    for dirs in git_dirs {
        if dirs.private != dirs.common && !dirs.private.starts_with(workspace) {
            paths.insert(dirs.private.clone(), RecursiveMode::Recursive);
        }
        if !dirs.common.starts_with(workspace) {
            paths.entry(dirs.common.clone()).or_insert(RecursiveMode::NonRecursive);
            for name in SHARED_GIT_DIRECTORIES {
                let path = dirs.common.join(name);
                if path.is_dir() {
                    paths.insert(path, RecursiveMode::Recursive);
                }
            }
        }
    }
    paths
}

fn update_external_git_watches(
    active: &mut Option<RecommendedWatcher>,
    watched: &mut WatchPaths,
    next: WatchPaths,
    sender: &mpsc::Sender<Message>,
) -> notify::Result<()> {
    if *watched == next {
        return Ok(());
    }
    // Reconfiguring FSEvents restarts its stream. Keep the workspace watcher
    // running while registering Git metadata outside the workspace.
    let replacement = if next.is_empty() {
        None
    } else {
        let sender = sender.clone();
        let mut watcher = RecommendedWatcher::new(move |event| {
            let _ = sender.send(Message::Event(event));
        }, Config::default())?;
        let mut paths = watcher.paths_mut();
        for (path, &mode) in &next {
            paths.add(path, mode)?;
        }
        paths.commit()?;
        Some(watcher)
    };
    *active = replacement;
    *watched = next;
    Ok(())
}

/// 持有后台文件监听线程与停止句柄，离开作用域自动优雅退出。
pub struct WorkspaceWatcher {
    sender: mpsc::Sender<Message>,
}

impl WorkspaceWatcher {
    pub fn start(
        workspace_path: PathBuf,
        initial_repos: Vec<RepoInfo>,
        on_hint: impl Fn(WatchHint) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let event_sender = sender.clone();
        let watch_root = canonicalize_best_effort(&workspace_path);
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = event_sender.send(Message::Event(res));
            },
            Config::default(),
        )?;

        if watch_root.exists() {
            watcher.watch(&watch_root, RecursiveMode::Recursive)?;
        }

        let external_sender = sender.clone();
        std::thread::Builder::new()
            .name("nemorous-watcher".to_string())
            .spawn(move || {
                let _workspace = watcher;
                let mut repos = initial_repos;
                let mut git_dirs = git_directories(&repos);
                let mut external = None;
                let mut watched = WatchPaths::new();
                let mut sync_external = |dirs: &[GitDirectories]| {
                    if let Err(err) = update_external_git_watches(
                        &mut external, &mut watched, external_git_paths(&watch_root, dirs), &external_sender,
                    ) {
                        log::warn!("无法监听工作区外的 Git 元数据: {err}");
                    }
                };
                sync_external(&git_dirs);
                let mut pending = WatchHint::default();
                let mut last_event: Option<Instant> = None;

                loop {
                    match receiver.recv_timeout(Duration::from_millis(50)) {
                        Ok(Message::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(Message::Repos(next)) if repos != next => {
                            let next_dirs = git_directories(&next);
                            sync_external(&next_dirs);
                            for dirs in &next_dirs {
                                if (!dirs.private.starts_with(&watch_root) || !dirs.common.starts_with(&watch_root))
                                    && !git_dirs.contains(dirs)
                                {
                                    // A Git write can land between discovery and registration.
                                    pending.repo_ids.insert(dirs.repo_id.clone());
                                    pending.history_repo_ids.insert(dirs.repo_id.clone());
                                    pending.git_meta_changed = true;
                                    last_event = Some(Instant::now());
                                }
                            }
                            repos = next;
                            git_dirs = next_dirs;
                        }
                        Ok(Message::Event(Ok(event))) => {
                            if event.paths.iter().any(|path| path.ends_with(".git")) {
                                let next_dirs = git_directories(&repos);
                                sync_external(&next_dirs);
                                git_dirs = next_dirs;
                            } else if event.paths.iter().any(|path| {
                                path.file_name().and_then(|name| name.to_str())
                                    .is_some_and(|name| SHARED_GIT_DIRECTORIES.contains(&name))
                            }) {
                                sync_external(&git_dirs);
                            }
                            let mut hint = classify_paths(&watch_root, &repos, &git_dirs, &event.paths);
                            if event.need_rescan() {
                                hint.git_meta_changed = true;
                                hint.worktree_changed = true;
                                hint.repo_ids.extend(repos.iter().map(|repo| repo.id.clone()));
                                hint.history_repo_ids.extend(repos.iter().map(|repo| repo.id.clone()));
                            }
                            if !hint.is_empty() {
                                pending.merge(hint);
                                last_event = Some(Instant::now());
                            }
                        }
                        Ok(Message::Event(Err(err))) => {
                            log::debug!("watcher error: {err}");
                        }
                        Ok(Message::Repos(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }

                    if let Some(t) = last_event {
                        if t.elapsed() >= DEBOUNCE_WINDOW && !pending.is_empty() {
                            let emit = std::mem::take(&mut pending);
                            last_event = None;
                            on_hint(emit);
                        }
                    }
                }
            })?;

        Ok(Self { sender })
    }

    pub fn update_repos(&self, next: Vec<RepoInfo>) {
        let _ = self.sender.send(Message::Repos(next));
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
    }
}
