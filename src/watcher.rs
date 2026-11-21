use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::git::RepoInfo;

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(180);

/// 监听提示：映射变更事件到受影响的仓库及刷新范围（对齐旧版 `classifyWatchHints`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchHint {
    pub repo_ids: HashSet<String>,
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
    let mut hint = WatchHint::default();
    let ws_canon = canonicalize_best_effort(workspace_path);
    let repo_canons: Vec<(&RepoInfo, PathBuf)> = repos
        .iter()
        .map(|r| (r, canonicalize_best_effort(Path::new(&r.path))))
        .collect();

    for raw_path in paths {
        let canon = canonicalize_best_effort(raw_path);
        // 仅按工作区相对路径判断构建忽略目录，避免工作区自身位于 `build/` 或 `target/` 等父目录下时误过滤全部事件
        let rel_for_ignore = canon.strip_prefix(&ws_canon).unwrap_or(&canon);
        if should_ignore_watch_path(rel_for_ignore) {
            continue;
        }
        let file_name = canon.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if (file_name == "mani.yaml" || file_name == "mani.yml")
            && canon.parent().map(|p| p == ws_canon).unwrap_or(false)
        {
            hint.mani_changed = true;
        }

        let norm = canon.to_string_lossy().replace('\\', "/");
        let is_git_meta = norm.contains("/.git/") || norm.ends_with("/.git");
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
        }
    }

    hint
}

/// 持有后台文件监听线程与停止句柄，离开作用域自动优雅退出。
pub struct WorkspaceWatcher {
    stop_tx: Option<mpsc::Sender<()>>,
    repos: Arc<RwLock<Vec<RepoInfo>>>,
}

impl WorkspaceWatcher {
    pub fn start(
        workspace_path: PathBuf,
        initial_repos: Vec<RepoInfo>,
        on_hint: impl Fn(WatchHint) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ev_tx, ev_rx) = mpsc::channel::<notify::Result<Event>>();
        let repos = Arc::new(RwLock::new(initial_repos));
        let repos_bg = repos.clone();

        let watch_root = canonicalize_best_effort(&workspace_path);
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = ev_tx.send(res);
            },
            Config::default(),
        )?;

        if watch_root.exists() {
            watcher.watch(&watch_root, RecursiveMode::Recursive)?;
        }

        std::thread::Builder::new()
            .name("nemorous-watcher".to_string())
            .spawn(move || {
                let _watcher = watcher;
                let mut pending = WatchHint::default();
                let mut last_event: Option<Instant> = None;

                loop {
                    if stop_rx.try_recv().is_ok() {
                        break;
                    }
                    match ev_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(Ok(event)) => {
                            let repo_list = repos_bg
                                .read()
                                .map(|r| r.clone())
                                .unwrap_or_default();
                            let hint =
                                classify_watch_hints(&watch_root, &repo_list, &event.paths);
                            if !hint.is_empty() {
                                pending.merge(hint);
                                last_event = Some(Instant::now());
                            }
                        }
                        Ok(Err(err)) => {
                            log::debug!("watcher error: {err}");
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
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

        Ok(Self {
            stop_tx: Some(stop_tx),
            repos,
        })
    }

    pub fn update_repos(&self, next: Vec<RepoInfo>) {
        if let Ok(mut guard) = self.repos.write() {
            *guard = next;
        }
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}
