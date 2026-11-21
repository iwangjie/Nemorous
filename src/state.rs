use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use nsh_base::{EditorApi, EditorPosition, EditorSelection};
use nsh_core::prelude::*;

use crate::config::{self, Config};
use crate::editor_split::{EditorSplit, UndoLog};
use crate::diff_model::{
    ComputedDiff, DiffSide, InlineHighlightMode, WhitespaceMode, compute_diff,
    compute_unified_layout, map_side_line_to_unified, source_position,
};
use crate::git::{
    self, ChangedFile, CommitDiffData, CommitEntry, CommitFileEntry, LineEnding, RepoInfo,
    RepoStatus, ReviewChangeData, ReviewVersion, SaveReviewOutcome, StashEntry, WorkspaceFile,
};
use crate::local_history::LocalHistoryStore;
use crate::paths::{
    self, basename, display_path, expand_tilde, nearest_existing_path, parent_dir_display,
    parse_file_location, path_with_line_reference, relative_slash_path,
};
use crate::theme::{self, Palette};
use crate::watcher::{WatchHint, WorkspaceWatcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftTab {
    Commit,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTabKind {
    Welcome,
    File,
    WorkingDiff,
    CommitDiff,
    Comparison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusZone {
    CommitTree,
    ProjectTree,
    GitFiles,
    CodeEditor,
    WorkingDiffOld,
    WorkingDiffNew,
    CommitDiffOld,
    CommitDiffNew,
    ComparisonOld,
    ComparisonNew,
    Modal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSelection {
    pub file: ChangedFile,
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFileSelection {
    pub commit: CommitEntry,
    pub file: CommitFileEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavTarget {
    CodeFile {
        repo_id: String,
        rel_path: String,
        pos: EditorPosition,
        scroll_y: i32,
    },
    WorkingDiff {
        file_key: String,
        staged: bool,
        side: DiffSide,
        pos: EditorPosition,
        scroll_y: i32,
    },
    CommitDiff {
        repo_id: String,
        commit_hash: String,
        file_path: String,
        side: DiffSide,
        pos: EditorPosition,
        scroll_y: i32,
    },
}

#[derive(Debug, Clone)]
pub struct CodeTab {
    pub id: String,
    pub file: WorkspaceFile,
    /// 若非空表示只读的历史修订版本预览（如 git 某提交版本）
    pub revision: String,
    pub text: String,
    pub saved_text: String,
    pub line_ending: LineEnding,
    pub version: ReviewVersion,
    pub editable: bool,
    pub readonly_reason: Option<String>,
    pub binary: bool,
    pub too_large: bool,
    pub cursor: EditorPosition,
    pub scroll_y: f32,
    pub(crate) scroll_x: f32,
    pub external_changed: bool,
    /// 未钉住的预览标签：斜体显示，可被后续预览复用替换；编辑或双击后转为普通标签
    pub preview: bool,
    /// 递增版本号，用于受控同步外部刷新/恢复内容到 Editor
    pub content_rev: u64,
}

impl CodeTab {
    pub fn is_dirty(&self) -> bool {
        self.editable && self.revision.is_empty() && self.text != self.saved_text
    }

    pub fn title(&self) -> String {
        let base = basename(&self.file.rel_path);
        if self.revision.is_empty() {
            base
        } else {
            format!("{base} ({})", self.revision.get(..8).unwrap_or(&self.revision))
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkingDiffSession {
    pub file: ChangedFile,
    pub data: ReviewChangeData,
    pub current_new_text: String,
    pub saved_new_text: String,
    pub computed: ComputedDiff,
    pub active_side: DiffSide,
    pub old_cursor: EditorPosition,
    pub new_cursor: EditorPosition,
    pub old_scroll_y: f32,
    pub new_scroll_y: f32,
    pub current_hunk: usize,
    pub boundary_hint: Option<i32>, // -1 上边界，+1 下边界
    pub external_changed: bool,
    pub content_rev: u64,
}

impl WorkingDiffSession {
    pub fn is_dirty(&self) -> bool {
        self.data.editable && !self.data.staged_only && self.current_new_text != self.saved_new_text
    }

    pub fn active_cursor(&self) -> EditorPosition {
        match self.active_side {
            DiffSide::Old => self.old_cursor,
            DiffSide::New => self.new_cursor,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommitDiffSession {
    pub commit: CommitEntry,
    pub file: CommitFileEntry,
    pub data: CommitDiffData,
    pub computed: ComputedDiff,
    pub active_side: DiffSide,
    pub old_cursor: EditorPosition,
    pub new_cursor: EditorPosition,
    pub old_scroll_y: f32,
    pub new_scroll_y: f32,
    pub current_hunk: usize,
    pub boundary_hint: Option<i32>,
    pub content_rev: u64,
}

impl CommitDiffSession {
    pub fn active_cursor(&self) -> EditorPosition {
        match self.active_side {
            DiffSide::Old => self.old_cursor,
            DiffSide::New => self.new_cursor,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConflictTarget {
    CodeTab { tab_id: String },
    WorkingDiff { file_key: String },
}

#[derive(Debug, Clone)]
pub struct SaveConflictModal {
    pub target: ConflictTarget,
    pub repo_id: String,
    pub rel_path: String,
    pub abs_path: String,
    pub disk_text: String,
    pub disk_version: ReviewVersion,
    pub disk_deleted: bool,
    pub edited_text: String,
    pub warning_message: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DirtyCloseAction {
    CloseSingleCodeTab(String),
    CloseOtherCodeTabs(String),
    CloseAllCodeTabs,
    CloseWorkingDiff,
}

#[derive(Debug, Clone)]
pub struct DirtyCloseModal {
    pub action: DirtyCloseAction,
    pub dirty_titles: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    GoToFile,
    RecentLocations,
}

#[derive(Debug, Clone)]
pub struct ToastItem {
    pub id: u64,
    pub kind: &'static str, // "info" | "ok" | "warn" | "err"
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct FileRevisionHistory {
    pub repo: RepoInfo,
    pub rel_path: String,
    pub rev: String,
    pub commits: Vec<CommitEntry>,
    pub(crate) paths: HashMap<String, String>,
    pub(crate) show_affected: bool,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

/// 编辑器 ref 注册表，允许快捷键与工具栏命令精准操控当前活跃的 Editor 实例。
#[derive(Clone, Default)]
pub struct EditorRefs {
    pub code: Option<ComponentRef<nsh_base::Editor>>,
    pub split_left: Option<ComponentRef<nsh_base::Editor>>,
    pub split_right: Option<ComponentRef<nsh_base::Editor>>,
    pub work_old: Option<ComponentRef<nsh_base::Editor>>,
    pub work_new: Option<ComponentRef<nsh_base::Editor>>,
    pub work_unified: Option<ComponentRef<nsh_base::Editor>>,
    pub commit_old: Option<ComponentRef<nsh_base::Editor>>,
    pub commit_new: Option<ComponentRef<nsh_base::Editor>>,
    pub commit_unified: Option<ComponentRef<nsh_base::Editor>>,
    pub compare_old: Option<ComponentRef<nsh_base::Editor>>,
    pub compare_new: Option<ComponentRef<nsh_base::Editor>>,
    pub modal_active: Option<ComponentRef<nsh_base::Editor>>,
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Config,
    pub repos: Vec<RepoInfo>,
    pub statuses: Vec<RepoStatus>,
    pub workspace_files: Vec<WorkspaceFile>,
    pub stashes: Vec<StashEntry>,

    // 布局与工具窗口状态
    pub left_tab: LeftTab,
    pub show_left_panel: bool,
    pub show_git_panel: bool,
    pub editor_tab: EditorTabKind,
    pub focus_zone: FocusZone,
    pub prev_focus_zone: FocusZone,

    // Commit 树状态（选中与提交勾选独立）
    pub changes_filter: String,
    pub change_selection: Option<ChangeSelection>,
    pub change_selected_dir: Option<(String, String, String)>, // (repo_id, rel_dir, abs_dir)
    pub checked_changes: HashSet<String>,
    pub(crate) included_hunks: HashMap<String, crate::ui::git_review::IncludedHunks>,
    pub known_change_keys: HashSet<String>,
    pub last_checked_key: Option<String>,
    pub last_checked_staged: Option<bool>,
    pub collapsed_staged_section: bool,
    pub collapsed_unstaged_section: bool,
    pub collapsed_change_repos: HashSet<String>,
    pub collapsed_change_dirs: HashSet<String>,

    // Project 树状态
    pub project_filter: String,
    pub project_workspace_expanded: bool,
    pub expanded_project_repos: HashSet<String>,
    pub expanded_project_dirs: HashSet<String>,
    pub(crate) project_compare_keys: Vec<String>,
    pub project_located_key: Option<String>,
    pub project_selected_file: Option<WorkspaceFile>,
    pub project_selected_dir: Option<(String, String)>, // (repo_id, rel_dir)

    // Git 历史面板状态
    pub git_commits: Vec<CommitEntry>,
    pub selected_commit: Option<CommitEntry>,
    pub selected_commit_files: Vec<CommitFileEntry>,
    pub selected_commit_file: Option<CommitFileEntry>,
    pub file_history: Option<FileRevisionHistory>,

    // 统一编辑区状态
    pub code_tabs: Vec<CodeTab>,
    pub(crate) code_split: Option<EditorSplit>,
    pub(crate) code_undo: HashMap<String, UndoLog>,
    pub active_code_tab_idx: usize,
    pub working_diff: Option<WorkingDiffSession>,
    pub commit_diff: Option<CommitDiffSession>,
    pub show_diff_options: bool,
    pub show_code_options: bool,

    // 查找替换条状态
    pub diff_find_open: bool,
    pub diff_find_query: String,
    pub diff_find_side: DiffSide,
    pub diff_find_matches: (u32, Option<u32>),
    pub code_find_open: bool,
    pub code_replace_open: bool,
    pub code_find_query: String,
    pub code_replace_text: String,
    pub code_find_matches: (u32, Option<u32>),

    // 导航历史与返回差异
    pub nav_back: Vec<NavTarget>,
    pub nav_forward: Vec<NavTarget>,
    pub recent_locations: Vec<NavTarget>,

    // 弹窗与提示
    pub save_conflict: Option<SaveConflictModal>,
    pub dirty_close: Option<DirtyCloseModal>,
    pub goto_line_open: bool,
    pub goto_line_input: String,
    pub picker_open: Option<PickerKind>,
    pub picker_query: String,
    pub stash_modal_open: bool,
    pub toast: Option<ToastItem>,
    pub next_toast_id: u64,

    // 刷新并发与代次控制
    pub refresh_gen: u64,
    pub is_refreshing: bool,
    pub command_busy: bool,
    pub pending_refresh: bool,
    pub pending_refresh_repos: HashSet<String>,
    pub last_refresh_ms: u64,
    pub window_focused: bool,
}

impl Default for AppState {
    fn default() -> Self {
        let cfg = config::load();
        Self {
            cfg,
            repos: Vec::new(),
            statuses: Vec::new(),
            workspace_files: Vec::new(),
            stashes: Vec::new(),
            left_tab: LeftTab::Commit,
            show_left_panel: true,
            show_git_panel: false,
            editor_tab: EditorTabKind::Welcome,
            focus_zone: FocusZone::CommitTree,
            prev_focus_zone: FocusZone::CommitTree,
            changes_filter: String::new(),
            change_selection: None,
            change_selected_dir: None,
            checked_changes: HashSet::new(),
            included_hunks: HashMap::new(),
            known_change_keys: HashSet::new(),
            last_checked_key: None,
            last_checked_staged: None,
            collapsed_staged_section: false,
            collapsed_unstaged_section: false,
            collapsed_change_repos: HashSet::new(),
            collapsed_change_dirs: HashSet::new(),
            project_filter: String::new(),
            project_workspace_expanded: true,
            expanded_project_repos: HashSet::new(),
            expanded_project_dirs: HashSet::new(),
            project_compare_keys: Vec::new(),
            project_located_key: None,
            project_selected_file: None,
            project_selected_dir: None,
            git_commits: Vec::new(),
            selected_commit: None,
            selected_commit_files: Vec::new(),
            selected_commit_file: None,
            file_history: None,
            code_tabs: Vec::new(),
            code_split: None,
            code_undo: HashMap::new(),
            active_code_tab_idx: 0,
            working_diff: None,
            commit_diff: None,
            show_diff_options: false,
            show_code_options: false,
            diff_find_open: false,
            diff_find_query: String::new(),
            diff_find_side: DiffSide::New,
            diff_find_matches: (0, None),
            code_find_open: false,
            code_replace_open: false,
            code_find_query: String::new(),
            code_replace_text: String::new(),
            code_find_matches: (0, None),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            recent_locations: Vec::new(),
            save_conflict: None,
            dirty_close: None,
            goto_line_open: false,
            goto_line_input: String::new(),
            picker_open: None,
            picker_query: String::new(),
            stash_modal_open: false,
            toast: None,
            next_toast_id: 1,
            refresh_gen: 0,
            is_refreshing: false,
            command_busy: false,
            pending_refresh: false,
            pending_refresh_repos: HashSet::new(),
            last_refresh_ms: 0,
            window_focused: true,
        }
    }
}

/// 每个 NSH Surface 独立的全局响应式句柄（测试间完全隔离）。
pub fn app_signal() -> Signal<AppState> {
    global::<AppState>()
}

pub fn editor_refs_signal() -> Signal<EditorRefs> {
    global::<EditorRefs>()
}

#[derive(Default)]
struct WatcherHolder {
    watcher: Option<WorkspaceWatcher>,
    workspace: String,
}

fn watcher_holder() -> Stored<WatcherHolder> {
    // 挂在 global 上由 surface 生命周期托管
    stored(WatcherHolder::default())
}

#[derive(Clone, Default)]
pub struct RootOwnerHolder {
    pub owner: Option<Owner>,
}

pub fn register_root_owner() {
    let cur = Owner::current();
    global::<RootOwnerHolder>().set(RootOwnerHolder { owner: Some(cur) });
}

/// 在应用根 Owner 下弹出菜单，确保列表/局部 dynamic 刷新重建时绝不会清除弹出菜单的回调！
pub fn show_popup_menu(items: Vec<nsh_core::element::desktop::MenuItem>) {
    if let Some(owner) = global::<RootOwnerHolder>().with_untracked(|h| h.owner) {
        owner.run(|| {
            let _ = nsh_core::element::desktop::popup_menu(items);
        });
    } else {
        let _ = nsh_core::element::desktop::popup_menu(items);
    }
}

/// 在应用根 Owner 下按坐标弹出右键菜单。
pub fn show_popup_menu_at(
    items: Vec<nsh_core::element::desktop::MenuItem>,
    at: Point<Points>,
) {
    if let Some(owner) = global::<RootOwnerHolder>().with_untracked(|h| h.owner) {
        owner.run(|| {
            let _ = nsh_core::element::desktop::popup_menu_at(items, at);
        });
    } else {
        let _ = nsh_core::element::desktop::popup_menu_at(items, at);
    }
}

#[derive(Clone, Default)]
pub struct ShortcutRebindState {
    pub rev: u64,
}

/// 独立于 AppState 的快捷键重绑版本信号：每当 Editor 重新获得焦点后触发一次重绑，
/// 确保应用级 IDEA 快捷键（`⌘L`、`⌘[`、`⌘]`、`⌘D`、`F4`）始终排在 `Editor::bind_keys` 之后生效。
pub fn shortcut_rebind_signal() -> Signal<ShortcutRebindState> {
    global::<ShortcutRebindState>()
}

pub fn request_shortcut_rebind() {
    shortcut_rebind_signal().update(|s| s.rev = s.rev.wrapping_add(1));
    set_timeout(Duration::from_millis(0), || {
        shortcut_rebind_signal().update(|s| s.rev = s.rev.wrapping_add(1));
    });
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct TextInputFocusState {
    pub focused: bool,
}

/// 独立追踪原生单行 `<input>`（筛选框、查找/替换框、转到行/文件框）的聚焦状态。
/// 用于动态切换应用菜单「编辑 > 复制」为 `MenuRole::Copy`（不劫持 `<input>`）或焦点感知自定义复制（支持树与 Editor 菜单点击）。
pub fn text_input_focus_signal() -> Signal<TextInputFocusState> {
    global::<TextInputFocusState>()
}

pub fn set_text_input_focused(focused: bool) {
    let changed = text_input_focus_signal().with_untracked(|s| s.focused != focused);
    if changed {
        text_input_focus_signal().set(TextInputFocusState { focused });
    }
}

/// 若工作区 Diff 对应的同文件源码标签处于脏态且与 Diff 同源，则将源码脏编辑同步回工作区 Diff，防止双缓冲分裂。
pub fn sync_dirty_code_tab_into_working_diff(s: &mut AppState) {
    let ws_mode = WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode);
    let Some(w) = &mut s.working_diff else {
        return;
    };
    if !w.data.editable || w.data.staged_only {
        return;
    }
    if let Some(tab) = s.code_tabs.iter().find(|t| {
        t.revision.is_empty()
            && t.file.repo_id == w.file.repo_id
            && t.file.rel_path == w.file.path
    }) {
        if tab.is_dirty()
            && tab.version.sha256 == w.data.version.sha256
            && w.current_new_text != tab.text
        {
            w.current_new_text = tab.text.clone();
            w.computed = compute_diff(&w.data.old_text, &w.current_new_text, ws_mode);
            w.content_rev += 1;
        }
    }
}

/// 若工作区 Diff 处于脏态且与目标源码标签同源，则将 Diff 脏编辑同步到源码标签，防止双缓冲分裂。
pub fn sync_dirty_working_diff_into_code_tab(s: &mut AppState, tab_idx: usize) {
    let Some(w) = s.working_diff.as_ref() else {
        return;
    };
    if !w.is_dirty() {
        return;
    }
    let w_repo = w.file.repo_id.clone();
    let w_path = w.file.path.clone();
    let w_sha = w.data.version.sha256.clone();
    let w_text = w.current_new_text.clone();
    if let Some(tab) = s.code_tabs.get_mut(tab_idx) {
        if tab.revision.is_empty()
            && tab.file.repo_id == w_repo
            && tab.file.rel_path == w_path
            && tab.version.sha256 == w_sha
            && tab.text != w_text
        {
            s.code_undo.remove(&tab.id);
            tab.text = w_text;
            tab.content_rev += 1;
        }
    }
}

#[derive(Clone)]
pub struct ThemeState {
    pub name: String,
}

impl Default for ThemeState {
    fn default() -> Self {
        Self {
            name: config::load().theme,
        }
    }
}

pub fn theme_signal() -> Signal<ThemeState> {
    global::<ThemeState>()
}

pub fn set_theme_name(next: String) {
    app_signal().update(|s| s.cfg.theme = next.clone());
    theme_signal().set(ThemeState { name: next });
    save_config_quiet();
}

pub fn current_palette() -> Palette {
    let theme_name = theme_signal().with(|t| t.name.clone());
    theme::palette_for(&theme_name, dark())
}

pub fn show_toast(kind: &'static str, message: impl Into<String>) {
    let msg = message.into();
    let mut id_out = 0u64;
    app_signal().update(|s| {
        s.next_toast_id += 1;
        id_out = s.next_toast_id;
        s.toast = Some(ToastItem {
            id: id_out,
            kind,
            message: msg,
        });
    });
    set_timeout(
        Duration::from_secs(4),
        move || {
            app_signal().update(|s| {
                if s.toast.as_ref().map(|t| t.id) == Some(id_out) {
                    s.toast = None;
                }
            });
        },
    );
}

pub fn save_config_quiet() {
    let cfg = app_signal().with_untracked(|s| s.cfg.clone());
    if let Err(err) = config::save(&cfg) {
        log::warn!("保存配置失败: {err}");
    }
}

/// 初始化工作区并启动文件监听与首次加载。
pub fn init_workspace() {
    let holder = watcher_holder();
    refresh_workspace_async(None);

    // 仅在 workspace_path 变化时重启文件监听，避免 repos 更新取消接收任务！
    let ws_memo = computed(|| app_signal().with(|s| s.cfg.workspace_path.clone()));
    effect(move || {
        let ws = ws_memo.get();
        if ws.trim().is_empty() {
            return;
        }
        let repos = app_signal().with_untracked(|s| s.repos.clone());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WatchHint>();
        let watcher = WorkspaceWatcher::start(PathBuf::from(&ws), repos, move |hint| {
            let _ = tx.send(hint);
        });
        match watcher {
            Ok(w) => {
                holder.with(|h| {
                    h.workspace = ws.clone();
                    h.watcher = Some(w);
                });
                spawn(async move {
                    while let Some(hint) = rx.recv().await {
                        handle_watch_hint(hint);
                    }
                });
            }
            Err(err) => {
                log::warn!("启动文件监听失败: {err}");
            }
        }
    });

    // 当 repos 更新时热更新 watcher 内部的仓库列表
    effect(move || {
        let repos = app_signal().with(|s| s.repos.clone());
        holder.with(|h| {
            if let Some(w) = &h.watcher {
                w.update_repos(repos);
            }
        });
    });
}

fn handle_watch_hint(hint: WatchHint) {
    if hint.is_empty() {
        return;
    }
    let is_busy = app_signal().with_untracked(|s| s.command_busy);
    if is_busy {
        app_signal().update(|s| {
            s.pending_refresh = true;
            if hint.mani_changed || hint.repo_ids.is_empty() {
                s.pending_refresh_repos.clear();
            } else {
                s.pending_refresh_repos.extend(hint.repo_ids);
            }
        });
        return;
    }
    if hint.mani_changed || hint.repo_ids.is_empty() {
        refresh_workspace_async(None);
    } else {
        refresh_workspace_async(Some(hint.repo_ids));
    }
}

/// 同步刷新工作区（主要用于 Headless 测试快速准备或同步调用）。
pub fn refresh_workspace_sync(only_repos: Option<HashSet<String>>) {
    let (ws, hidden, include_root, generation, existing_repos) = app_signal().with_untracked(|s| {
        (
            s.cfg.workspace_path.clone(),
            s.cfg.hidden_repos.clone(),
            s.cfg.extra_bool("include_root_repo",true),
            s.refresh_gen + 1,
            s.repos.clone(),
        )
    });
    if ws.trim().is_empty() {
        return;
    }
    app_signal().update(|s| {
        s.refresh_gen = generation;
        s.is_refreshing = true;
    });

    let result = compute_workspace_snapshot(ws, hidden, include_root, only_repos, existing_repos);
    apply_workspace_snapshot(generation, result);
}

struct RefreshSnapshot {
    repos: Vec<RepoInfo>,
    statuses: Vec<RepoStatus>,
    partial_repo_ids: Option<HashSet<String>>,
    workspace_files: Vec<WorkspaceFile>,
    commits: Vec<CommitEntry>,
    stashes: Vec<StashEntry>,
    elapsed_ms: u64,
}

fn compute_workspace_snapshot(
    ws: String,
    hidden: Vec<String>,
    include_root: bool,
    only_repos: Option<HashSet<String>>,
    existing_repos: Vec<RepoInfo>,
) -> RefreshSnapshot {
    let start = Instant::now();
    let repos = if only_repos.is_some() && !existing_repos.is_empty() {
        existing_repos
    } else {
        let mut repos = git::discover_workspace_repos(&ws, &hidden);
        if !include_root { repos.retain(|repo| !repo.is_root); }
        repos
    };

    let target_repos: Vec<&RepoInfo> = match &only_repos {
        Some(ids) if !ids.is_empty() => repos.iter().filter(|r| ids.contains(&r.id)).collect(),
        _ => repos.iter().collect(),
    };

    let statuses: Vec<RepoStatus> = target_repos.iter().map(|r| git::inspect_repo(r)).collect();
    let workspace_files = git::list_workspace_files(&repos);

    let mut commits = Vec::new();
    for repo in &repos {
        if let Ok(mut list) = git::repo_log(repo, 40) {
            commits.append(&mut list);
        }
    }
    commits.sort_by(|a, b| b.date.cmp(&a.date));
    if commits.len() > 120 {
        commits.truncate(120);
    }

    let stashes = git::list_stashes(&repos);

    RefreshSnapshot {
        repos,
        statuses,
        partial_repo_ids: only_repos,
        workspace_files,
        commits,
        stashes,
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

/// 异步刷新工作区（后台线程跑 git，代次防过期覆盖，绝不覆盖脏缓冲）。
pub fn refresh_workspace_async(only_repos: Option<HashSet<String>>) {
    let (ws, hidden, include_root, generation, existing_repos, busy) = app_signal().with_untracked(|s| {
        (
            s.cfg.workspace_path.clone(),
            s.cfg.hidden_repos.clone(),
            s.cfg.extra_bool("include_root_repo",true),
            s.refresh_gen + 1,
            s.repos.clone(),
            s.command_busy,
        )
    });
    if ws.trim().is_empty() {
        return;
    }
    if busy {
        app_signal().update(|s| {
            s.pending_refresh = true;
            if let Some(ids) = only_repos {
                s.pending_refresh_repos.extend(ids);
            }
        });
        return;
    }

    app_signal().update(|s| {
        s.refresh_gen = generation;
        s.is_refreshing = true;
    });

    spawn(async move {
        let res = blocking(move || {
            compute_workspace_snapshot(ws, hidden, include_root, only_repos, existing_repos)
        })
        .await;
        if let Ok(snapshot) = res {
            apply_workspace_snapshot(generation, snapshot);
        } else {
            app_signal().update(|s| s.is_refreshing = false);
        }
    });
}

fn apply_workspace_snapshot(generation: u64, snap: RefreshSnapshot) {
    // 丢弃过期代次刷新结果
    let stale = app_signal().with_untracked(|s| generation < s.refresh_gen);
    if stale {
        return;
    }

    let complete = snap.partial_repo_ids.is_none();
    app_signal().update(|s| {
        s.is_refreshing = false;
        s.last_refresh_ms = snap.elapsed_ms;
        for repo in &snap.repos {
            if !s.repos.iter().any(|previous| previous.path == repo.path) {
                s.expanded_project_repos.insert(repo.id.clone());
            }
        }
        s.expanded_project_repos
            .retain(|id| snap.repos.iter().any(|repo| &repo.id == id));
        s.repos = snap.repos;
        if let Some(ref ids) = snap.partial_repo_ids {
            let mut by_id: HashMap<String, RepoStatus> = snap
                .statuses
                .into_iter()
                .map(|st| (st.repo.id.clone(), st))
                .collect();
            for st in &mut s.statuses {
                if ids.contains(&st.repo.id) {
                    if let Some(updated) = by_id.remove(&st.repo.id) {
                        *st = updated;
                    }
                }
            }
            for (_, rem) in by_id {
                s.statuses.push(rem);
            }
        } else {
            s.statuses = snap.statuses;
        }
        s.workspace_files = snap.workspace_files;
        s.git_commits = snap.commits;
        s.stashes = snap.stashes;

        // 同步所有当前存在的变更文件 key：
        // 对齐基线 state.go:1261,1439 —— 新发现的变更默认勾选，用户手动取消勾选的保留未勾选状态，已消失的变更清理。
        let all_change_map: HashMap<String, ChangedFile> = s
            .statuses
            .iter()
            .flat_map(|st| st.files.iter())
            .map(|f| (f.key(), f.clone()))
            .collect();
        for key in all_change_map.keys() {
            if !s.known_change_keys.contains(key) {
                s.known_change_keys.insert(key.clone());
                s.checked_changes.insert(key.clone());
            }
        }
        s.known_change_keys.retain(|k| all_change_map.contains_key(k));
        s.checked_changes.retain(|k| all_change_map.contains_key(k));
        s.included_hunks.retain(|k, _| all_change_map.contains_key(k));

        // 更新当前选中行（保持原路径）
        if let Some(sel) = &mut s.change_selection {
            if let Some(latest) = all_change_map.get(&sel.file.key()) {
                sel.file = latest.clone();
            } else {
                s.change_selection = None;
            }
        }

        // 刷新所有已打开的源码标签（干净标签自动更新，脏标签绝不覆盖但标记 external_changed）
        for tab in &mut s.code_tabs {
            if !tab.revision.is_empty() {
                continue;
            }
            let wt = git::read_review_worktree(Path::new(&tab.file.abs_path));
            if wt.version.sha256 != tab.version.sha256 || wt.version.exists != tab.version.exists {
                if tab.is_dirty() {
                    tab.external_changed = true;
                } else {
                    s.code_undo.remove(&tab.id);
                    tab.text = wt.text.clone();
                    tab.saved_text = wt.text;
                    tab.line_ending = wt.line_ending;
                    tab.version = wt.version;
                    tab.editable = wt.editable;
                    tab.readonly_reason = wt.readonly_reason;
                    tab.binary = wt.binary;
                    tab.too_large = wt.too_large;
                    tab.external_changed = false;
                    tab.content_rev += 1;
                }
            }
        }

        // 刷新当前打开的工作区 Diff（干净则自动重载最新差异，脏则绝不覆盖新侧内容）
        let ws_mode = WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode);
        if let Some(wd) = &mut s.working_diff {
            if let Some(latest_cf) = all_change_map.get(&wd.file.key()) {
                wd.file = latest_cf.clone();
            }
            let fresh = git::read_review_change(&wd.file, wd.data.staged_only);
            let old_changed = fresh.old_text != wd.data.old_text
                || fresh.head_snapshot != wd.data.head_snapshot
                || fresh.index_snapshot != wd.data.index_snapshot;
            let disk_changed = fresh.version.sha256 != wd.data.version.sha256
                || fresh.version.exists != wd.data.version.exists
                || fresh.new_text != wd.data.new_text;

            if wd.is_dirty() {
                if disk_changed {
                    wd.external_changed = true;
                }
                if old_changed {
                    wd.data.old_text = fresh.old_text;
                    wd.data.index_snapshot = fresh.index_snapshot;
                    wd.data.head_snapshot = fresh.head_snapshot;
                    wd.computed = compute_diff(&wd.data.old_text, &wd.current_new_text, ws_mode);
                    wd.content_rev += 1;
                }
            } else if old_changed || disk_changed {
                let new_text = fresh.new_text.clone();
                wd.computed = compute_diff(&fresh.old_text, &new_text, ws_mode);
                wd.data = fresh;
                wd.current_new_text = new_text.clone();
                wd.saved_new_text = new_text;
                wd.external_changed = false;
                wd.content_rev += 1;
            }
        }

        // 选中默认提交（若 Git 面板打开且尚无选中）
        if s.selected_commit.is_none() {
            if let Some(first) = s.git_commits.first().cloned() {
                let files = git::commit_files(&first.repo_path, &first.hash).unwrap_or_default();
                s.selected_commit_file = files.first().cloned();
                s.selected_commit_files = files;
                s.selected_commit = Some(first);
            }
        }
    });
    retry_file_navigation();
    if complete {
        crate::session_runtime::restore_after_refresh();
    }
    if std::env::var_os("NEMOROUS_TRACE_TIMING").is_some() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let (repo_cnt, file_cnt, diff_rev) = app_signal().with_untracked(|s| {
            let files: usize = s.statuses.iter().map(|st| st.files.len()).sum();
            (
                s.repos.len(),
                files,
                s.working_diff.as_ref().map(|w| w.content_rev).unwrap_or(0),
            )
        });
        eprintln!(
            "NEMOROUS_TIMING event=commit_ready epoch_ms={now_ms} repos={repo_cnt} changes={file_cnt} diff_rev={diff_rev}"
        );
    }
}

// ============================================================================
// Commit 树可见列表、勾选与选择逻辑
// ============================================================================

#[derive(Default)]
struct VisibleDirNode {
    rel_path: String,
    subdirs: std::collections::BTreeMap<String, VisibleDirNode>,
    files: Vec<ChangedFile>,
}

fn push_rendered_change_files(
    files: &[ChangedFile],
    flat: bool,
    section_repo_prefix: &str,
    collapsed_dirs: &HashSet<String>,
    staged: bool,
    out: &mut Vec<ChangeSelection>,
) {
    if flat {
        let mut sorted = files.to_vec();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        for f in sorted {
            out.push(ChangeSelection { file: f, staged });
        }
        return;
    }

    let mut root = VisibleDirNode::default();
    for f in files {
        let parts: Vec<&str> = f.path.split('/').collect();
        if parts.len() <= 1 {
            root.files.push(f.clone());
            continue;
        }
        let mut cur = &mut root;
        let mut acc = String::new();
        for seg in &parts[..parts.len() - 1] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            cur = cur
                .subdirs
                .entry((*seg).to_string())
                .or_insert_with(|| VisibleDirNode {
                    rel_path: acc.clone(),
                    subdirs: std::collections::BTreeMap::new(),
                    files: Vec::new(),
                });
        }
        cur.files.push(f.clone());
    }

    fn walk(
        node: &mut VisibleDirNode,
        prefix: &str,
        collapsed_dirs: &HashSet<String>,
        staged: bool,
        out: &mut Vec<ChangeSelection>,
    ) {
        node.files.sort_by(|a, b| a.path.cmp(&b.path));
        for sub in node.subdirs.values_mut() {
            let dir_key = format!("{prefix}::{}", sub.rel_path);
            if !collapsed_dirs.contains(&dir_key) {
                walk(sub, prefix, collapsed_dirs, staged, out);
            }
        }
        for f in &node.files {
            out.push(ChangeSelection {
                file: f.clone(),
                staged,
            });
        }
    }

    walk(&mut root, section_repo_prefix, collapsed_dirs, staged, out);
}

pub fn visible_change_items(s: &AppState) -> Vec<ChangeSelection> {
    let q = s.changes_filter.trim().to_ascii_lowercase();
    let mut out = Vec::new();
    let matches_query = |f: &ChangedFile| {
        s.cfg.matches_change_list(&f.key()) && (q.is_empty()
            || f.path.to_ascii_lowercase().contains(&q)
            || f.repo_name.to_ascii_lowercase().contains(&q))
    };

    if !s.collapsed_staged_section {
        for st in &s.statuses {
            let repo_key = format!("staged::{}", st.repo.id);
            if s.collapsed_change_repos.contains(&repo_key) {
                continue;
            }
            let files: Vec<ChangedFile> = st
                .files
                .iter()
                .filter(|f| f.staged && matches_query(f))
                .cloned()
                .collect();
            push_rendered_change_files(
                &files,
                s.cfg.changes_flat,
                &repo_key,
                &s.collapsed_change_dirs,
                true,
                &mut out,
            );
        }
    }

    if !s.collapsed_unstaged_section {
        for st in &s.statuses {
            let repo_key = format!("unstaged::{}", st.repo.id);
            if s.collapsed_change_repos.contains(&repo_key) {
                continue;
            }
            let files: Vec<ChangedFile> = st
                .files
                .iter()
                .filter(|f| (f.unstaged || f.untracked || !f.staged) && matches_query(f))
                .cloned()
                .collect();
            push_rendered_change_files(
                &files,
                s.cfg.changes_flat,
                &repo_key,
                &s.collapsed_change_dirs,
                false,
                &mut out,
            );
        }
    }
    out
}

/// 三态勾选状态：None=全未选，Some(false)=半选，Some(true)=全选。
pub fn tri_state_for_keys<'a>(
    keys: impl IntoIterator<Item = &'a str>,
    checked: &HashSet<String>,
) -> Option<bool> {
    let mut total = 0usize;
    let mut hit = 0usize;
    for k in keys {
        total += 1;
        if checked.contains(k) {
            hit += 1;
        }
    }
    if total == 0 || hit == 0 {
        None
    } else if hit == total {
        Some(true)
    } else {
        Some(false)
    }
}

pub fn toggle_change_checked_in_section(key: &str, staged: bool, shift_held: bool) {
    app_signal().update(|s| {
        if shift_held {
            if let Some(last_key) = s.last_checked_key.clone() {
                let last_staged = s.last_checked_staged.unwrap_or(staged);
                let ordered: Vec<(bool, String)> = visible_change_items(s)
                    .into_iter()
                    .map(|it| (it.staged, it.file.key()))
                    .collect();
                let from = ordered
                    .iter()
                    .position(|(st, k)| *st == last_staged && k == &last_key)
                    .or_else(|| ordered.iter().position(|(_, k)| k == &last_key));
                let to = ordered
                    .iter()
                    .position(|(st, k)| *st == staged && k == key)
                    .or_else(|| ordered.iter().position(|(_, k)| k == key));
                if let (Some(a), Some(b)) = (from, to) {
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    let target_val = !s.checked_changes.contains(key);
                    for (_, k) in &ordered[lo..=hi] {
                        s.included_hunks.remove(k);
                        if target_val {
                            s.checked_changes.insert(k.clone());
                        } else {
                            s.checked_changes.remove(k);
                        }
                    }
                    s.last_checked_key = Some(key.to_string());
                    s.last_checked_staged = Some(staged);
                    return;
                }
            }
        }
        s.included_hunks.remove(key);
        if s.checked_changes.contains(key) {
            s.checked_changes.remove(key);
        } else {
            s.checked_changes.insert(key.to_string());
        }
        s.last_checked_key = Some(key.to_string());
        s.last_checked_staged = Some(staged);
    });
}

pub fn toggle_change_checked(key: &str, shift_held: bool) {
    let staged = app_signal().with_untracked(|s| {
        s.change_selection
            .as_ref()
            .filter(|sel| sel.file.key() == key)
            .map(|sel| sel.staged)
            .unwrap_or(false)
    });
    toggle_change_checked_in_section(key, staged, shift_held);
}

pub fn set_group_checked(keys: Vec<String>, check: bool) {
    app_signal().update(|s| {
        for k in keys {
            s.included_hunks.remove(&k);
            if check {
                s.checked_changes.insert(k);
            } else {
                s.checked_changes.remove(&k);
            }
        }
    });
}

pub fn toggle_selected_change_checkbox() {
    let pair = app_signal().with_untracked(|s| {
        s.change_selection
            .as_ref()
            .map(|sel| (sel.file.key(), sel.staged))
            .or_else(|| {
                s.working_diff
                    .as_ref()
                    .map(|w| (w.file.key(), w.data.staged_only))
            })
    });
    if let Some((k, st)) = pair {
        toggle_change_checked_in_section(&k, st, false);
    }
}

/// 单击 Commit 树文件：改选中行、聚焦树，仅在开启「单击时预览差异」时才立即打开 Diff。
pub fn click_change_row(file: ChangedFile, staged: bool, clicks: u32) {
    set_text_input_focused(false);
    let preview = app_signal().with_untracked(|s| s.cfg.change_diff_preview);
    app_signal().update(|s| {
        s.change_selection = Some(ChangeSelection {
            file: file.clone(),
            staged,
        });
        s.change_selected_dir = None;
        s.focus_zone = FocusZone::CommitTree;
    });
    if clicks >= 2 {
        open_working_diff(file, staged, true);
    } else if preview {
        open_working_diff(file, staged, false);
    }
}

/// 移动 Commit 树选中行（`↑`/`↓`）或切换 Diff 邻文件（`⌥←`/`⌥→`）。
pub fn move_change_selection(delta: i32, open_diff_directly: bool) -> bool {
    move_change_selection_with_hunk(delta, open_diff_directly, false)
}

fn move_change_selection_with_hunk(
    delta: i32,
    open_diff_directly: bool,
    land_on_last_hunk: bool,
) -> bool {
    let (items, cur_key, cur_staged, preview) = app_signal().with_untracked(|s| {
        let items = visible_change_items(s);
        let (k, st) = if open_diff_directly {
            if let Some(wd) = &s.working_diff {
                (Some(wd.file.key()), wd.data.staged_only)
            } else if let Some(sel) = &s.change_selection {
                (Some(sel.file.key()), sel.staged)
            } else {
                (None, false)
            }
        } else if let Some(sel) = &s.change_selection {
            (Some(sel.file.key()), sel.staged)
        } else {
            (None, false)
        };
        (items, k, st, s.cfg.change_diff_preview)
    });

    if items.is_empty() {
        return false;
    }
    let idx = cur_key
        .as_ref()
        .and_then(|k| {
            items
                .iter()
                .position(|it| it.staged == cur_staged && &it.file.key() == k)
        })
        .map(|i| i as i32)
        .unwrap_or(-1);

    let next = if open_diff_directly {
        let cand = if idx < 0 {
            if delta < 0 { items.len() as i32 - 1 } else { 0 }
        } else {
            idx + delta
        };
        if cand < 0 || cand >= items.len() as i32 {
            return false;
        }
        cand as usize
    } else {
        let len = items.len() as i32;
        if idx < 0 {
            if delta < 0 { (len - 1) as usize } else { 0 }
        } else {
            (((idx + delta) % len) + len) as usize % items.len()
        }
    };

    let target = items[next].clone();
    app_signal().update(|s| {
        s.change_selection = Some(target.clone());
    });
    if open_diff_directly {
        open_working_diff_with_hunk(target.file, target.staged, true, land_on_last_hunk);
    } else if preview {
        open_working_diff_with_hunk(target.file, target.staged, false, land_on_last_hunk);
    }
    true
}

/// 打开工作区单个 Diff（切文件前按旧版规则自动受检保存前一个脏工作区 Diff）。
pub fn open_working_diff(file: ChangedFile, staged_only: bool, focus_diff: bool) {
    open_working_diff_with_hunk(file, staged_only, focus_diff, false);
}

fn open_working_diff_with_hunk(
    file: ChangedFile,
    staged_only: bool,
    focus_diff: bool,
    land_on_last_hunk: bool,
) {
    crate::code_split_runtime::capture_views();
    // 若当前已有脏工作区 Diff 且目标不同，先自动保存
    let need_autosave = app_signal().with_untracked(|s| {
        s.working_diff.as_ref().is_some_and(|w| {
            w.is_dirty() && (w.file.key() != file.key() || w.data.staged_only != staged_only)
        })
    });
    if need_autosave {
        if let Some(target) = crate::format_runtime::working_target() {
            let next = file.clone();
            if crate::format_runtime::save_if_enabled(vec![target], move || {
                open_working_diff_with_hunk(next, staged_only, focus_diff, land_on_last_hunk);
            }) {
                return;
            }
        }
        if !save_working_diff_now() {
            // 发生外部覆盖冲突弹窗或拒绝保存时，阻止切换保护用户编辑
            return;
        }
    }

    push_nav_back_current();

    let ws_mode =
        app_signal().with_untracked(|s| WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode));
    let data = git::read_review_change(&file, staged_only);
    let saved_new_text = data.new_text.clone();
    // 双向同步：若同文件已有未保存的同源 CodeTab（或同文件脏 WorkingDiff），保留未保存缓冲避免分裂与丢失
    let carried_text = if !staged_only && data.editable {
        app_signal().with_untracked(|s| {
            s.code_tabs
                .iter()
                .find(|t| {
                    t.revision.is_empty()
                        && t.file.repo_id == file.repo_id
                        && t.file.rel_path == file.path
                        && t.is_dirty()
                        && t.version.sha256 == data.version.sha256
                })
                .map(|t| t.text.clone())
                .or_else(|| {
                    s.working_diff
                        .as_ref()
                        .filter(|w| {
                            !w.data.staged_only
                                && w.file.key() == file.key()
                                && w.is_dirty()
                                && w.data.version.sha256 == data.version.sha256
                        })
                        .map(|w| w.current_new_text.clone())
                })
        })
    } else {
        None
    };
    let current_new_text = carried_text.unwrap_or_else(|| saved_new_text.clone());
    let computed = compute_diff(&data.old_text, &current_new_text, ws_mode);
    let initial_hunk_idx = if land_on_last_hunk {
        computed.hunks.len().saturating_sub(1)
    } else {
        0
    };
    let initial_old_line = computed
        .hunks
        .get(initial_hunk_idx)
        .map(|h| h.old_start as u32)
        .unwrap_or(0);
    let initial_hunk_line = computed
        .hunks
        .get(initial_hunk_idx)
        .map(|h| h.new_start as u32)
        .unwrap_or(0);
    let has_hunks = !computed.hunks.is_empty();

    app_signal().update(|s| {
        s.change_selection = Some(ChangeSelection {
            file: file.clone(),
            staged: staged_only,
        });
        let next_rev = s.working_diff.as_ref().map(|w| w.content_rev + 1).unwrap_or(1);
        s.working_diff = Some(WorkingDiffSession {
            file: file.clone(),
            data,
            current_new_text,
            saved_new_text,
            computed,
            active_side: DiffSide::New,
            old_cursor: EditorPosition::new(initial_old_line, 0),
            new_cursor: EditorPosition::new(initial_hunk_line, 0),
            old_scroll_y: 0.0,
            new_scroll_y: 0.0,
            current_hunk: initial_hunk_idx,
            boundary_hint: None,
            external_changed: false,
            content_rev: next_rev,
        });
        s.editor_tab = EditorTabKind::WorkingDiff;
        if focus_diff {
            s.focus_zone = FocusZone::WorkingDiffNew;
        }
    });

    record_recent_location(NavTarget::WorkingDiff {
        file_key: file.key(),
        staged: staged_only,
        side: DiffSide::New,
        pos: EditorPosition::new(initial_hunk_line, 0),
        scroll_y: 0,
    });

    if focus_diff {
        focus_active_editor_ref();
    }
    if has_hunks && land_on_last_hunk {
        scroll_to_working_hunk(initial_hunk_idx);
        set_timeout(Duration::from_millis(16), move || {
            scroll_to_working_hunk(initial_hunk_idx);
        });
    }
}

/// 切换工作区 Diff 的暂存/未暂存 scope。
pub fn switch_working_diff_scope(staged_only: bool) {
    let file = app_signal().with_untracked(|s| s.working_diff.as_ref().map(|w| w.file.clone()));
    if let Some(cf) = file {
        open_working_diff(cf, staged_only, true);
    }
}

/// 打开历史提交的单个只读 Diff。
pub fn open_commit_diff(commit: CommitEntry, file: CommitFileEntry, focus_diff: bool) {
    open_commit_diff_with_hunk(commit, file, focus_diff, false);
}

fn open_commit_diff_with_hunk(
    commit: CommitEntry,
    file: CommitFileEntry,
    focus_diff: bool,
    land_on_last_hunk: bool,
) {
    let ws_mode = app_signal().with_untracked(|s| WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode));
    match git::read_commit_diff(&commit, &file) {
        Ok(data) => {
            let computed = compute_diff(&data.old_text, &data.new_text, ws_mode);
            install_commit_diff(commit, file, data, computed, focus_diff, land_on_last_hunk);
        }
        Err(err) => show_toast("err", format!("读取提交差异失败: {err}")),
    }
}

#[derive(Clone, Default)]
struct CommitPreviewRequest(u64);

pub(crate) fn preview_commit_diff(commit: CommitEntry, file: CommitFileEntry) {
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else { return };
    let (workspace, origin, whitespace) = app_signal().with_untracked(|s| {
        (s.cfg.workspace_path.clone(), s.editor_tab, WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode))
    });
    let request = global::<CommitPreviewRequest>().with_untracked(|s| s.0.wrapping_add(1));
    global::<CommitPreviewRequest>().set(CommitPreviewRequest(request));
    let (source, item) = (commit.clone(), file.clone());
    owner.run(|| spawn(async move {
        let result = blocking(move || -> anyhow::Result<_> {
            let data = git::read_commit_diff(&source, &item)?;
            let computed = compute_diff(&data.old_text, &data.new_text, whitespace);
            Ok((data, computed))
        }).await;
        if !global::<CommitPreviewRequest>().with_untracked(|s| s.0 == request)
            || !app_signal().with_untracked(|s| {
                s.editor_tab == origin && s.cfg.workspace_path == workspace && s.show_git_panel
                    && s.cfg.git_log_diff_preview && s.selected_commit_file.as_ref() == Some(&file)
                    && s.selected_commit.as_ref().is_some_and(|selected|
                        selected.hash == commit.hash && selected.repo_id == commit.repo_id)
            })
        {
            return;
        }
        match result {
            Ok(Ok((data, computed))) => install_commit_diff(commit, file, data, computed, false, false),
            Ok(Err(err)) => show_toast("warn", format!("读取差异预览失败：{err}")),
            Err(err) => show_toast("warn", format!("读取差异预览失败：{err}")),
        }
    }));
}

fn install_commit_diff(
    commit: CommitEntry,
    file: CommitFileEntry,
    data: CommitDiffData,
    computed: ComputedDiff,
    focus_diff: bool,
    land_on_last_hunk: bool,
) {
    crate::code_split_runtime::capture_views();
    push_nav_back_current();
    let initial_hunk_idx = if land_on_last_hunk {
        computed.hunks.len().saturating_sub(1)
    } else {
        0
    };
    let initial_old_line = computed
        .hunks
        .get(initial_hunk_idx)
        .map(|h| h.old_start as u32)
        .unwrap_or(0);
    let initial_line = computed
        .hunks
        .get(initial_hunk_idx)
        .map(|h| h.new_start as u32)
        .unwrap_or(0);
    let has_hunks = !computed.hunks.is_empty();
    let default_side = if data.status == "D" {
        DiffSide::Old
    } else {
        DiffSide::New
    };

    app_signal().update(|s| {
        s.selected_commit = Some(commit.clone());
        s.selected_commit_file = Some(file.clone());
        let next_rev = s.commit_diff.as_ref().map(|c| c.content_rev + 1).unwrap_or(1);
        s.commit_diff = Some(CommitDiffSession {
            commit: commit.clone(),
            file: file.clone(),
            data,
            computed,
            active_side: default_side,
            old_cursor: EditorPosition::new(initial_old_line, 0),
            new_cursor: EditorPosition::new(initial_line, 0),
            old_scroll_y: 0.0,
            new_scroll_y: 0.0,
            current_hunk: initial_hunk_idx,
            boundary_hint: None,
            content_rev: next_rev,
        });
        s.editor_tab = EditorTabKind::CommitDiff;
        if focus_diff {
            s.focus_zone = match default_side {
                DiffSide::Old => FocusZone::CommitDiffOld,
                DiffSide::New => FocusZone::CommitDiffNew,
            };
        }
    });

    record_recent_location(NavTarget::CommitDiff {
        repo_id: commit.repo_id.clone(),
        commit_hash: commit.hash.clone(),
        file_path: file.path.clone(),
        side: default_side,
        pos: EditorPosition::new(initial_line, 0),
        scroll_y: 0,
    });

    if focus_diff {
        focus_active_editor_ref();
    }
    if has_hunks && land_on_last_hunk {
        scroll_to_commit_hunk(initial_hunk_idx);
        set_timeout(Duration::from_millis(16), move || {
            scroll_to_commit_hunk(initial_hunk_idx);
        });
    }
}

/// 在历史提交 Diff 中切换上/下一个文件。
pub fn move_commit_diff_file(delta: i32) -> bool {
    move_commit_diff_file_with_hunk(delta, false)
}

fn move_commit_diff_file_with_hunk(delta: i32, land_on_last_hunk: bool) -> bool {
    let (commit, files, cur_path) = app_signal().with_untracked(|s| {
        let cur_path = s
            .commit_diff
            .as_ref()
            .map(|c| c.file.path.clone())
            .or_else(|| s.selected_commit_file.as_ref().map(|f| f.path.clone()));
        (
            s.selected_commit.clone(),
            s.selected_commit_files.clone(),
            cur_path,
        )
    });
    let (Some(commit), Some(cur_path)) = (commit, cur_path) else {
        return false;
    };
    if files.is_empty() {
        return false;
    }
    let Some(idx) = files.iter().position(|f| f.path == cur_path) else {
        return false;
    };
    let next = idx as i32 + delta;
    if next < 0 || next >= files.len() as i32 {
        return false;
    }
    open_commit_diff_with_hunk(commit, files[next as usize].clone(), true, land_on_last_hunk);
    true
}

// ============================================================================
// 源码标签打开、跳转源码、往返与关闭保护
// ============================================================================

pub fn open_code_file(file: WorkspaceFile, target_pos: Option<EditorPosition>, focus_editor: bool) {
    open_code_file_mode(file, target_pos, focus_editor, false)
}

/// 以预览方式打开：未钉住的预览标签为斜体，可被下一次预览复用替换；
/// 已钉住（或脏）的标签一律保留，不会丢缓冲。
pub fn open_code_file_preview(file: WorkspaceFile) {
    open_code_file_mode(file, None, true, true)
}

fn open_code_file_mode(
    file: WorkspaceFile,
    target_pos: Option<EditorPosition>,
    focus_editor: bool,
    preview: bool,
) {
    crate::code_split_runtime::capture_views();
    push_nav_back_current();
    let tab_id = format!("file::{}::{}", file.repo_id, file.rel_path);

    let existing_idx = app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .position(|t| t.id == tab_id && t.revision.is_empty())
    });

    if let Some(idx) = existing_idx {
        app_signal().update(|s| {
            crate::code_split_runtime::select_tab_in_model(s, idx);
            sync_dirty_working_diff_into_code_tab(s, idx);
            s.editor_tab = EditorTabKind::File;
            if !preview {
                s.code_tabs[idx].preview = false;
            }
            if let Some(pos) = target_pos {
                crate::code_split_runtime::set_active_position(s, pos);
            }
            if focus_editor {
                s.focus_zone = FocusZone::CodeEditor;
            }
        });
    } else {
        let wt = git::read_review_worktree(Path::new(&file.abs_path));
        let cursor = target_pos.unwrap_or_else(|| EditorPosition::new(0, 0));
        let tab = CodeTab {
            id: tab_id,
            file: file.clone(),
            revision: String::new(),
            text: wt.text.clone(),
            saved_text: wt.text,
            line_ending: wt.line_ending,
            version: wt.version,
            editable: wt.editable,
            readonly_reason: wt.readonly_reason,
            binary: wt.binary,
            too_large: wt.too_large,
            cursor,
            scroll_y: 0.0,
            scroll_x: 0.0,
            preview,
            external_changed: false,
            content_rev: 1,
        };
        app_signal().update(|s| {
            if preview {
                s.code_tabs.retain_mut(|tab| {
                    if tab.is_dirty() {
                        tab.preview = false;
                    }
                    !tab.preview
                });
            }
            s.code_tabs.push(tab);
            let idx = s.code_tabs.len() - 1;
            crate::code_split_runtime::select_tab_in_model(s, idx);
            if preview {
                crate::code_split_runtime::prune_closed_tabs(s);
            }
            sync_dirty_working_diff_into_code_tab(s, idx);
            s.editor_tab = EditorTabKind::File;
            if focus_editor {
                s.focus_zone = FocusZone::CodeEditor;
            }
        });
    }

    let pos = target_pos.unwrap_or_else(|| EditorPosition::new(0, 0));
    record_recent_location(NavTarget::CodeFile {
        repo_id: file.repo_id.clone(),
        rel_path: file.rel_path.clone(),
        pos,
        scroll_y: 0,
    });

    if let Some(p) = target_pos {
        reveal_in_code_editor(p);
    }
    if focus_editor {
        focus_active_editor_ref();
    }
}

/// 打开指定 Git 提交版本的只读源码标签（用于右键「打开仓库版本」）。
pub fn open_revision_code_file(commit: &CommitEntry, rel_path: &str) {
    crate::code_split_runtime::capture_views();
    let spec = format!("{}:{rel_path}", commit.hash);
    let (text, binary, too_large) =
        git::read_git_object_text(Path::new(&commit.repo_path), &spec).unwrap_or_default();
    let abs = Path::new(&commit.repo_path)
        .join(rel_path)
        .to_string_lossy()
        .to_string();
    let tab_id = format!("rev::{}::{}::{}", commit.repo_id, commit.hash, rel_path);

    app_signal().update(|s| {
        if let Some(idx) = s.code_tabs.iter().position(|t| t.id == tab_id) {
            crate::code_split_runtime::select_tab_in_model(s, idx);
            s.editor_tab = EditorTabKind::File;
            s.focus_zone = FocusZone::CodeEditor;
            return;
        }
        s.code_tabs.push(CodeTab {
            id: tab_id,
            file: WorkspaceFile {
                repo_id: commit.repo_id.clone(),
                repo_name: commit.repo_name.clone(),
                repo_path: commit.repo_path.clone(),
                rel_path: rel_path.to_string(),
                abs_path: abs,
                color_idx: 0,
            },
            revision: commit.hash.clone(),
            text: text.clone(),
            saved_text: text,
            line_ending: LineEnding::Lf,
            version: ReviewVersion::default(),
            editable: false,
            readonly_reason: Some(format!("仓库版本 {} 为只读视图", commit.short_hash)),
            binary,
            too_large,
            cursor: EditorPosition::new(0, 0),
            scroll_y: 0.0,
            scroll_x: 0.0,
            preview: false,
            external_changed: false,
            content_rev: 1,
        });
        crate::code_split_runtime::select_tab_in_model(s, s.code_tabs.len() - 1);
        s.editor_tab = EditorTabKind::File;
        s.focus_zone = FocusZone::CodeEditor;
    });
    focus_active_editor_ref();
}

/// 从当前 Diff（工作区或历史提交）按 `F4` / `Cmd+Down` 跳转到源码页并定位对应行。
/// 若工作区 Diff 存在未保存轻改，将脏缓冲内容无缝同步到目标源码标签，避免编辑丢失或双缓冲分裂。
pub fn jump_from_diff_to_source() {
    struct DirtyCarry {
        current_text: String,
        saved_text: String,
        version: ReviewVersion,
        line_ending: LineEnding,
    }

    let target = app_signal().with_untracked(|s| match s.editor_tab {
        EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|w| {
            let mapped = source_position(
                &w.current_new_text,
                &w.computed.hunks,
                w.active_side,
                w.active_cursor(),
            );
            let wf = WorkspaceFile {
                repo_id: w.file.repo_id.clone(),
                repo_name: w.file.repo_name.clone(),
                repo_path: w.file.repo_path.clone(),
                rel_path: w.file.path.clone(),
                abs_path: w.file.abs_path(),
                color_idx: s
                    .repos
                    .iter()
                    .find(|r| r.id == w.file.repo_id)
                    .map(|r| r.color_idx)
                    .unwrap_or(0),
            };
            let carry = if w.is_dirty() {
                Some(DirtyCarry {
                    current_text: w.current_new_text.clone(),
                    saved_text: w.saved_new_text.clone(),
                    version: w.data.version.clone(),
                    line_ending: w.data.line_ending,
                })
            } else {
                None
            };
            let missing = !Path::new(&wf.abs_path).exists();
            (wf, mapped, missing, carry)
        }),
        EditorTabKind::CommitDiff => s.commit_diff.as_ref().map(|c| {
            let mapped = source_position(
                &c.data.new_text,
                &c.computed.hunks,
                c.active_side,
                c.active_cursor(),
            );
            let abs = Path::new(&c.commit.repo_path)
                .join(&c.data.new_path)
                .to_string_lossy()
                .to_string();
            let wf = WorkspaceFile {
                repo_id: c.commit.repo_id.clone(),
                repo_name: c.commit.repo_name.clone(),
                repo_path: c.commit.repo_path.clone(),
                rel_path: c.data.new_path.clone(),
                abs_path: abs.clone(),
                color_idx: s
                    .repos
                    .iter()
                    .find(|r| r.id == c.commit.repo_id)
                    .map(|r| r.color_idx)
                    .unwrap_or(0),
            };
            (wf, mapped, !Path::new(&abs).exists(), None)
        }),
        _ => None,
    });

    let Some((wf, pos, missing, carry)) = target else {
        return;
    };
    if missing {
        show_toast("warn", "工作区中该文件已删除，无法打开源码");
        return;
    }
    open_code_file(wf.clone(), Some(pos), true);
    if let Some(dirty) = carry {
        let tab_id = format!("file::{}::{}", wf.repo_id, wf.rel_path);
        app_signal().update(|s| {
            if let Some(t) = s
                .code_tabs
                .iter_mut()
                .find(|t| t.id == tab_id && t.revision.is_empty())
            {
                if t.text != dirty.current_text {
                    s.code_undo.remove(&t.id);
                }
                t.text = dirty.current_text;
                t.saved_text = dirty.saved_text;
                t.version = dirty.version;
                t.line_ending = dirty.line_ending;
                t.content_rev += 1;
            }
        });
        reveal_in_code_editor(pos);
    }
}

pub fn can_return_to_review() -> bool {
    app_signal().with(|s| {
        s.nav_back.iter().rev().any(|t| {
            matches!(
                t,
                NavTarget::WorkingDiff { .. } | NavTarget::CommitDiff { .. }
            )
        })
    })
}

pub fn return_to_review() {
    let target = app_signal().with_untracked(|s| {
        s.nav_back
            .iter()
            .rposition(|t| {
                matches!(
                    t,
                    NavTarget::WorkingDiff { .. } | NavTarget::CommitDiff { .. }
                )
            })
            .map(|idx| (idx, s.nav_back[idx].clone()))
    });
    let Some((idx, nav)) = target else {
        return;
    };
    let cur = current_nav_target();
    app_signal().update(|s| {
        s.nav_back.truncate(idx);
        if let Some(c) = cur {
            s.nav_forward.push(c);
        }
    });
    restore_nav_target(nav);
}

pub fn navigate_back() {
    let target = app_signal().with_untracked(|s| s.nav_back.last().cloned());
    let Some(nav) = target else {
        return;
    };
    let cur = current_nav_target();
    app_signal().update(|s| {
        s.nav_back.pop();
        if let Some(c) = cur {
            s.nav_forward.push(c);
        }
    });
    restore_nav_target(nav);
}

pub fn navigate_forward() {
    let target = app_signal().with_untracked(|s| s.nav_forward.last().cloned());
    let Some(nav) = target else {
        return;
    };
    let cur = current_nav_target();
    app_signal().update(|s| {
        s.nav_forward.pop();
        if let Some(c) = cur {
            s.nav_back.push(c);
        }
    });
    restore_nav_target(nav);
}

fn current_nav_target() -> Option<NavTarget> {
    app_signal().with_untracked(|s| match s.editor_tab {
        EditorTabKind::File => s.code_tabs.get(s.active_code_tab_idx).map(|t| {
            NavTarget::CodeFile {
                repo_id: t.file.repo_id.clone(),
                rel_path: t.file.rel_path.clone(),
                pos: t.cursor,
                scroll_y: t.scroll_y.round() as i32,
            }
        }),
        EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|w| NavTarget::WorkingDiff {
            file_key: w.file.key(),
            staged: w.data.staged_only,
            side: w.active_side,
            pos: w.active_cursor(),
            scroll_y: match w.active_side {
                DiffSide::Old => w.old_scroll_y.round() as i32,
                DiffSide::New => w.new_scroll_y.round() as i32,
            },
        }),
        EditorTabKind::CommitDiff => s.commit_diff.as_ref().map(|c| NavTarget::CommitDiff {
            repo_id: c.commit.repo_id.clone(),
            commit_hash: c.commit.hash.clone(),
            file_path: c.file.path.clone(),
            side: c.active_side,
            pos: c.active_cursor(),
            scroll_y: match c.active_side {
                DiffSide::Old => c.old_scroll_y.round() as i32,
                DiffSide::New => c.new_scroll_y.round() as i32,
            },
        }),
        EditorTabKind::Welcome | EditorTabKind::Comparison => None,
    })
}

fn push_nav_back_current() {
    if let Some(cur) = current_nav_target() {
        app_signal().update(|s| {
            if s.nav_back.last() != Some(&cur) {
                s.nav_back.push(cur);
                if s.nav_back.len() > 64 {
                    s.nav_back.remove(0);
                }
            }
            s.nav_forward.clear();
        });
    }
}

fn record_recent_location(loc: NavTarget) {
    app_signal().update(|s| {
        s.recent_locations.retain(|x| x != &loc);
        s.recent_locations.insert(0, loc);
        if s.recent_locations.len() > 32 {
            s.recent_locations.truncate(32);
        }
    });
}

pub fn restore_nav_target(nav: NavTarget) {
    crate::code_split_runtime::capture_views();
    match nav {
        NavTarget::CodeFile {
            repo_id,
            rel_path,
            pos,
            ..
        } => {
            let wf = app_signal().with_untracked(|s| {
                s.workspace_files
                    .iter()
                    .find(|f| f.repo_id == repo_id && f.rel_path == rel_path)
                    .cloned()
                    .or_else(|| {
                        s.repos.iter().find(|r| r.id == repo_id).map(|r| WorkspaceFile {
                            repo_id: r.id.clone(),
                            repo_name: r.name.clone(),
                            repo_path: r.path.clone(),
                            rel_path: rel_path.clone(),
                            abs_path: Path::new(&r.path)
                                .join(&rel_path)
                                .to_string_lossy()
                                .to_string(),
                            color_idx: r.color_idx,
                        })
                    })
            });
            if let Some(f) = wf {
                // 不重复 push nav_back
                let tab_id = format!("file::{}::{}", f.repo_id, f.rel_path);
                let existing = app_signal()
                    .with_untracked(|s| s.code_tabs.iter().position(|t| t.id == tab_id));
                if let Some(idx) = existing {
                    app_signal().update(|s| {
                        crate::code_split_runtime::select_tab_in_model(s, idx);
                        crate::code_split_runtime::set_active_position(s, pos);
                        s.editor_tab = EditorTabKind::File;
                        s.focus_zone = FocusZone::CodeEditor;
                    });
                } else {
                    let wt = git::read_review_worktree(Path::new(&f.abs_path));
                    app_signal().update(|s| {
                        s.code_tabs.push(CodeTab {
                            id: tab_id,
                            file: f,
                            revision: String::new(),
                            text: wt.text.clone(),
                            saved_text: wt.text,
                            line_ending: wt.line_ending,
                            version: wt.version,
                            editable: wt.editable,
                            readonly_reason: wt.readonly_reason,
                            binary: wt.binary,
                            too_large: wt.too_large,
                            cursor: pos,
                            scroll_y: 0.0,
                            scroll_x: 0.0,
                            preview: false,
                            external_changed: false,
                            content_rev: 1,
                        });
                        crate::code_split_runtime::select_tab_in_model(s, s.code_tabs.len() - 1);
                        s.editor_tab = EditorTabKind::File;
                        s.focus_zone = FocusZone::CodeEditor;
                    });
                }
                reveal_in_code_editor(pos);
                focus_active_editor_ref();
            }
        }
        NavTarget::WorkingDiff {
            file_key,
            staged,
            side,
            pos,
            scroll_y,
        } => {
            let need_reopen = app_signal().with_untracked(|s| {
                s.working_diff
                    .as_ref()
                    .map(|w| w.file.key() != file_key || w.data.staged_only != staged)
                    .unwrap_or(true)
            });
            if need_reopen {
                let cf = app_signal().with_untracked(|s| {
                    s.statuses
                        .iter()
                        .flat_map(|st| st.files.iter())
                        .find(|f| f.key() == file_key)
                        .cloned()
                });
                if let Some(cf) = cf {
                    let ws_mode = app_signal()
                        .with_untracked(|s| WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode));
                    let data = git::read_review_change(&cf, staged);
                    let computed = compute_diff(&data.old_text, &data.new_text, ws_mode);
                    let new_text = data.new_text.clone();
                    app_signal().update(|s| {
                        let rev = s.working_diff.as_ref().map(|w| w.content_rev + 1).unwrap_or(1);
                        s.working_diff = Some(WorkingDiffSession {
                            file: cf,
                            data,
                            current_new_text: new_text.clone(),
                            saved_new_text: new_text,
                            computed,
                            active_side: side,
                            old_cursor: if side == DiffSide::Old {
                                pos
                            } else {
                                EditorPosition::new(0, 0)
                            },
                            new_cursor: if side == DiffSide::New {
                                pos
                            } else {
                                EditorPosition::new(0, 0)
                            },
                            old_scroll_y: if side == DiffSide::Old {
                                scroll_y as f32
                            } else {
                                0.0
                            },
                            new_scroll_y: if side == DiffSide::New {
                                scroll_y as f32
                            } else {
                                0.0
                            },
                            current_hunk: 0,
                            boundary_hint: None,
                            external_changed: false,
                            content_rev: rev,
                        });
                    });
                }
            }
            app_signal().update(|s| {
                sync_dirty_code_tab_into_working_diff(s);
                if let Some(w) = &mut s.working_diff {
                    w.active_side = side;
                    match side {
                        DiffSide::Old => {
                            w.old_cursor = pos;
                            w.old_scroll_y = scroll_y as f32;
                        }
                        DiffSide::New => {
                            w.new_cursor = pos;
                            w.new_scroll_y = scroll_y as f32;
                        }
                    }
                }
                s.editor_tab = EditorTabKind::WorkingDiff;
                s.focus_zone = match side {
                    DiffSide::Old => FocusZone::WorkingDiffOld,
                    DiffSide::New => FocusZone::WorkingDiffNew,
                };
            });
            set_text_input_focused(false);
            reveal_in_active_diff(side, pos, Some(scroll_y as f32));
            focus_active_editor_ref();
        }
        NavTarget::CommitDiff {
            repo_id,
            commit_hash,
            file_path,
            side,
            pos,
            scroll_y,
        } => {
            app_signal().update(|s| {
                if let Some(c) = &mut s.commit_diff {
                    if c.commit.repo_id == repo_id
                        && c.commit.hash == commit_hash
                        && c.file.path == file_path
                    {
                        c.active_side = side;
                        match side {
                            DiffSide::Old => {
                                c.old_cursor = pos;
                                c.old_scroll_y = scroll_y as f32;
                            }
                            DiffSide::New => {
                                c.new_cursor = pos;
                                c.new_scroll_y = scroll_y as f32;
                            }
                        }
                        s.editor_tab = EditorTabKind::CommitDiff;
                        s.focus_zone = match side {
                            DiffSide::Old => FocusZone::CommitDiffOld,
                            DiffSide::New => FocusZone::CommitDiffNew,
                        };
                    }
                }
            });
            reveal_in_active_diff(side, pos, Some(scroll_y as f32));
            focus_active_editor_ref();
        }
    }
}

// ============================================================================
// 受检保存与冲突二次核对
// ============================================================================

pub fn save_active_document() -> bool {
    let tab = app_signal().with_untracked(|s| s.editor_tab);
    match tab {
        EditorTabKind::WorkingDiff => save_working_diff_with_format(),
        EditorTabKind::File => save_active_code_tab_now(),
        _ => false,
    }
}

pub fn save_working_diff_with_format() -> bool {
    if let Some(target) = crate::format_runtime::working_target()
        && crate::format_runtime::save_if_enabled(vec![target], || {})
    {
        return false;
    }
    save_working_diff_now()
}

pub fn save_working_diff_now() -> bool {
    let req = app_signal().with_untracked(|s| {
        s.working_diff.as_ref().map(|w| {
            (
                w.file.key(),
                w.file.repo_id.clone(),
                w.file.repo_path.clone(),
                w.file.path.clone(),
                w.file.abs_path(),
                w.data.version.clone(),
                w.data.line_ending,
                w.current_new_text.clone(),
                w.data.editable && !w.data.staged_only,
            )
        })
    });
    let Some((file_key, repo_id, repo_path, rel_path, abs_path, expected, le, text, editable)) = req else {
        return false;
    };
    if !editable {
        show_toast("warn", "当前差异视图为只读，无法保存");
        return false;
    }

    let history = LocalHistoryStore::default_store();
    let outcome = git::save_review_text_in_repo(
        &repo_id,
        Some(Path::new(&repo_path)),
        &rel_path,
        Path::new(&abs_path),
        &expected,
        le,
        &text,
        Some(&history),
    );
    match outcome {
        SaveReviewOutcome::Saved { version } => {
            app_signal().update(|s| {
                if let Some(w) = &mut s.working_diff {
                    if w.file.key() == file_key {
                        w.data.version = version.clone();
                        w.data.new_text = text.clone();
                        w.saved_new_text = text.clone();
                        w.external_changed = false;
                    }
                }
                // 同步打开中的同文件非修订版源码标签（干净、内容一致或基于同一保存前基线版本）
                for t in &mut s.code_tabs {
                    if t.revision.is_empty()
                        && t.file.repo_id == repo_id
                        && t.file.rel_path == rel_path
                        && (!t.is_dirty()
                            || t.text == text
                            || t.version.sha256 == expected.sha256)
                    {
                        if t.text != text {
                            s.code_undo.remove(&t.id);
                        }
                        t.text = text.clone();
                        t.saved_text = text.clone();
                        t.version = version.clone();
                        t.external_changed = false;
                        t.content_rev += 1;
                    }
                }
            });
            show_toast("ok", format!("已保存 {rel_path}"));
            refresh_workspace_async(Some(HashSet::from([repo_id])));
            true
        }
        SaveReviewOutcome::Conflict {
            disk_text,
            disk_version,
            disk_deleted,
        } => {
            app_signal().update(|s| {
                s.save_conflict = Some(SaveConflictModal {
                    target: ConflictTarget::WorkingDiff { file_key },
                    repo_id,
                    rel_path,
                    abs_path,
                    disk_text,
                    disk_version,
                    disk_deleted,
                    edited_text: text,
                    warning_message: None,
                });
                enter_modal_focus(s);
            });
            false
        }
        SaveReviewOutcome::Rejected { reason } => {
            show_toast("err", reason);
            false
        }
    }
}

pub fn save_active_code_tab_now() -> bool {
    let tab_id = app_signal().with_untracked(|s| {
        s.code_tabs
            .get(s.active_code_tab_idx)
            .map(|t| t.id.clone())
    });
    match tab_id {
        Some(id) => {
            if crate::format_runtime::save_if_enabled(vec![crate::format_runtime::Target::Code(id.clone())], || {}) {
                return false;
            }
            save_code_tab_by_id(&id)
        }
        None => false,
    }
}

pub fn save_code_tab_by_id(tab_id: &str) -> bool {
    let req = app_signal().with_untracked(|s| {
        s.code_tabs.iter().find(|t| t.id == tab_id).map(|t| {
            (
                t.id.clone(),
                t.file.repo_id.clone(),
                t.file.repo_path.clone(),
                t.file.rel_path.clone(),
                t.file.abs_path.clone(),
                t.version.clone(),
                t.line_ending,
                t.text.clone(),
                t.editable && t.revision.is_empty(),
            )
        })
    });
    let Some((id, repo_id, repo_path, rel_path, abs_path, expected, le, text, editable)) = req else {
        return false;
    };
    if !editable {
        show_toast("warn", "当前文件为只读，无法保存");
        return false;
    }

    let history = LocalHistoryStore::default_store();
    let outcome = git::save_review_text_in_repo(
        &repo_id,
        Some(Path::new(&repo_path)),
        &rel_path,
        Path::new(&abs_path),
        &expected,
        le,
        &text,
        Some(&history),
    );
    match outcome {
        SaveReviewOutcome::Saved { version } => {
            app_signal().update(|s| {
                if let Some(t) = s.code_tabs.iter_mut().find(|t| t.id == id) {
                    t.version = version.clone();
                    t.saved_text = text.clone();
                    t.external_changed = false;
                }
                if let Some(w) = &mut s.working_diff {
                    if w.file.repo_id == repo_id
                        && w.file.path == rel_path
                        && (!w.is_dirty()
                            || w.current_new_text == text
                            || w.data.version.sha256 == expected.sha256)
                    {
                        let ws_mode = WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode);
                        w.data.version = version;
                        w.data.new_text = text.clone();
                        w.current_new_text = text.clone();
                        w.saved_new_text = text;
                        w.computed = compute_diff(&w.data.old_text, &w.current_new_text, ws_mode);
                        w.external_changed = false;
                        w.content_rev += 1;
                    }
                }
            });
            show_toast("ok", format!("已保存 {rel_path}"));
            refresh_workspace_async(Some(HashSet::from([repo_id])));
            true
        }
        SaveReviewOutcome::Conflict {
            disk_text,
            disk_version,
            disk_deleted,
        } => {
            app_signal().update(|s| {
                s.save_conflict = Some(SaveConflictModal {
                    target: ConflictTarget::CodeTab { tab_id: id },
                    repo_id,
                    rel_path,
                    abs_path,
                    disk_text,
                    disk_version,
                    disk_deleted,
                    edited_text: text,
                    warning_message: None,
                });
                enter_modal_focus(s);
            });
            false
        }
        SaveReviewOutcome::Rejected { reason } => {
            show_toast("err", reason);
            false
        }
    }
}

/// 冲突弹窗点击「使用磁盘版本 / 放弃编辑并关闭文件」：
/// 必须再次重读磁盘核对 SHA-256；若磁盘在弹窗期间再次变化，刷新左侧并提示再次核对！
pub fn accept_disk_version_in_conflict() {
    let Some(modal) = app_signal().with_untracked(|s| s.save_conflict.clone()) else {
        return;
    };
    let latest = git::read_review_worktree(Path::new(&modal.abs_path));
    let latest_deleted = !latest.version.exists;
    let changed_again = latest_deleted != modal.disk_deleted
        || (!latest_deleted && latest.version.sha256 != modal.disk_version.sha256);

    if changed_again {
        app_signal().update(|s| {
            if let Some(m) = &mut s.save_conflict {
                m.disk_text = latest.text;
                m.disk_version = latest.version;
                m.disk_deleted = latest_deleted;
                m.warning_message = Some("磁盘内容再次变化，请核对最新版本".to_string());
            }
        });
        return;
    }

    // 覆盖放弃前先将用户未保存编辑写入本地历史，防误丢
    let history = LocalHistoryStore::default_store();
    let _ = history.record(
        &modal.repo_id,
        &modal.rel_path,
        "冲突放弃前",
        modal.edited_text.as_bytes(),
    );

    if modal.disk_deleted {
        // 磁盘已删：放弃编辑并关闭对应文件或清空工作区 Diff
        app_signal().update(|s| {
            s.save_conflict = None;
            match &modal.target {
                ConflictTarget::CodeTab { tab_id } => {
                    s.code_tabs.retain(|t| &t.id != tab_id);
                    crate::code_split_runtime::prune_closed_tabs(s);
                    if s.active_code_tab_idx >= s.code_tabs.len() && !s.code_tabs.is_empty() {
                        crate::code_split_runtime::select_tab_in_model(s, s.code_tabs.len() - 1);
                    }
                    if s.code_tabs.is_empty() && s.editor_tab == EditorTabKind::File {
                        s.editor_tab = if s.working_diff.is_some() {
                            EditorTabKind::WorkingDiff
                        } else {
                            EditorTabKind::Welcome
                        };
                    }
                }
                ConflictTarget::WorkingDiff { .. } => {
                    s.working_diff = None;
                    if s.editor_tab == EditorTabKind::WorkingDiff {
                        s.editor_tab = if !s.code_tabs.is_empty() {
                            EditorTabKind::File
                        } else {
                            EditorTabKind::Welcome
                        };
                    }
                }
            }
        });
        show_toast("info", "已放弃编辑并关闭已删除文件");
        return;
    }

    app_signal().update(|s| {
        s.save_conflict = None;
        match &modal.target {
            ConflictTarget::CodeTab { tab_id } => {
                if let Some(t) = s.code_tabs.iter_mut().find(|t| &t.id == tab_id) {
                    s.code_undo.remove(&t.id);
                    t.text = latest.text.clone();
                    t.saved_text = latest.text;
                    t.line_ending = latest.line_ending;
                    t.version = latest.version;
                    t.editable = latest.editable;
                    t.readonly_reason = latest.readonly_reason;
                    t.external_changed = false;
                    t.content_rev += 1;
                }
                s.focus_zone = FocusZone::CodeEditor;
            }
            ConflictTarget::WorkingDiff { .. } => {
                let ws_mode = WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode);
                if let Some(w) = &mut s.working_diff {
                    w.data.new_text = latest.text.clone();
                    w.current_new_text = latest.text.clone();
                    w.saved_new_text = latest.text;
                    w.data.line_ending = latest.line_ending;
                    w.data.version = latest.version;
                    w.data.editable = latest.editable;
                    w.computed = compute_diff(&w.data.old_text, &w.current_new_text, ws_mode);
                    w.external_changed = false;
                    w.content_rev += 1;
                }
                s.focus_zone = FocusZone::WorkingDiffNew;
            }
        }
    });
    show_toast("ok", "已加载磁盘最新版本");
}

// ============================================================================
// 关闭标签与脏标签保护
// ============================================================================

pub fn close_active_tab() {
    let tab = app_signal().with_untracked(|s| s.editor_tab);
    match tab {
        EditorTabKind::WorkingDiff => request_close_working_diff(),
        EditorTabKind::CommitDiff => close_commit_diff_now(),
        EditorTabKind::Comparison => crate::ui::comparison::close(),
        EditorTabKind::File => {
            if let Some(id) = app_signal().with_untracked(|s| {
                s.code_tabs
                    .get(s.active_code_tab_idx)
                    .map(|t| t.id.clone())
            }) {
                request_close_code_tab(&id);
            }
        }
        EditorTabKind::Welcome => {}
    }
}

pub fn request_close_working_diff() {
    let dirty = app_signal().with_untracked(|s| {
        s.working_diff.as_ref().and_then(|w| {
            if w.is_dirty() {
                Some(format!("Diff: {}", basename(&w.file.path)))
            } else {
                None
            }
        })
    });
    if let Some(title) = dirty {
        app_signal().update(|s| {
            s.dirty_close = Some(DirtyCloseModal {
                action: DirtyCloseAction::CloseWorkingDiff,
                dirty_titles: vec![title],
            });
            enter_modal_focus(s);
        });
    } else {
        close_working_diff_now();
    }
}

pub fn close_working_diff_now() {
    app_signal().update(|s| {
        s.working_diff = None;
        s.diff_find_open = false;
        if s.editor_tab == EditorTabKind::WorkingDiff {
            s.editor_tab = if !s.code_tabs.is_empty() {
                EditorTabKind::File
            } else if s.commit_diff.is_some() {
                EditorTabKind::CommitDiff
            } else {
                EditorTabKind::Welcome
            };
        }
        // 关闭 Diff 后保留原选中行并恢复 Commit 树焦点
        s.focus_zone = FocusZone::CommitTree;
    });
}

pub fn close_commit_diff_now() {
    app_signal().update(|s| {
        s.commit_diff = None;
        s.diff_find_open = false;
        if s.editor_tab == EditorTabKind::CommitDiff {
            s.editor_tab = if !s.code_tabs.is_empty() {
                EditorTabKind::File
            } else if s.working_diff.is_some() {
                EditorTabKind::WorkingDiff
            } else {
                EditorTabKind::Welcome
            };
        }
        s.focus_zone = FocusZone::GitFiles;
    });
}

pub fn request_close_code_tab(tab_id: &str) {
    let dirty = app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .find(|t| t.id == tab_id && t.is_dirty())
            .map(|t| t.title())
    });
    if let Some(title) = dirty {
        app_signal().update(|s| {
            s.dirty_close = Some(DirtyCloseModal {
                action: DirtyCloseAction::CloseSingleCodeTab(tab_id.to_string()),
                dirty_titles: vec![title],
            });
            enter_modal_focus(s);
        });
    } else {
        force_close_code_tabs(|t| t.id == tab_id);
    }
}

pub fn request_close_other_code_tabs(keep_id: &str) {
    let dirty_titles: Vec<String> = app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .filter(|t| t.id != keep_id && t.is_dirty())
            .map(|t| t.title())
            .collect()
    });
    if !dirty_titles.is_empty() {
        app_signal().update(|s| {
            s.dirty_close = Some(DirtyCloseModal {
                action: DirtyCloseAction::CloseOtherCodeTabs(keep_id.to_string()),
                dirty_titles,
            });
            enter_modal_focus(s);
        });
    } else {
        force_close_code_tabs(|t| t.id != keep_id);
    }
}

pub fn request_close_all_code_tabs() {
    let dirty_titles: Vec<String> = app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .filter(|t| t.is_dirty())
            .map(|t| t.title())
            .collect()
    });
    if !dirty_titles.is_empty() {
        app_signal().update(|s| {
            s.dirty_close = Some(DirtyCloseModal {
                action: DirtyCloseAction::CloseAllCodeTabs,
                dirty_titles,
            });
            enter_modal_focus(s);
        });
    } else {
        force_close_code_tabs(|_| true);
    }
}

pub fn resolve_dirty_close(save_first: bool) {
    let Some(modal) = app_signal().with_untracked(|s| s.dirty_close.clone()) else {
        return;
    };
    app_signal().update(|s| {
        s.dirty_close = None;
        restore_focus_after_modal(s);
    });

    if save_first {
        use crate::format_runtime::Target;
        let targets = match &modal.action {
            DirtyCloseAction::CloseWorkingDiff => crate::format_runtime::working_target().into_iter().collect(),
            DirtyCloseAction::CloseSingleCodeTab(id) => vec![Target::Code(id.clone())],
            action => app_signal().with_untracked(|s| {
                s.code_tabs.iter().filter(|tab| {
                    tab.is_dirty() && !matches!(action, DirtyCloseAction::CloseOtherCodeTabs(keep) if keep == &tab.id)
                }).map(|tab| Target::Code(tab.id.clone())).collect()
            }),
        };
        let action = modal.action.clone();
        if crate::format_runtime::save_if_enabled(targets, move || match action {
            DirtyCloseAction::CloseWorkingDiff => request_close_working_diff(),
            DirtyCloseAction::CloseSingleCodeTab(id) => request_close_code_tab(&id),
            DirtyCloseAction::CloseOtherCodeTabs(id) => request_close_other_code_tabs(&id),
            DirtyCloseAction::CloseAllCodeTabs => request_close_all_code_tabs(),
        }) {
            return;
        }
    }

    match modal.action {
        DirtyCloseAction::CloseWorkingDiff => {
            if save_first && !save_working_diff_now() {
                return;
            }
            close_working_diff_now();
        }
        DirtyCloseAction::CloseSingleCodeTab(id) => {
            if save_first && !save_code_tab_by_id(&id) {
                return;
            }
            force_close_code_tabs(|t| t.id == id);
        }
        DirtyCloseAction::CloseOtherCodeTabs(keep_id) => {
            if save_first {
                let ids: Vec<String> = app_signal().with_untracked(|s| {
                    s.code_tabs
                        .iter()
                        .filter(|t| t.id != keep_id && t.is_dirty())
                        .map(|t| t.id.clone())
                        .collect()
                });
                for id in ids {
                    if !save_code_tab_by_id(&id) {
                        return;
                    }
                }
            }
            force_close_code_tabs(|t| t.id != keep_id);
        }
        DirtyCloseAction::CloseAllCodeTabs => {
            if save_first {
                let ids: Vec<String> = app_signal().with_untracked(|s| {
                    s.code_tabs
                        .iter()
                        .filter(|t| t.is_dirty())
                        .map(|t| t.id.clone())
                        .collect()
                });
                for id in ids {
                    if !save_code_tab_by_id(&id) {
                        return;
                    }
                }
            }
            force_close_code_tabs(|_| true);
        }
    }
}

fn force_close_code_tabs(should_close: impl Fn(&CodeTab) -> bool) {
    app_signal().update(|s| {
        s.code_tabs.retain(|t| !should_close(t));
        crate::code_split_runtime::prune_closed_tabs(s);
        if s.code_tabs.is_empty() {
            s.active_code_tab_idx = 0;
            if s.editor_tab == EditorTabKind::File {
                s.editor_tab = if s.working_diff.is_some() {
                    EditorTabKind::WorkingDiff
                } else if s.commit_diff.is_some() {
                    EditorTabKind::CommitDiff
                } else {
                    EditorTabKind::Welcome
                };
            }
        } else if s.active_code_tab_idx >= s.code_tabs.len() {
            crate::code_split_runtime::select_tab_in_model(s, s.code_tabs.len() - 1);
        }
    });
}

// ============================================================================
// 剪贴板复制、路径引用与系统打开/定位
// ============================================================================

/// 按当前焦点执行 `Cmd+C` 或原生 `Edit > Copy`：
/// - 编辑器/Diff 聚焦：复制当前选区文本（空选区不破坏剪贴板）
/// - Commit 树聚焦：未跟踪文件或目录复制 `display_path`，已跟踪文件复制 `basename`
/// - Project 树聚焦：复制文件或目录名
/// - Git 提交文件树聚焦：复制文件名
pub fn copy_for_current_focus() {
    let focus = app_signal().with_untracked(|s| s.focus_zone);
    match focus {
        FocusZone::CodeEditor
        | FocusZone::WorkingDiffOld
        | FocusZone::WorkingDiffNew
        | FocusZone::CommitDiffOld
        | FocusZone::CommitDiffNew
        | FocusZone::ComparisonOld
        | FocusZone::ComparisonNew => {
            if let Some(ed) = active_editor_api(focus) {
                let selected = ed.selected_text();
                if !selected.is_empty() {
                    set_clipboard(selected);
                } else {
                    ed.run(nsh_base::EditorCommand::Copy);
                }
            }
        }
        FocusZone::CommitTree => {
            let text = app_signal().with_untracked(|s| {
                if let Some(sel) = &s.change_selection {
                    if sel.file.untracked {
                        Some(display_path(&sel.file.abs_path()))
                    } else {
                        Some(basename(&sel.file.path))
                    }
                } else {
                    s.change_selected_dir
                        .as_ref()
                        .map(|(_, _, abs)| display_path(abs))
                }
            });
            if let Some(t) = text {
                set_clipboard(t);
            }
        }
        FocusZone::ProjectTree => {
            let text = app_signal().with_untracked(|s| {
                if let Some(wf) = &s.project_selected_file {
                    Some(basename(&wf.rel_path))
                } else if let Some((_, dir)) = &s.project_selected_dir {
                    Some(basename(dir))
                } else {
                    None
                }
            });
            if let Some(t) = text {
                set_clipboard(t);
            }
        }
        FocusZone::GitFiles => {
            let text = app_signal()
                .with_untracked(|s| s.selected_commit_file.as_ref().map(|f| basename(&f.path)));
            if let Some(t) = text {
                set_clipboard(t);
            }
        }
        FocusZone::Modal => {
            if let Some(api) = editor_refs_signal()
                .get_untracked()
                .modal_active
                .and_then(|r| r.api::<EditorApi>())
            {
                let selected = api.selected_text();
                if !selected.is_empty() {
                    set_clipboard(selected);
                } else {
                    api.run(nsh_base::EditorCommand::Copy);
                }
            }
        }
    }
}

/// 获取当前焦点对象的 `(repo_path, rel_path, abs_path, line_1based)`。
pub fn current_location_info() -> Option<(String, String, String, u32)> {
    app_signal().with_untracked(|s| match s.focus_zone {
        FocusZone::CodeEditor => s.code_tabs.get(s.active_code_tab_idx).map(|t| {
            (
                t.file.repo_path.clone(),
                t.file.rel_path.clone(),
                t.file.abs_path.clone(),
                t.cursor.line + 1,
            )
        }),
        FocusZone::WorkingDiffOld => s.working_diff.as_ref().map(|w| {
            let p = w.data.old_path.clone();
            let abs = Path::new(&w.file.repo_path).join(&p).to_string_lossy().to_string();
            (w.file.repo_path.clone(), p, abs, w.old_cursor.line + 1)
        }),
        FocusZone::WorkingDiffNew => s.working_diff.as_ref().map(|w| {
            let p = w.data.new_path.clone();
            let abs = Path::new(&w.file.repo_path).join(&p).to_string_lossy().to_string();
            (w.file.repo_path.clone(), p, abs, w.new_cursor.line + 1)
        }),
        FocusZone::CommitDiffOld => s.commit_diff.as_ref().map(|c| {
            let p = c.data.old_path.clone();
            let abs = Path::new(&c.commit.repo_path).join(&p).to_string_lossy().to_string();
            (c.commit.repo_path.clone(), p, abs, c.old_cursor.line + 1)
        }),
        FocusZone::CommitDiffNew => s.commit_diff.as_ref().map(|c| {
            let p = c.data.new_path.clone();
            let abs = Path::new(&c.commit.repo_path).join(&p).to_string_lossy().to_string();
            (c.commit.repo_path.clone(), p, abs, c.new_cursor.line + 1)
        }),
        FocusZone::CommitTree => s
            .change_selection
            .as_ref()
            .map(|sel| {
                (
                    sel.file.repo_path.clone(),
                    sel.file.path.clone(),
                    sel.file.abs_path(),
                    1,
                )
            })
            .or_else(|| {
                s.change_selected_dir.as_ref().and_then(|(rid, rel, abs)| {
                    s.repos
                        .iter()
                        .find(|r| &r.id == rid)
                        .map(|r| (r.path.clone(), rel.clone(), abs.clone(), 1))
                })
            }),
        FocusZone::ProjectTree => {
            if let Some(wf) = &s.project_selected_file {
                Some((wf.repo_path.clone(), wf.rel_path.clone(), wf.abs_path.clone(), 1))
            } else if let Some((repo_id, rel)) = &s.project_selected_dir {
                s.repos.iter().find(|r| &r.id == repo_id).map(|r| {
                    let abs = Path::new(&r.path).join(rel).to_string_lossy().to_string();
                    (r.path.clone(), rel.clone(), abs, 1)
                })
            } else {
                None
            }
        }
        FocusZone::GitFiles => {
            let commit = s.selected_commit.as_ref()?;
            let file = s.selected_commit_file.as_ref()?;
            let abs = Path::new(&commit.repo_path)
                .join(&file.path)
                .to_string_lossy()
                .to_string();
            Some((commit.repo_path.clone(), file.path.clone(), abs, 1))
        }
        FocusZone::Modal | FocusZone::ComparisonOld | FocusZone::ComparisonNew => None,
    })
}

pub fn copy_absolute_path_current() {
    if let Some((_, _, abs, _)) = current_location_info() {
        set_clipboard(abs.clone());
        show_toast("ok", format!("已复制绝对路径: {}", basename(&abs)));
    }
}

pub fn copy_reference_current() {
    let ws = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    if let Some((_, rel, abs, line)) = current_location_info() {
        let reference = path_with_line_reference(&ws, &abs, &rel, line);
        set_clipboard(reference.clone());
        show_toast("ok", format!("已复制引用: {reference}"));
    }
}

pub fn copy_path_variant(
    repo_path: &str,
    rel_path: &str,
    abs_path: &str,
    line_1based: u32,
    variant: &str,
) {
    let ws = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let text = match variant {
        "name" => basename(rel_path),
        "abs" => abs_path.to_string(),
        "repo_rel" => relative_slash_path(repo_path, abs_path, rel_path),
        "ws_rel" => relative_slash_path(&ws, abs_path, rel_path),
        "dir" => parent_dir_display(abs_path),
        "ref" => path_with_line_reference(&ws, abs_path, rel_path, line_1based),
        "display" => display_path(abs_path),
        _ => abs_path.to_string(),
    };
    set_clipboard(text.clone());
    show_toast("ok", format!("已复制: {text}"));
}

fn select_project_file_in_tree(s: &mut AppState, repo_id: &str, rel_path: &str) {
    let norm = paths::normalize_slashes(rel_path);
    s.project_workspace_expanded = true;
    s.expanded_project_repos.insert(repo_id.to_string());

    let mut parent = Path::new(&norm).parent();
    while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
        s.expanded_project_dirs.insert(format!("{repo_id}::{}", dir.display()));
        parent = dir.parent();
    }
    let key = format!("{repo_id}::{norm}");
    s.project_located_key = Some(key.clone());
    if let Some(file) = s.workspace_files.iter()
        .find(|file| file.repo_id == repo_id && file.rel_path == norm)
        .cloned()
    {
        s.project_selected_file = Some(file);
        s.project_selected_dir = None;
        s.project_compare_keys = vec![key];
    } else {
        s.project_selected_file = None;
        s.project_selected_dir = None;
        s.project_compare_keys.clear();
    }
}

pub(crate) fn setup_project_follow() {
    let target = computed(|| app_signal().with(|s| {
        if !s.cfg.project_follow_editor || s.editor_tab != EditorTabKind::File {
            return None;
        }
        s.code_tabs.get(s.active_code_tab_idx)
            .filter(|tab| tab.revision.is_empty())
            .map(|tab| (tab.file.repo_id.clone(), tab.file.rel_path.clone()))
    }));
    effect(move || {
        if let Some((repo_id, path)) = target.get() {
            app_signal().update(|s| select_project_file_in_tree(s, &repo_id, &path));
        }
    });
}

/// 在文件树中定位（`⌥F1` 或右键「在文件树中定位」）：显示 Project 并聚焦目标。
pub fn locate_in_project_tree(repo_id: &str, rel_path: &str) {
    app_signal().update(|s| {
        s.show_left_panel = true;
        s.left_tab = LeftTab::Project;
        select_project_file_in_tree(s, repo_id, rel_path);
        s.focus_zone = FocusZone::ProjectTree;
    });
}

pub fn locate_current_in_project_tree() {
    let info = app_signal().with_untracked(|s| match s.editor_tab {
        EditorTabKind::File => s
            .code_tabs
            .get(s.active_code_tab_idx)
            .map(|t| (t.file.repo_id.clone(), t.file.rel_path.clone())),
        EditorTabKind::WorkingDiff => s
            .working_diff
            .as_ref()
            .map(|w| (w.file.repo_id.clone(), w.file.path.clone())),
        EditorTabKind::CommitDiff => s
            .commit_diff
            .as_ref()
            .map(|c| (c.commit.repo_id.clone(), c.data.new_path.clone())),
        EditorTabKind::Welcome => s
            .change_selection
            .as_ref()
            .map(|sel| (sel.file.repo_id.clone(), sel.file.path.clone())),
        EditorTabKind::Comparison => None,
    });
    if let Some((repo_id, rel)) = info {
        locate_in_project_tree(&repo_id, &rel);
    }
}

pub fn reveal_in_system_finder(abs_path: &str) {
    let target = nearest_existing_path(Path::new(abs_path));
    let Some(existing) = target else {
        show_toast("warn", "找不到目标路径或父目录");
        return;
    };
    let path_str = existing.to_string_lossy().to_string();
    spawn(async move {
        if let Err(err) = reveal_path(path_str).await {
            show_toast("err", format!("在文件管理器中显示失败: {err}"));
        }
    });
}

pub fn open_with_default_app(abs_path: &str) {
    if !Path::new(abs_path).exists() {
        show_toast("warn", "文件不存在，无法用默认应用打开");
        return;
    }
    let path_str = abs_path.to_string();
    spawn(async move {
        if let Err(err) = open_path(path_str).await {
            show_toast("err", format!("打开文件失败: {err}"));
        }
    });
}

// ============================================================================
// Diff 导航（F7 / Shift+F7 居中与文件边界二次确认）、折叠、空白选项与查找
// ============================================================================

pub fn jump_diff_hunk(delta: i32) {
    let tab = app_signal().with_untracked(|s| s.editor_tab);
    let sign = delta.signum();
    if sign == 0 {
        return;
    }
    match tab {
        EditorTabKind::WorkingDiff => {
            let action = app_signal().with_untracked(|s| {
                let w = s.working_diff.as_ref()?;
                let count = w.computed.hunks.len();
                if count == 0 {
                    if w.boundary_hint == Some(sign) {
                        return Some((None, Some(2 * sign)));
                    }
                    return Some((None, Some(sign)));
                }
                let next = w.current_hunk as i32 + delta;
                if next < 0 {
                    if w.boundary_hint == Some(-1) {
                        return Some((None, Some(-2))); // 触发切上一文件（落到末个差异块）
                    }
                    return Some((None, Some(-1)));
                }
                if next >= count as i32 {
                    if w.boundary_hint == Some(1) {
                        return Some((None, Some(2))); // 触发切下一文件
                    }
                    return Some((None, Some(1)));
                }
                Some((Some(next as usize), None))
            });
            let Some((target_hunk, boundary)) = action else {
                return;
            };
            match boundary {
                Some(-2) => {
                    if !move_change_selection_with_hunk(-1, true, true) {
                        show_toast("info", "已是第一个变更文件");
                    }
                }
                Some(2) => {
                    if !move_change_selection_with_hunk(1, true, false) {
                        show_toast("info", "已是最后一个变更文件");
                    }
                }
                Some(-1) => {
                    app_signal().update(|s| {
                        if let Some(w) = &mut s.working_diff {
                            w.boundary_hint = Some(-1);
                        }
                    });
                    show_toast("info", "已是本文件第一处差异，再按 ⇧F7 切到上一文件");
                }
                Some(1) => {
                    app_signal().update(|s| {
                        if let Some(w) = &mut s.working_diff {
                            w.boundary_hint = Some(1);
                        }
                    });
                    show_toast("info", "已是本文件最后一处差异，再按 F7 切到下一文件");
                }
                _ => {
                    if let Some(idx) = target_hunk {
                        scroll_to_working_hunk(idx);
                    }
                }
            }
        }
        EditorTabKind::CommitDiff => {
            let action = app_signal().with_untracked(|s| {
                let c = s.commit_diff.as_ref()?;
                let count = c.computed.hunks.len();
                if count == 0 {
                    if c.boundary_hint == Some(sign) {
                        return Some((None, Some(2 * sign)));
                    }
                    return Some((None, Some(sign)));
                }
                let next = c.current_hunk as i32 + delta;
                if next < 0 {
                    if c.boundary_hint == Some(-1) {
                        return Some((None, Some(-2)));
                    }
                    return Some((None, Some(-1)));
                }
                if next >= count as i32 {
                    if c.boundary_hint == Some(1) {
                        return Some((None, Some(2)));
                    }
                    return Some((None, Some(1)));
                }
                Some((Some(next as usize), None))
            });
            let Some((target_hunk, boundary)) = action else {
                return;
            };
            match boundary {
                Some(-2) => {
                    if !move_commit_diff_file_with_hunk(-1, true) {
                        show_toast("info", "已是本提交第一个文件");
                    }
                }
                Some(2) => {
                    if !move_commit_diff_file_with_hunk(1, false) {
                        show_toast("info", "已是本提交最后一个文件");
                    }
                }
                Some(-1) => {
                    app_signal().update(|s| {
                        if let Some(c) = &mut s.commit_diff {
                            c.boundary_hint = Some(-1);
                        }
                    });
                    show_toast("info", "已是第一处差异，再按 ⇧F7 切到上一文件");
                }
                Some(1) => {
                    app_signal().update(|s| {
                        if let Some(c) = &mut s.commit_diff {
                            c.boundary_hint = Some(1);
                        }
                    });
                    show_toast("info", "已是最后一处差异，再按 F7 切到下一文件");
                }
                _ => {
                    if let Some(idx) = target_hunk {
                        scroll_to_commit_hunk(idx);
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn scroll_to_working_hunk(idx: usize) {
    let info = app_signal().with_untracked(|s| {
        let w = s.working_diff.as_ref()?;
        let h = w.computed.hunks.get(idx)?;
        let single = s.cfg.diff_single_column;
        let unified_target = if single {
            let (line_map, hunk_starts, _) =
                compute_unified_layout(&w.data.old_text, &w.current_new_text, &w.computed.hunks);
            let u_line = hunk_starts.get(idx).copied().unwrap_or(0);
            let mapped = line_map.get(u_line).copied();
            Some((u_line as u32, mapped))
        } else {
            None
        };
        Some((h.old_start as u32, h.new_start as u32, single, unified_target))
    });
    let Some((old_l, new_l, single, unified_target)) = info else {
        return;
    };
    app_signal().update(|s| {
        if let Some(w) = &mut s.working_diff {
            w.current_hunk = idx;
            w.boundary_hint = None;
            w.old_cursor = EditorPosition::new(old_l, 0);
            w.new_cursor = EditorPosition::new(new_l, 0);
            if let Some((_, Some(m))) = unified_target {
                w.active_side = m.side;
                match m.side {
                    DiffSide::Old => w.old_cursor = EditorPosition::new(m.orig_line as u32, 0),
                    DiffSide::New => w.new_cursor = EditorPosition::new(m.orig_line as u32, 0),
                }
            }
        }
    });
    let refs = editor_refs_signal().get_untracked();
    if single {
        if let Some((u_line, _)) = unified_target {
            if let Some(api) = refs.work_unified.and_then(|r| r.api::<EditorApi>()) {
                let pos = EditorPosition::new(u_line, 2);
                api.set_selections(vec![EditorSelection::caret(pos)]);
                api.reveal_position(pos, true);
            }
        }
    } else {
        if let Some(r) = refs.work_old {
            if let Some(api) = r.api::<EditorApi>() {
                api.reveal_position(EditorPosition::new(old_l, 0), true);
            }
        }
        if let Some(r) = refs.work_new {
            if let Some(api) = r.api::<EditorApi>() {
                api.set_selections(vec![EditorSelection::caret(EditorPosition::new(new_l, 0))]);
                api.reveal_position(EditorPosition::new(new_l, 0), true);
            }
        }
    }
}

pub fn scroll_to_commit_hunk(idx: usize) {
    let info = app_signal().with_untracked(|s| {
        let c = s.commit_diff.as_ref()?;
        let h = c.computed.hunks.get(idx)?;
        let single = s.cfg.diff_single_column;
        let unified_target = if single {
            let (line_map, hunk_starts, _) =
                compute_unified_layout(&c.data.old_text, &c.data.new_text, &c.computed.hunks);
            let u_line = hunk_starts.get(idx).copied().unwrap_or(0);
            let mapped = line_map.get(u_line).copied();
            Some((u_line as u32, mapped))
        } else {
            None
        };
        Some((h.old_start as u32, h.new_start as u32, single, unified_target))
    });
    let Some((old_l, new_l, single, unified_target)) = info else {
        return;
    };
    app_signal().update(|s| {
        if let Some(c) = &mut s.commit_diff {
            c.current_hunk = idx;
            c.boundary_hint = None;
            c.old_cursor = EditorPosition::new(old_l, 0);
            c.new_cursor = EditorPosition::new(new_l, 0);
            if let Some((_, Some(m))) = unified_target {
                c.active_side = m.side;
                match m.side {
                    DiffSide::Old => c.old_cursor = EditorPosition::new(m.orig_line as u32, 0),
                    DiffSide::New => c.new_cursor = EditorPosition::new(m.orig_line as u32, 0),
                }
            }
        }
    });
    let refs = editor_refs_signal().get_untracked();
    if single {
        if let Some((u_line, _)) = unified_target {
            if let Some(api) = refs.commit_unified.and_then(|r| r.api::<EditorApi>()) {
                let pos = EditorPosition::new(u_line, 2);
                api.set_selections(vec![EditorSelection::caret(pos)]);
                api.reveal_position(pos, true);
            }
        }
    } else {
        if let Some(r) = refs.commit_old {
            if let Some(api) = r.api::<EditorApi>() {
                api.reveal_position(EditorPosition::new(old_l, 0), true);
            }
        }
        if let Some(r) = refs.commit_new {
            if let Some(api) = r.api::<EditorApi>() {
                api.set_selections(vec![EditorSelection::caret(EditorPosition::new(new_l, 0))]);
                api.reveal_position(EditorPosition::new(new_l, 0), true);
            }
        }
    }
}

pub fn set_whitespace_mode(mode: WhitespaceMode) {
    app_signal().update(|s| {
        s.cfg.diff_whitespace_mode = mode.as_config().to_string();
        if let Some(w) = &mut s.working_diff {
            w.computed = compute_diff(&w.data.old_text, &w.current_new_text, mode);
            w.current_hunk = 0;
            w.content_rev += 1;
        }
        if let Some(c) = &mut s.commit_diff {
            c.computed = compute_diff(&c.data.old_text, &c.data.new_text, mode);
            c.current_hunk = 0;
            c.content_rev += 1;
        }
    });
    save_config_quiet();
}

pub fn set_inline_mode(mode: InlineHighlightMode) {
    app_signal().update(|s| {
        s.cfg.diff_inline_mode = mode.as_config().to_string();
    });
    save_config_quiet();
}

pub fn open_find_bar(with_replace: bool) {
    let (tab, focus) = app_signal().with_untracked(|s| (s.editor_tab, s.focus_zone));
    match tab {
        EditorTabKind::File => {
            app_signal().update(|s| {
                s.code_find_open = true;
                if with_replace {
                    s.code_replace_open = true;
                }
            });
        }
        EditorTabKind::WorkingDiff => {
            let side = if focus == FocusZone::WorkingDiffOld {
                DiffSide::Old
            } else {
                app_signal()
                    .with_untracked(|s| s.working_diff.as_ref().map(|w| w.active_side))
                    .unwrap_or(DiffSide::New)
            };
            app_signal().update(|s| {
                s.diff_find_open = true;
                s.diff_find_side = side;
            });
        }
        EditorTabKind::CommitDiff => {
            let side = if focus == FocusZone::CommitDiffOld {
                DiffSide::Old
            } else {
                app_signal()
                    .with_untracked(|s| s.commit_diff.as_ref().map(|c| c.active_side))
                    .unwrap_or(DiffSide::New)
            };
            app_signal().update(|s| {
                s.diff_find_open = true;
                s.diff_find_side = side;
            });
        }
        EditorTabKind::Welcome | EditorTabKind::Comparison => {}
    }
}

/// 进入弹窗前记录当前非弹窗焦点，确保关闭弹窗后恢复树/编辑器焦点而非永久滞留 `FocusZone::Modal`。
pub fn enter_modal_focus(s: &mut AppState) {
    if s.focus_zone != FocusZone::Modal {
        s.prev_focus_zone = s.focus_zone;
    }
    s.focus_zone = FocusZone::Modal;
}

/// 关闭弹窗后恢复先前焦点（若原焦点对应的编辑页已关闭则回退到 Commit 树）。
pub fn restore_focus_after_modal(s: &mut AppState) {
    if s.focus_zone != FocusZone::Modal {
        return;
    }
    let fallback = match s.editor_tab {
        EditorTabKind::File => FocusZone::CodeEditor,
        EditorTabKind::WorkingDiff => match s.working_diff.as_ref().map(|w| w.active_side) {
            Some(DiffSide::Old) => FocusZone::WorkingDiffOld,
            _ => FocusZone::WorkingDiffNew,
        },
        EditorTabKind::CommitDiff => match s.commit_diff.as_ref().map(|c| c.active_side) {
            Some(DiffSide::Old) => FocusZone::CommitDiffOld,
            _ => FocusZone::CommitDiffNew,
        },
        EditorTabKind::Comparison => FocusZone::ComparisonNew,
        EditorTabKind::Welcome => match s.left_tab {
            LeftTab::Commit => FocusZone::CommitTree,
            LeftTab::Project => FocusZone::ProjectTree,
        },
    };
    let candidate = s.prev_focus_zone;
    s.focus_zone = match candidate {
        FocusZone::Modal => fallback,
        FocusZone::CodeEditor if s.code_tabs.is_empty() => fallback,
        FocusZone::WorkingDiffOld | FocusZone::WorkingDiffNew if s.working_diff.is_none() => {
            fallback
        }
        FocusZone::CommitDiffOld | FocusZone::CommitDiffNew if s.commit_diff.is_none() => fallback,
        other => other,
    };
}

/// 统一关闭最顶层弹窗（或查找条），并恢复焦点与原生编辑器焦点。
pub fn close_active_modal() {
    if crate::ui::content_search::intercept_close() || crate::history_runtime::intercept_close()
        || crate::ui::index_review::intercept_close()
        || crate::ui::conflict_workbench::intercept_close() {
        return;
    }
    set_text_input_focused(false);
    editor_refs_signal().update(|r| r.modal_active = None);
    let content_search_closed = crate::ui::content_search::close_if_open();
    let bookmarks_closed = crate::ui::bookmarks::close_active();
    let navigation_closed = crate::ui::navigation_picker::close_active();
    let revision_closed = crate::ui::revision_picker::close_active();
    let keymap_closed = crate::ui::keymap_settings::close_active();
    let history_closed = crate::history_runtime::close_active();
    let editor_tool_closed = crate::ui::editor_tools::close_active();
    let git_prompt_closed = crate::ui::git_prompt::close_active();
    let run_config_closed = crate::ui::run_panel::close_active();
    let github_closed = crate::ui::github_panel::close_active();
    let settings_closed = crate::ui::settings::close_active();
    let branches_closed = crate::ui::branch_matrix::close_active();
    let sync_closed = crate::ui::sync_dialog::close_active();
    let index_review_closed = crate::ui::index_review::close_active();
    let conflict_workbench_closed = crate::ui::conflict_workbench::close_active();
    let merge_menu_closed = crate::ui::merge_menu::close_active();
    let rebase_plan_closed = crate::ui::rebase_plan_view::close_active();
    let git_advanced_closed = crate::ui::git_advanced::close_active();
    let credentials_closed = crate::ui::credentials_view::close_active();
    let mut restored_editor = false;
    app_signal().update(|s| {
        let mut closed_modal = false;
        if settings_closed || github_closed || run_config_closed || credentials_closed || git_advanced_closed || rebase_plan_closed || merge_menu_closed || conflict_workbench_closed || index_review_closed || sync_closed || branches_closed || git_prompt_closed || editor_tool_closed || content_search_closed || bookmarks_closed || navigation_closed || revision_closed || keymap_closed || history_closed {
            closed_modal = true;
        } else if s.save_conflict.is_some() {
            s.save_conflict = None;
            closed_modal = true;
        } else if s.dirty_close.is_some() {
            s.dirty_close = None;
            closed_modal = true;
        } else if s.goto_line_open {
            s.goto_line_open = false;
            closed_modal = true;
        } else if s.picker_open.is_some() {
            s.picker_open = None;
            closed_modal = true;
        } else if s.stash_modal_open {
            s.stash_modal_open = false;
            closed_modal = true;
        } else if s.diff_find_open {
            s.diff_find_open = false;
            restored_editor = true;
        } else if s.code_find_open {
            s.code_find_open = false;
            s.code_replace_open = false;
            restored_editor = true;
        }
        if closed_modal {
            restore_focus_after_modal(s);
            restored_editor = matches!(
                s.focus_zone,
                FocusZone::CodeEditor
                    | FocusZone::WorkingDiffOld
                    | FocusZone::WorkingDiffNew
                    | FocusZone::CommitDiffOld
                    | FocusZone::CommitDiffNew
            );
        }
    });
    if restored_editor {
        focus_active_editor_ref();
    }
}

/// 在当前编辑器（源码、双栏 Diff 聚焦侧或单栏 Diff）中跳转到下一/上一查找匹配（`⌘G` / `⇧⌘G`）。
pub fn run_active_find_nav(next: bool) {
    let (tab, side, single) =
        app_signal().with_untracked(|s| (s.editor_tab, s.diff_find_side, s.cfg.diff_single_column));
    if tab == EditorTabKind::File {
        if let Some(editor) = active_editor_api(FocusZone::CodeEditor) {
            editor.run(if next { nsh_base::EditorCommand::FindNext } else { nsh_base::EditorCommand::FindPrevious });
        }
        return;
    }
    let refs = editor_refs_signal().get_untracked();
    let target = match tab {
        EditorTabKind::File => None,
        EditorTabKind::WorkingDiff => {
            if single {
                refs.work_unified
            } else {
                match side {
                    DiffSide::Old => refs.work_old,
                    DiffSide::New => refs.work_new,
                }
            }
        }
        EditorTabKind::CommitDiff => {
            if single {
                refs.commit_unified
            } else {
                match side {
                    DiffSide::Old => refs.commit_old,
                    DiffSide::New => refs.commit_new,
                }
            }
        }
        EditorTabKind::Welcome | EditorTabKind::Comparison => None,
    };
    if let Some(api) = target.and_then(|r| r.api::<EditorApi>()) {
        api.run(if next {
            nsh_base::EditorCommand::FindNext
        } else {
            nsh_base::EditorCommand::FindPrevious
        });
    }
}

/// Project 树可见节点键盘导航（`↑`/`↓` 切换选中文件/目录，`→`/`←` 展开或折叠目录）。
pub fn move_project_selection(delta: i32) {
    #[derive(Clone)]
    enum ProjNode {
        Dir { repo_id: String, rel: String, key: String },
        File { wf: WorkspaceFile, key: String },
    }

    let (nodes, cur_key) = app_signal().with_untracked(|s| {
        let q = s.project_filter.trim().to_ascii_lowercase();
        let force_expand = !q.is_empty();
        let mut out = Vec::new();
        if !force_expand && !s.project_workspace_expanded {
            return (out, None);
        }
        for repo in &s.repos {
            let repo_files: Vec<WorkspaceFile> = s
                .workspace_files
                .iter()
                .filter(|f| f.repo_id == repo.id && (q.is_empty() || f.rel_path.to_ascii_lowercase().contains(&q)))
                .cloned()
                .collect();
            if force_expand && repo_files.is_empty() {
                continue;
            }
            if !(force_expand || s.expanded_project_repos.contains(&repo.id)) {
                continue;
            }
            // 按目录优先的树序展开可见节点
            #[derive(Default)]
            struct D {
                rel: String,
                subs: std::collections::BTreeMap<String, D>,
                files: Vec<WorkspaceFile>,
            }
            let mut root = D::default();
            for wf in repo_files {
                let parts: Vec<&str> = wf.rel_path.split('/').collect();
                if parts.len() <= 1 {
                    root.files.push(wf);
                } else {
                    let mut cur = &mut root;
                    let mut acc = String::new();
                    for seg in &parts[..parts.len() - 1] {
                        if !acc.is_empty() {
                            acc.push('/');
                        }
                        acc.push_str(seg);
                        cur = cur.subs.entry((*seg).to_string()).or_insert_with(|| D {
                            rel: acc.clone(),
                            subs: std::collections::BTreeMap::new(),
                            files: Vec::new(),
                        });
                    }
                    cur.files.push(wf);
                }
            }
            fn walk(
                d: &mut D,
                repo_id: &str,
                force_expand: bool,
                expanded_dirs: &HashSet<String>,
                out: &mut Vec<ProjNode>,
            ) {
                d.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
                for sub in d.subs.values_mut() {
                    let key = format!("{repo_id}::{}", sub.rel);
                    out.push(ProjNode::Dir {
                        repo_id: repo_id.to_string(),
                        rel: sub.rel.clone(),
                        key: key.clone(),
                    });
                    if force_expand || expanded_dirs.contains(&key) {
                        walk(sub, repo_id, force_expand, expanded_dirs, out);
                    }
                }
                for wf in &d.files {
                    let key = format!("{}::{}", wf.repo_id, wf.rel_path);
                    out.push(ProjNode::File {
                        wf: wf.clone(),
                        key,
                    });
                }
            }
            walk(&mut root, &repo.id, force_expand, &s.expanded_project_dirs, &mut out);
        }
        (out, s.project_located_key.clone())
    });

    if nodes.is_empty() {
        return;
    }
    let idx = cur_key
        .as_deref()
        .and_then(|ck| {
            nodes.iter().position(|n| match n {
                ProjNode::Dir { key, .. } | ProjNode::File { key, .. } => key == ck,
            })
        })
        .map(|i| i as i32)
        .unwrap_or(-1);
    let len = nodes.len() as i32;
    let next = if idx < 0 {
        if delta < 0 { (len - 1) as usize } else { 0 }
    } else {
        (((idx + delta) % len) + len) as usize % nodes.len()
    };
    match &nodes[next] {
        ProjNode::Dir { repo_id, rel, key } => {
            let (rid, r, k) = (repo_id.clone(), rel.clone(), key.clone());
            app_signal().update(|s| {
                s.project_located_key = Some(k);
                s.project_selected_dir = Some((rid, r));
                s.project_selected_file = None;
                s.project_compare_keys.clear();
                s.focus_zone = FocusZone::ProjectTree;
            });
        }
        ProjNode::File { wf, key } => {
            let (f, k) = (wf.clone(), key.clone());
            app_signal().update(|s| {
                s.project_located_key = Some(k.clone());
                s.project_selected_file = Some(f);
                s.project_selected_dir = None;
                s.project_compare_keys = vec![k];
                s.focus_zone = FocusZone::ProjectTree;
            });
        }
    }
}

pub fn toggle_project_selected_dir(expand: bool) {
    app_signal().update(|s| {
        if let Some((rid, rel)) = &s.project_selected_dir {
            let key = format!("{rid}::{rel}");
            if expand {
                s.expanded_project_dirs.insert(key);
            } else {
                s.expanded_project_dirs.remove(&key);
            }
        }
    });
}

pub fn open_goto_line_modal() {
    let tab = app_signal().with_untracked(|s| s.editor_tab);
    if matches!(tab, EditorTabKind::Welcome) {
        return;
    }
    app_signal().update(|s| {
        s.goto_line_open = true;
        s.goto_line_input.clear();
        enter_modal_focus(s);
    });
}

pub fn submit_goto_line() {
    let (input, tab, w_side, c_side) = app_signal().with_untracked(|s| {
        (
            s.goto_line_input.clone(),
            s.editor_tab,
            s.working_diff.as_ref().map(|w| w.active_side).unwrap_or(DiffSide::New),
            s.commit_diff.as_ref().map(|c| c.active_side).unwrap_or(DiffSide::New),
        )
    });
    let Some((line, col)) = paths::parse_goto_line_col(&input) else {
        show_toast("warn", "请输入有效行号（如 12 或 12:5）");
        return;
    };
    let pos = EditorPosition::new(line, col);
    set_text_input_focused(false);
    app_signal().update(|s| {
        s.goto_line_open = false;
        match tab {
            EditorTabKind::File => {
                if let Some(t) = s.code_tabs.get_mut(s.active_code_tab_idx) {
                    t.cursor = pos;
                }
                s.focus_zone = FocusZone::CodeEditor;
            }
            EditorTabKind::WorkingDiff => {
                if let Some(w) = &mut s.working_diff {
                    match w.active_side {
                        DiffSide::Old => w.old_cursor = pos,
                        DiffSide::New => w.new_cursor = pos,
                    }
                }
                s.focus_zone = match w_side {
                    DiffSide::Old => FocusZone::WorkingDiffOld,
                    DiffSide::New => FocusZone::WorkingDiffNew,
                };
            }
            EditorTabKind::CommitDiff => {
                if let Some(c) = &mut s.commit_diff {
                    match c.active_side {
                        DiffSide::Old => c.old_cursor = pos,
                        DiffSide::New => c.new_cursor = pos,
                    }
                }
                s.focus_zone = match c_side {
                    DiffSide::Old => FocusZone::CommitDiffOld,
                    DiffSide::New => FocusZone::CommitDiffNew,
                };
            }
            EditorTabKind::Comparison => restore_focus_after_modal(s),
            EditorTabKind::Welcome => {}
        }
    });

    match tab {
        EditorTabKind::File => reveal_in_code_editor(pos),
        EditorTabKind::WorkingDiff => reveal_in_active_diff(w_side, pos, None),
        EditorTabKind::CommitDiff => reveal_in_active_diff(c_side, pos, None),
        EditorTabKind::Comparison => {
            let focus = app_signal().with_untracked(|s| s.focus_zone);
            if let Some(api) = active_editor_api(focus) {
                api.set_selections(vec![EditorSelection::caret(pos)]);
                api.reveal_position(pos, true);
            }
        }
        EditorTabKind::Welcome => {}
    }
    focus_active_editor_ref();
}

#[derive(Clone, Default)]
struct PendingFileNavigation(Option<(String, String)>);

fn retry_file_navigation() {
    let mut pending = None;
    global::<PendingFileNavigation>().update(|request| pending = request.0.take());
    if let Some((workspace, query)) = pending
        && app_signal().with_untracked(|s| {
            s.cfg.workspace_path == workspace && s.picker_open == Some(PickerKind::GoToFile)
                && s.focus_zone == FocusZone::Modal && s.picker_query.trim() == query.trim()
        })
    {
        open_from_file_picker_input(&query);
    }
}

/// 处理转到文件（`Cmd+Shift+O`）输入，支持 `~/path:line:col` 和工作区相对路径搜索。
pub fn open_from_file_picker_input(raw_input: &str) {
    global::<PendingFileNavigation>().set(PendingFileNavigation::default());
    let parsed = parse_file_location(raw_input);
    if parsed.path.is_empty() {
        return;
    }
    let pos = parsed
        .line
        .map(|l| EditorPosition::new(l.saturating_sub(1), parsed.col.unwrap_or(1).saturating_sub(1)));
    let expanded = expand_tilde(&parsed.path);
    let expanded_path = PathBuf::from(&expanded);

    let matched = app_signal().with_untracked(|s| {
        if expanded_path.is_absolute() && expanded_path.exists() {
            let canon = std::fs::canonicalize(&expanded_path).unwrap_or(expanded_path.clone());
            if let Some(found) = s.workspace_files.iter().find(|f| {
                let fc = std::fs::canonicalize(&f.abs_path).unwrap_or_else(|_| PathBuf::from(&f.abs_path));
                fc == canon
            }) {
                return Some(found.clone());
            }
            // 也允许打开绝对路径位于某个仓库内的新文件
            for repo in &s.repos {
                let rc = std::fs::canonicalize(&repo.path).unwrap_or_else(|_| PathBuf::from(&repo.path));
                if let Ok(rel) = canon.strip_prefix(&rc) {
                    return Some(WorkspaceFile {
                        repo_id: repo.id.clone(),
                        repo_name: repo.name.clone(),
                        repo_path: repo.path.clone(),
                        rel_path: paths::normalize_slashes(&rel.to_string_lossy()),
                        abs_path: canon.to_string_lossy().to_string(),
                        color_idx: repo.color_idx,
                    });
                }
            }
        }
        let needle = parsed.path.to_ascii_lowercase();
        s.workspace_files
            .iter()
            .find(|f| {
                f.rel_path.to_ascii_lowercase() == needle
                    || format!("{}/{}", f.repo_name, f.rel_path).to_ascii_lowercase() == needle
            })
            .or_else(|| {
                s.workspace_files.iter().find(|f| {
                    f.rel_path.to_ascii_lowercase().contains(&needle)
                        || basename(&f.rel_path).to_ascii_lowercase().contains(&needle)
                })
            })
            .cloned()
    });

    if let Some(wf) = matched {
        set_text_input_focused(false);
        app_signal().update(|s| {
            s.picker_open = None;
            restore_focus_after_modal(s);
        });
        open_code_file(wf, pos, true);
    } else {
        let defer = app_signal().with_untracked(|s| {
            s.is_refreshing.then(|| (s.cfg.workspace_path.clone(), raw_input.to_owned()))
        });
        if let Some(request) = defer {
            global::<PendingFileNavigation>().set(PendingFileNavigation(Some(request)));
        } else {
            show_toast("warn", format!("未找到匹配文件: {}", parsed.path));
        }
    }
}

pub(crate) fn spawn_from_root(future: impl std::future::Future<Output = ()> + 'static) {
    if let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) {
        owner.run(|| spawn(future));
    } else {
        spawn(future);
    }
}

/// Run a Git write off the UI thread and refresh after releasing the workspace write guard.
pub(crate) fn run_git_action<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    complete: impl FnOnce(T) + 'static,
) -> bool {
    if app_signal().with_untracked(|s| s.command_busy) {
        show_toast("warn", "其他 Git 操作正在运行");
        return false;
    }
    app_signal().update(|s| s.command_busy = true);
    let task = async move {
        let result = blocking(work).await;
        app_signal().update(|s| {
            s.command_busy = false;
            s.pending_refresh = false;
            s.pending_refresh_repos.clear();
        });
        match result {
            Ok(value) => complete(value),
            Err(err) => show_toast("err", format!("任务失败：{err}")),
        }
        refresh_workspace_async(None);
    };
    spawn_from_root(task);
    true
}

/// 暂存或取消暂存指定文件（命令期间暂缓监听刷新，结束后补刷）。
pub fn stage_files_action(files: Vec<ChangedFile>, stage: bool) {
    if files.is_empty() {
        return;
    }
    let mut by_repo: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in &files {
        by_repo
            .entry(f.repo_path.clone())
            .or_default()
            .push(f.path.clone());
    }
    run_git_action(
        move || {
            for (repo_path, paths) in by_repo {
                git::stage_or_unstage_files(&repo_path, &paths, stage)?;
            }
            anyhow::Ok(())
        },
        move |res| match res {
            Ok(()) => show_toast("ok", if stage { "已暂存选中文件" } else { "已取消暂存" }),
            Err(err) => show_toast("err", format!("暂存操作失败：{err}")),
        },
    );
}

/// 同步版暂存（便于测试或直接按键即时反馈）。
pub fn stage_selected_change_sync(stage: bool) {
    if app_signal().with_untracked(|s| s.command_busy) {
        show_toast("warn", "其他 Git 操作正在运行");
        return;
    }
    let file = app_signal().with_untracked(|s| {
        s.change_selection
            .as_ref()
            .map(|sel| sel.file.clone())
            .or_else(|| s.working_diff.as_ref().map(|w| w.file.clone()))
    });
    let Some(cf) = file else {
        return;
    };
    app_signal().update(|s| s.command_busy = true);
    let res = git::stage_or_unstage_files(&cf.repo_path, &[cf.path.clone()], stage);
    app_signal().update(|s| {
        s.command_busy = false;
        s.pending_refresh = false;
    });
    match res {
        Ok(()) => {
            show_toast(
                "ok",
                if stage {
                    format!("已暂存 {}", cf.path)
                } else {
                    format!("已取消暂存 {}", cf.path)
                },
            );
            refresh_workspace_sync(Some(HashSet::from([cf.repo_id])));
        }
        Err(err) => show_toast("err", format!("暂存操作失败: {err}")),
    }
}

pub fn open_local_history_for_active() {
    crate::history_runtime::open_active();
}

pub fn open_file_revision_history(repo: RepoInfo, rev: String, rel_path: String) {
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let request_id = global::<FileHistoryRequest>().with_untracked(|request| request.0.wrapping_add(1));
    global::<FileHistoryRequest>().set(FileHistoryRequest(request_id));
    app_signal().update(|s| {
        s.show_git_panel = true;
        s.file_history = Some(FileRevisionHistory {
            repo: repo.clone(),
            rel_path: rel_path.clone(),
            rev: rev.clone(),
            commits: Vec::new(),
            paths: HashMap::new(),
            show_affected: false,
            loading: true,
            error: None,
        });
        s.selected_commit = None;
        s.selected_commit_file = None;
        s.selected_commit_files.clear();
    });

    crate::ui::content_search::hide_find_panel();
    crate::ui::git_pane::focus_history();

    owner.run(|| spawn(async move {
        let repo_id = repo.id.clone();
        let expected_rev = rev.clone();
        let expected_path = rel_path.clone();
        let outcome = blocking(move || -> anyhow::Result<_> {
            let commits = git::file_history_at_revision(&repo, &rev, &rel_path, 60)?;
            let paths = crate::file_revisions::list(Path::new(&repo.path), &rev, &rel_path, 60)?
                .into_iter()
                .map(|revision| (revision.hash, revision.path))
                .collect();
            Ok((commits, paths))
        }).await;
        if !global::<FileHistoryRequest>().with_untracked(|request| request.0 == request_id) {
            return;
        }
        let mut first = None;
        app_signal().update(|s| {
            let Some(history) = s.file_history.as_mut().filter(|history|
                s.cfg.workspace_path == workspace && history.repo.id == repo_id
                    && history.rel_path == expected_path && history.rev == expected_rev
            ) else { return };
            history.loading = false;
            match outcome {
                Ok(Ok((commits, paths))) => {
                    first = commits.first().cloned();
                    history.commits = commits;
                    history.paths = paths;
                }
                Ok(Err(err)) => history.error = Some(err.to_string()),
                Err(err) => history.error = Some(err.to_string()),
            }
        });
        if let Some(first) = first {
            crate::ui::git_pane::select_commit(first);
        }
    }));
}

#[derive(Clone, Default)]
struct FileHistoryRequest(u64);

// ============================================================================
// EditorRef 操控辅助
// ============================================================================

pub fn active_editor_api(zone: FocusZone) -> Option<Rc<EditorApi>> {
    let refs = editor_refs_signal().get_untracked();
    let single = app_signal().with_untracked(|s| s.cfg.diff_single_column);
    match zone {
        FocusZone::CodeEditor => {
            let focus = app_signal().with_untracked(|s| s.code_split.as_ref().map(|split| split.focused));
            match focus {
                Some(crate::editor_split::PaneId::Left) => refs.split_left,
                Some(crate::editor_split::PaneId::Right) => refs.split_right,
                None => refs.code,
            }.and_then(|reference| reference.api::<EditorApi>())
        },
        FocusZone::ComparisonOld => refs.compare_old.and_then(|reference| reference.api::<EditorApi>()),
        FocusZone::ComparisonNew => refs.compare_new.and_then(|reference| reference.api::<EditorApi>()),
        FocusZone::WorkingDiffOld => {
            if single {
                refs.work_unified.and_then(|r| r.api::<EditorApi>())
            } else {
                refs.work_old.and_then(|r| r.api::<EditorApi>())
            }
        }
        FocusZone::WorkingDiffNew => {
            if single {
                refs.work_unified.and_then(|r| r.api::<EditorApi>())
            } else {
                refs.work_new.and_then(|r| r.api::<EditorApi>())
            }
        }
        FocusZone::CommitDiffOld => {
            if single {
                refs.commit_unified.and_then(|r| r.api::<EditorApi>())
            } else {
                refs.commit_old.and_then(|r| r.api::<EditorApi>())
            }
        }
        FocusZone::CommitDiffNew => {
            if single {
                refs.commit_unified.and_then(|r| r.api::<EditorApi>())
            } else {
                refs.commit_new.and_then(|r| r.api::<EditorApi>())
            }
        }
        _ => None,
    }
}

pub fn focus_active_editor_ref() {
    let zone = app_signal().with_untracked(|s| s.focus_zone);
    if let Some(api) = active_editor_api(zone) {
        api.focus();
        request_shortcut_rebind();
    } else if matches!(
        zone,
        FocusZone::CodeEditor
            | FocusZone::WorkingDiffOld
            | FocusZone::WorkingDiffNew
            | FocusZone::CommitDiffOld
            | FocusZone::CommitDiffNew
    ) {
        set_timeout(Duration::from_millis(0), move || {
            let still_same = app_signal().with_untracked(|s| s.focus_zone == zone)
                && !text_input_focus_signal().with_untracked(|f| f.focused);
            if still_same {
                if let Some(api) = active_editor_api(zone) {
                    api.focus();
                    request_shortcut_rebind();
                }
            }
        });
    }
}

pub fn reveal_in_code_editor(pos: EditorPosition) {
    app_signal().update(|s| crate::code_split_runtime::set_active_position(s, pos));
    if let Some(api) = active_editor_api(FocusZone::CodeEditor) {
        api.set_selections(vec![EditorSelection::caret(pos)]);
        api.reveal_position(pos, true);
    }
}

pub fn reveal_in_active_diff(side: DiffSide, pos: EditorPosition, scroll_y: Option<f32>) {
    let (tab, effective_pos) = app_signal().with_untracked(|s| {
        let single = s.cfg.diff_single_column;
        let mapped_pos = if single {
            let u_line = match s.editor_tab {
                EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|w| {
                    map_side_line_to_unified(
                        &w.data.old_text,
                        &w.current_new_text,
                        &w.computed.hunks,
                        side,
                        pos.line as usize,
                    )
                }),
                EditorTabKind::CommitDiff => s.commit_diff.as_ref().map(|c| {
                    map_side_line_to_unified(
                        &c.data.old_text,
                        &c.data.new_text,
                        &c.computed.hunks,
                        side,
                        pos.line as usize,
                    )
                }),
                _ => None,
            };
            match u_line {
                Some(line) => EditorPosition::new(line as u32, pos.column.saturating_add(2)),
                None => pos,
            }
        } else {
            pos
        };
        (s.editor_tab, mapped_pos)
    });
    let zone = match (tab, side) {
        (EditorTabKind::WorkingDiff, DiffSide::Old) => FocusZone::WorkingDiffOld,
        (EditorTabKind::WorkingDiff, DiffSide::New) => FocusZone::WorkingDiffNew,
        (EditorTabKind::CommitDiff, DiffSide::Old) => FocusZone::CommitDiffOld,
        (EditorTabKind::CommitDiff, DiffSide::New) => FocusZone::CommitDiffNew,
        _ => return,
    };
    if let Some(api) = active_editor_api(zone) {
        api.set_selections(vec![EditorSelection::caret(effective_pos)]);
        if let Some(y) = scroll_y {
            if y > 0.0 {
                api.set_scroll_offset(point(px(0.0), px(y)));
            } else {
                api.reveal_position(effective_pos, true);
            }
        } else {
            api.reveal_position(effective_pos, true);
        }
    }
}
