use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_base::{EditorApi, EditorPosition, EditorSearch, EditorSearchResult, EditorSelection};
use nsh_base::editor::EditorUndoHistory;
use nsh_core::{element::after_answer, prelude::*};

use crate::config::{self, Config};
use crate::editor_split::EditorSplit;
use crate::diff_model::{
    ComputedDiff, DiffSide, InlineHighlightMode, WhitespaceMode, compute_diff,
    map_editable_side_line_to_unified, map_side_line_to_unified, source_position,
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

mod diff_open;
mod diff_viewport;
mod navigation;
mod refresh;
mod source_open;
mod workspace_refresh;
pub use diff_open::{move_commit_diff_file, open_commit_diff};
pub(crate) use diff_open::{cancel_diff_read, diff_read_pending, preview_commit_diff, set_commit_diff_preview};
use diff_open::open_working_diff_with_hunk;
pub use diff_viewport::{DiffViewport, jump_diff_hunk};
use diff_viewport::reveal_opened_diff_hunk;
pub(crate) use diff_viewport::{copy_diff_selection, focus_diff_side, record_diff_caret, restore_diff_view_after_render, sync_diff_scroll, toggle_diff_layout};
use navigation::navigate_history;
pub use navigation::{open_revision_code_file, restore_nav_target};
pub use source_open::{open_code_file, open_code_file_preview};
pub(crate) use source_open::prepare_workspace_edit_tab;
pub use workspace_refresh::refresh_workspace_async;
pub(crate) use workspace_refresh::{WorkspaceData, WorkspaceNotice, refresh_workspace_on_focus};

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
    FindResults,
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
        revision: String,
        pos: EditorPosition,
        scroll_y: i32,
    },
    WorkingDiff {
        file_key: String,
        staged: bool,
        side: DiffSide,
        pos: EditorPosition,
        viewport: Option<DiffViewport>,
    },
    CommitDiff {
        repo_id: String,
        commit_hash: String,
        file_path: String,
        scope: CommitDiffScope,
        side: DiffSide,
        pos: EditorPosition,
        viewport: Option<DiffViewport>,
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
    pub(crate) disk_state: ReviewDiskState,
    /// 未钉住的预览标签：斜体显示，可被后续预览复用替换；编辑或双击后转为普通标签
    pub preview: bool,
    /// 递增版本号，用于受控同步外部刷新/恢复内容到 Editor
    pub content_rev: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ReviewDiskState {
    #[default]
    Current,
    Modified,
    Missing,
    Unreadable,
}

impl ReviewDiskState {
    fn for_diff(data: &ReviewChangeData) -> Self {
        if !data.staged_only && !data.version.exists {
            Self::Missing
        } else {
            Self::Current
        }
    }

    pub(crate) fn notice(self) -> Option<&'static str> {
        match self {
            Self::Current => None,
            Self::Modified => Some("磁盘文件已修改"),
            Self::Missing => Some("磁盘文件已移动或删除"),
            Self::Unreadable => Some("磁盘文件暂不可读"),
        }
    }
}

impl CodeTab {
    fn from_read(file: WorkspaceFile, revision: String, read: git::WorktreeRead, cursor: EditorPosition, preview: bool) -> Self {
        Self {
            id: code_tab_id(&file.repo_id, &file.rel_path, &revision),
            file,
            revision,
            text: read.text.clone(),
            saved_text: read.text,
            line_ending: read.line_ending,
            version: read.version,
            editable: read.editable,
            readonly_reason: read.readonly_reason,
            binary: read.binary,
            too_large: read.too_large,
            cursor,
            scroll_y: 0.0,
            scroll_x: 0.0,
            preview,
            disk_state: ReviewDiskState::Current,
            content_rev: 1,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.editable && self.revision.is_empty() && self.text != self.saved_text
    }

    pub(crate) fn editor_editable(&self) -> bool {
        self.editable && (self.is_dirty() || !matches!(self.disk_state, ReviewDiskState::Missing | ReviewDiskState::Unreadable))
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
    viewport: Option<DiffViewport>,
    pub boundary_hint: Option<i32>, // -1 上边界，+1 下边界
    pub(crate) disk_state: ReviewDiskState,
    pub(crate) git_baseline_issue: Option<GitBaselineIssue>,
    pub content_rev: u64,
    pub(crate) undo_history: DocumentUndo,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentUndo(Arc<Mutex<Option<EditorUndoHistory>>>);

impl PartialEq for DocumentUndo {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl DocumentUndo {
    pub(crate) fn park(&self, editor: &EditorApi) {
        if let Some(history) = editor.take_undo_history() {
            let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if slot.is_none() {
                *slot = Some(history);
            }
        }
    }

    pub(crate) fn resume(&self, editor: &EditorApi) {
        let history = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(history) = history {
            editor.restore_undo_history(history);
        }
    }

    fn record_replacement(&self, before: &str, after: &str, caret: EditorPosition) {
        let mut history = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *history = EditorUndoHistory::record_replacement(history.take(), before, after, caret);
    }

    pub(crate) fn record_source_replacement(&self, before: &str, after: &str) {
        let mut history = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *history = EditorUndoHistory::record_source_replacement(history.take(), before, after);
    }

    pub(crate) fn step(&self, expected: &str, redo: bool) -> Option<(String, EditorPosition)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).as_mut()?.step(expected, redo)
    }

    fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[derive(Clone)]
pub(crate) struct ParkedWorkingDiffUndo {
    repo_id: String,
    repo_path: String,
    path: String,
    version: ReviewVersion,
    history: DocumentUndo,
}

impl ParkedWorkingDiffUndo {
    fn matches_file(&self, file: &ChangedFile) -> bool {
        self.repo_id == file.repo_id && self.repo_path == file.repo_path && self.path == file.path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitBaselineIssue {
    ReadFailed,
    TextBlocked(git::ReviewTextBlock),
}

impl WorkingDiffSession {
    fn replace_external_text(&mut self, text: String) {
        if text != self.current_new_text {
            self.undo_history.clear();
            self.current_new_text = text;
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.data.editable && !self.data.staged_only && self.current_new_text != self.saved_new_text
    }

    pub(crate) fn editor_editable(&self) -> bool {
        self.data.editable && !self.data.staged_only
            && (self.is_dirty() || self.read_failure_notice().is_none())
    }

    pub(crate) fn read_failure_notice(&self) -> Option<&'static str> {
        match self.git_baseline_issue {
            Some(GitBaselineIssue::ReadFailed) => Some("Git 比较基准暂不可读"),
            Some(GitBaselineIssue::TextBlocked(git::ReviewTextBlock::Binary)) => {
                Some("Git 比较基准为二进制或非 UTF-8 文件")
            }
            Some(GitBaselineIssue::TextBlocked(git::ReviewTextBlock::TooLarge)) => {
                Some("Git 比较基准超过 2 MiB 审阅上限")
            }
            None if self.disk_state == ReviewDiskState::Unreadable => self.disk_state.notice(),
            None => None,
        }
    }

    pub(crate) fn can_compare_text(&self) -> bool {
        self.data.can_compare_text() && self.read_failure_notice().is_none()
    }

    pub(crate) fn recompute_diff(&mut self, mode: WhitespaceMode) {
        let draft = self.is_dirty().then_some(self.current_new_text.as_str());
        self.computed = compute_working_text_diff(&self.data, draft, mode);
    }

    fn update_draft_comparison(&mut self, mode: WhitespaceMode, was_dirty: bool) -> Option<String> {
        self.recompute_diff(mode);
        self.boundary_hint = None;
        (was_dirty && !self.is_dirty()
            && matches!(self.git_baseline_issue, Some(GitBaselineIssue::TextBlocked(_))))
            .then(|| self.file.repo_id.clone())
    }

    pub fn active_cursor(&self) -> EditorPosition {
        match self.active_side {
            DiffSide::Old => self.old_cursor,
            DiffSide::New => self.new_cursor,
        }
    }

    pub(crate) fn active_text(&self) -> &str {
        match self.active_side {
            DiffSide::Old => &self.data.old_text,
            DiffSide::New => &self.current_new_text,
        }
    }
}

fn compute_working_text_diff(
    data: &ReviewChangeData,
    draft: Option<&str>,
    mode: WhitespaceMode,
) -> ComputedDiff {
    if data.old_text_block.is_some() || (data.new_text_block.is_some() && draft.is_none()) {
        return ComputedDiff::default();
    }
    compute_diff(&data.old_text, draft.unwrap_or(&data.new_text), mode)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CommitDiffScope {
    #[default]
    AllFiles,
    SingleFile,
}

#[derive(Default)]
struct CommitDiffOpenOptions {
    scope: CommitDiffScope,
    focus_diff: bool,
    land_on_last_hunk: bool,
    record_navigation: bool,
    select_log_file: bool,
}

#[derive(Debug, Clone)]
pub struct CommitDiffSession {
    pub commit: CommitEntry,
    pub file: CommitFileEntry,
    pub data: CommitDiffData,
    pub computed: ComputedDiff,
    pub(crate) scope: CommitDiffScope,
    pub active_side: DiffSide,
    pub old_cursor: EditorPosition,
    pub new_cursor: EditorPosition,
    viewport: Option<DiffViewport>,
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
    head_seen: Option<git::RepositoryHead>,
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
    pub(crate) code_find_input: Option<ElementRef>,
    pub(crate) diff_find_input: Option<ElementRef>,
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Config,
    pub repos: Vec<RepoInfo>,
    pub(crate) hidden_repo_labels: BTreeMap<String, String>,
    pub statuses: Vec<RepoStatus>,
    pub workspace_files: Arc<[WorkspaceFile]>,
    pub stashes: Vec<StashEntry>,

    // 布局与工具窗口状态
    pub left_tab: LeftTab,
    pub show_left_panel: bool,
    pub show_git_panel: bool,
    pub editor_tab: EditorTabKind,
    pub focus_zone: FocusZone,
    pub prev_focus_zone: FocusZone,

    // Commit 树状态（选中与提交勾选独立）
    pub tree_filter_open: Option<LeftTab>,
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
    pub(crate) project_reveal_request: u64,
    pub project_selected_file: Option<WorkspaceFile>,
    pub project_selected_dir: Option<(String, String)>, // (repo_id, rel_dir)

    // Git 历史面板状态
    pub selected_commit: Option<CommitEntry>,
    pub selected_commit_files: Vec<CommitFileEntry>,
    pub selected_commit_file: Option<CommitFileEntry>,
    pub file_history: Option<FileRevisionHistory>,

    // 统一编辑区状态
    pub code_tabs: Vec<CodeTab>,
    pub(crate) code_split: Option<EditorSplit>,
    pub(crate) code_undo: HashMap<String, DocumentUndo>,
    pub active_code_tab_idx: usize,
    pub working_diff: Option<WorkingDiffSession>,
    pub(crate) parked_working_diff_undo: Vec<ParkedWorkingDiffUndo>,
    pub commit_diff: Option<CommitDiffSession>,
    pub show_code_options: bool,

    // 查找替换条状态
    pub diff_find_open: bool,
    pub(crate) diff_search: EditorSearch,
    pub diff_find_side: DiffSide,
    pub(crate) diff_find_matches: EditorSearchResult,
    pub(crate) comparison_find_open: bool,
    pub(crate) comparison_find_side: DiffSide,
    pub code_find_open: bool,
    pub code_replace_open: bool,
    pub(crate) code_search: EditorSearch,
    pub code_replace_text: String,
    pub(crate) code_find_matches: EditorSearchResult,

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
    pub command_busy: bool,
    workspace_refresh: workspace_refresh::WorkspaceRefresh,
    pub last_refresh_ms: u64,
    pub window_focused: bool,
}

impl AppState {
    pub(crate) fn dismiss_tree_filter(&mut self) {
        match self.tree_filter_open.take() {
            Some(LeftTab::Project) => self.project_filter.clear(),
            Some(LeftTab::Commit) => self.changes_filter.clear(),
            None => {}
        }
    }

    pub(crate) fn active_find_open(&self) -> bool {
        match self.editor_tab {
            EditorTabKind::File => self.code_find_open,
            EditorTabKind::WorkingDiff | EditorTabKind::CommitDiff => self.diff_find_open,
            EditorTabKind::Comparison => self.comparison_find_open,
            _ => false,
        }
    }

    pub(crate) fn diff_find_side_for(&self, tab: EditorTabKind) -> Option<DiffSide> {
        match tab {
            EditorTabKind::Comparison => Some(self.comparison_find_side),
            _ if self.cfg.diff_single_column => None,
            _ => Some(self.diff_find_side),
        }
    }

    fn close_editor_find(&mut self) -> bool {
        if !self.active_find_open() {
            return false;
        }
        match self.editor_tab {
            EditorTabKind::File => {
                self.code_find_open = false;
                self.code_replace_open = false;
            }
            EditorTabKind::Comparison => self.comparison_find_open = false,
            _ => self.diff_find_open = false,
        }
        true
    }

    pub(crate) fn apply_working_diff_edit(
        &mut self,
        history: &DocumentUndo,
        text: String,
    ) -> Option<String> {
        let diff = self.working_diff.as_mut().filter(|diff| {
            &diff.undo_history == history && diff.editor_editable() && diff.current_new_text != text
        })?;
        let was_dirty = diff.is_dirty();
        let before = std::mem::replace(&mut diff.current_new_text, text);
        let refresh = diff.update_draft_comparison(
            WhitespaceMode::from_config(&self.cfg.diff_whitespace_mode), was_dirty,
        );
        for tab in self.code_tabs.iter_mut().filter(|tab| {
            tab.text == before && same_working_document(tab, &diff.file, &diff.data)
        }) {
            self.code_undo.insert(tab.id.clone(), history.clone());
            tab.text.clone_from(&diff.current_new_text);
            tab.preview &= !tab.is_dirty();
            tab.content_rev += 1;
        }
        refresh
    }

    pub(crate) fn mirror_source_edit(&mut self, id: &str, before: &str, history: &DocumentUndo) -> Option<String> {
        let tab = self.code_tabs.iter().find(|tab| tab.id == id)?;
        let diff = self.working_diff.as_mut().filter(|diff| {
            diff.current_new_text == before && same_working_document(tab, &diff.file, &diff.data)
        })?;
        let was_dirty = diff.is_dirty();
        diff.undo_history = history.clone();
        diff.current_new_text.clone_from(&tab.text);
        diff.content_rev += 1;
        diff.update_draft_comparison(WhitespaceMode::from_config(&self.cfg.diff_whitespace_mode), was_dirty)
    }

    fn working_diff_undo_for(
        &mut self,
        file: &ChangedFile,
        data: &ReviewChangeData,
        text: &str,
    ) -> DocumentUndo {
        const MAX_PARKED_HISTORIES: usize = 8;
        let source_history = self.code_tabs.iter()
            .find(|tab| tab.text == text && same_working_document(tab, file, data))
            .and_then(|tab| self.code_undo.get(&tab.id)).cloned();

        if let Some(active) = &self.working_diff {
            let same_file = active.file.repo_id == file.repo_id
                && active.file.repo_path == file.repo_path
                && active.file.path == file.path;
            if same_file && active.data.staged_only == data.staged_only {
                return if active.current_new_text == text
                    && !refresh::content_changed(&active.data.version, &data.version)
                {
                    active.undo_history.clone()
                } else {
                    source_history.unwrap_or_default()
                };
            }
            if active.data.editable && !active.data.staged_only && !active.is_dirty() {
                let parked = &mut self.parked_working_diff_undo;
                parked.retain(|entry| !entry.matches_file(&active.file));
                // The editor puts its history in this slot when the viewer unmounts.
                parked.insert(0, ParkedWorkingDiffUndo {
                    repo_id: active.file.repo_id.clone(),
                    repo_path: active.file.repo_path.clone(),
                    path: active.file.path.clone(),
                    version: active.data.version.clone(),
                    history: active.undo_history.clone(),
                });
            }
        }

        let parked = &mut self.parked_working_diff_undo;
        let restored = if data.staged_only {
            None
        } else {
            parked.iter().position(|entry| entry.matches_file(file))
                .map(|index| parked.remove(index))
                .filter(|entry| data.editable && entry.version == data.version)
        };
        parked.truncate(MAX_PARKED_HISTORIES);
        source_history.or_else(|| restored.map(|entry| entry.history)).unwrap_or_default()
    }

    pub(crate) fn repository_head(&self, repo_path: &str) -> git::RepositoryHead {
        self.statuses
            .iter()
            .find(|status| status.repo.path == repo_path)
            .map(|status| status.head.clone())
            .unwrap_or_default()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::for_config(config::load())
    }
}

impl AppState {
    pub(crate) fn for_config(cfg: Config) -> Self {
        Self {
            cfg,
            repos: Vec::new(),
            hidden_repo_labels: BTreeMap::new(),
            statuses: Vec::new(),
            workspace_files: Arc::default(),
            stashes: Vec::new(),
            left_tab: LeftTab::Commit,
            show_left_panel: true,
            show_git_panel: false,
            editor_tab: EditorTabKind::Welcome,
            focus_zone: FocusZone::CommitTree,
            prev_focus_zone: FocusZone::CommitTree,
            tree_filter_open: None,
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
            project_reveal_request: 0,
            project_selected_file: None,
            project_selected_dir: None,
            selected_commit: None,
            selected_commit_files: Vec::new(),
            selected_commit_file: None,
            file_history: None,
            code_tabs: Vec::new(),
            code_split: None,
            code_undo: HashMap::new(),
            active_code_tab_idx: 0,
            working_diff: None,
            parked_working_diff_undo: Vec::new(),
            commit_diff: None,
            show_code_options: false,
            diff_find_open: false,
            diff_search: EditorSearch::default(),
            diff_find_side: DiffSide::New,
            diff_find_matches: EditorSearchResult::default(),
            comparison_find_open: false,
            comparison_find_side: DiffSide::New,
            code_find_open: false,
            code_replace_open: false,
            code_search: EditorSearch::default(),
            code_replace_text: String::new(),
            code_find_matches: EditorSearchResult::default(),
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
            command_busy: false,
            workspace_refresh: workspace_refresh::WorkspaceRefresh::default(),
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

fn same_working_document(tab: &CodeTab, file: &ChangedFile, data: &ReviewChangeData) -> bool {
    !data.staged_only && data.editable && tab.revision.is_empty() && tab.editor_editable()
        && tab.file.repo_id == file.repo_id && tab.file.repo_path == file.repo_path
        && tab.file.rel_path == file.path && tab.version == data.version
}

/// Keep a previously opened working comparison aligned when activating it.
pub fn sync_dirty_code_tab_into_working_diff(s: &mut AppState) {
    let ws_mode = WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode);
    let Some(w) = &mut s.working_diff else {
        return;
    };
    if !w.editor_editable() {
        return;
    }
    if let Some(tab) = s.code_tabs.iter().find(|t| same_working_document(t, &w.file, &w.data)) {
        if tab.is_dirty()
            && w.current_new_text != tab.text
        {
            w.undo_history = s.code_undo.entry(tab.id.clone()).or_default().clone();
            w.current_new_text.clone_from(&tab.text);
            w.recompute_diff(ws_mode);
            w.content_rev += 1;
        }
        if w.current_new_text == tab.text {
            s.code_undo.insert(tab.id.clone(), w.undo_history.clone());
        }
    }
}

/// Join a source tab to the same working document, including its undo history.
pub fn sync_dirty_working_diff_into_code_tab(s: &mut AppState, tab_idx: usize) {
    let Some(w) = s.working_diff.as_ref() else {
        return;
    };
    if let Some(tab) = s.code_tabs.get_mut(tab_idx).filter(|tab| same_working_document(tab, &w.file, &w.data)) {
        if w.is_dirty() && tab.text != w.current_new_text {
            tab.text.clone_from(&w.current_new_text);
            tab.preview = false;
            tab.content_rev += 1;
        }
        if tab.text == w.current_new_text {
            s.code_undo.insert(tab.id.clone(), w.undo_history.clone());
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
                        if app_signal().with_untracked(|s| s.cfg.workspace_path == ws) {
                            handle_watch_hint(hint);
                        }
                    }
                });
            }
            Err(err) => {
                log::warn!("启动文件监听失败: {err}");
            }
        }
    });

    // 当 repos 更新时热更新 watcher 内部的仓库列表
    let repos = computed(|| app_signal().with(|s| s.repos.clone()));
    effect(move || {
        let repos = repos.get();
        holder.with(|h| {
            if let Some(w) = &h.watcher {
                w.update_repos(repos);
            }
        });
    });

    let pending_history_head = computed(|| app_signal().with(|s| {
        let history = s.file_history.as_ref().filter(|_| s.show_git_panel)?;
        file_history_head(s, &history.rev, &history.repo.path).filter(|head| {
            *head != git::RepositoryHead::Unavailable && history.head_seen.as_ref() != Some(head)
        })
    }));
    effect(move || {
        if pending_history_head.get().is_some() {
            load_file_revision_history(true);
        }
    });
}

fn handle_watch_hint(hint: WatchHint) {
    if hint.is_empty() {
        return;
    }
    if !hint.history_repo_ids.is_empty() {
        crate::ui::repository_history_changed(&hint.history_repo_ids);
    }
    let repos = (!hint.mani_changed && !hint.repo_ids.is_empty()).then_some(hint.repo_ids);
    workspace_refresh::refresh_on_watch(repos);
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

pub(crate) fn change_in_section(file: &ChangedFile, staged: bool) -> bool {
    if staged {
        file.staged
    } else {
        file.unstaged || file.untracked || !file.staged
    }
}

pub(crate) fn set_change_tree_expanded(expanded: bool) {
    app_signal().update(|s| {
        s.collapsed_staged_section = !expanded;
        s.collapsed_unstaged_section = !expanded;
        s.collapsed_change_repos.clear();
        s.collapsed_change_dirs.clear();
        if expanded {
            return;
        }
        for status in &s.statuses {
            for file in &status.files {
                for staged in [true, false].into_iter().filter(|&side| change_in_section(file, side)) {
                    let section = if staged { "staged" } else { "unstaged" };
                    let repo_key = format!("{section}::{}", status.repo.id);
                    for parent in Path::new(&file.path).ancestors().skip(1)
                        .take_while(|path| !path.as_os_str().is_empty())
                    {
                        let name = paths::normalize_slashes(&parent.to_string_lossy());
                        s.collapsed_change_dirs.insert(format!("{repo_key}::{name}"));
                    }
                    s.collapsed_change_repos.insert(repo_key);
                }
            }
        }
    });
}

pub fn visible_change_items(s: &AppState) -> Vec<ChangeSelection> {
    let q = s.changes_filter.trim().to_ascii_lowercase();
    let mut out = Vec::new();
    let matches_query = |f: &ChangedFile| {
        s.cfg.matches_change_list(&f.key()) && (q.is_empty()
            || f.path.to_ascii_lowercase().contains(&q)
            || f.repo_name.to_ascii_lowercase().contains(&q))
    };

    for (staged, collapsed) in [(true, s.collapsed_staged_section), (false, s.collapsed_unstaged_section)] {
        if collapsed {
            continue;
        }
        let section = if staged { "staged" } else { "unstaged" };
        let groups: Vec<_> = s.statuses.iter().filter_map(|status| {
            let files: Vec<_> = status.files.iter()
                .filter(|file| change_in_section(file, staged) && matches_query(file))
                .cloned().collect();
            (!files.is_empty()).then_some((&status.repo.id, files))
        }).collect();
        let multi_repo = groups.len() > 1;
        for (repo_id, files) in groups {
            let repo_key = format!("{section}::{repo_id}");
            if multi_repo && s.collapsed_change_repos.contains(&repo_key) {
                continue;
            }
            push_rendered_change_files(
                &files, s.cfg.changes_flat, &repo_key, &s.collapsed_change_dirs, staged, &mut out,
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
        s.change_selected_dir = None;
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

fn save_before_switching_working_diff(
    file_key: &str,
    staged_only: bool,
    resume: impl FnOnce() + 'static,
) -> bool {
    let needs_save = app_signal().with_untracked(|s| {
        s.working_diff.as_ref().is_some_and(|diff| {
            diff.is_dirty() && (diff.file.key() != file_key || diff.data.staged_only != staged_only)
        })
    });
    if !needs_save {
        return true;
    }
    if let Some(target) = crate::format_runtime::working_target()
        && crate::format_runtime::save_if_enabled(vec![target], resume)
    {
        return false;
    }
    save_working_diff_now()
}

fn install_working_diff(
    file: ChangedFile,
    data: ReviewChangeData,
    draft: Option<String>,
    computed: ComputedDiff,
    focus_diff: bool,
    land_on_last_hunk: bool,
) {
    capture_editor_views();
    push_nav_back_current();

    let staged_only = data.staged_only;
    let saved_new_text = data.new_text.clone();
    let current_new_text = draft.unwrap_or_else(|| saved_new_text.clone());
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
        let undo_history = s.working_diff_undo_for(&file, &data, &current_new_text);
        s.working_diff = Some(WorkingDiffSession {
            file: file.clone(),
            disk_state: ReviewDiskState::for_diff(&data),
            git_baseline_issue: None,
            data,
            current_new_text,
            saved_new_text,
            computed,
            active_side: DiffSide::New,
            old_cursor: EditorPosition::new(initial_old_line, 0),
            new_cursor: EditorPosition::new(initial_hunk_line, 0),
            viewport: None,
            boundary_hint: None,
            content_rev: next_rev,
            undo_history,
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
        viewport: None,
    });

    if focus_diff {
        focus_active_editor_ref();
    }
    if has_hunks {
        reveal_opened_diff_hunk(EditorTabKind::WorkingDiff, initial_hunk_idx);
    }
}

fn install_commit_diff(
    commit: CommitEntry,
    file: CommitFileEntry,
    data: CommitDiffData,
    computed: ComputedDiff,
    options: CommitDiffOpenOptions,
) {
    if options.record_navigation {
        capture_editor_views();
        push_nav_back_current();
    }
    let initial_hunk_idx = if options.land_on_last_hunk {
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
        if options.select_log_file && crate::ui::git_pane::file_is_visible(s, &commit, &file) {
            s.selected_commit_file = Some(file.clone());
        }
        let next_rev = s.commit_diff.as_ref().map(|c| c.content_rev + 1).unwrap_or(1);
        s.commit_diff = Some(CommitDiffSession {
            commit: commit.clone(),
            file: file.clone(),
            data,
            computed,
            scope: options.scope,
            active_side: default_side,
            old_cursor: EditorPosition::new(initial_old_line, 0),
            new_cursor: EditorPosition::new(initial_line, 0),
            viewport: None,
            boundary_hint: None,
            content_rev: next_rev,
        });
        s.editor_tab = EditorTabKind::CommitDiff;
        if options.focus_diff {
            s.focus_zone = match default_side {
                DiffSide::Old => FocusZone::CommitDiffOld,
                DiffSide::New => FocusZone::CommitDiffNew,
            };
        }
    });

    if options.record_navigation {
        record_recent_location(NavTarget::CommitDiff {
            repo_id: commit.repo_id.clone(),
            commit_hash: commit.hash.clone(),
            file_path: file.path.clone(),
            scope: options.scope,
            side: default_side,
            pos: EditorPosition::new(initial_line, 0),
            viewport: None,
        });
    }

    if options.focus_diff {
        focus_active_editor_ref();
    }
    if has_hunks && options.record_navigation {
        reveal_opened_diff_hunk(EditorTabKind::CommitDiff, initial_hunk_idx);
    }
}

// ============================================================================
// 源码标签打开、跳转源码、往返与关闭保护
// ============================================================================

/// 从当前 Diff（工作区或历史提交）按 `F4` / `Cmd+Down` 跳转到源码页并定位对应行。
/// 若工作区 Diff 存在未保存轻改，将脏缓冲内容无缝同步到目标源码标签，避免编辑丢失或双缓冲分裂。
pub fn jump_from_diff_to_source() {
    jump_to_diff_source(None);
}

pub(crate) fn jump_from_diff_line(tab: EditorTabKind, side: DiffSide, position: EditorPosition) {
    jump_to_diff_source(Some((tab, side, position)));
}

fn jump_to_diff_source(clicked: Option<(EditorTabKind, DiffSide, EditorPosition)>) {
    let position = |side, cursor| clicked.map_or((side, cursor), |(_, side, cursor)| (side, cursor));
    let target = app_signal().with_untracked(|s| match s.editor_tab {
        tab if clicked.is_some_and(|(expected, _, _)| expected != tab) => None,
        EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|w| {
            let (side, caret) = position(w.active_side, w.active_cursor());
            let mapped = source_position(&w.current_new_text, &w.computed.hunks, side, caret);
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
            (wf, mapped, true)
        }),
        EditorTabKind::CommitDiff => s.commit_diff.as_ref().map(|c| {
            let (side, caret) = position(c.active_side, c.active_cursor());
            let mapped = source_position(&c.data.new_text, &c.computed.hunks, side, caret);
            let abs = Path::new(&c.commit.repo_path)
                .join(&c.data.new_path)
                .to_string_lossy()
                .to_string();
            let wf = WorkspaceFile {
                repo_id: c.commit.repo_id.clone(),
                repo_name: c.commit.repo_name.clone(),
                repo_path: c.commit.repo_path.clone(),
                rel_path: c.data.new_path.clone(),
                abs_path: abs,
                color_idx: s
                    .repos
                    .iter()
                    .find(|r| r.id == c.commit.repo_id)
                    .map(|r| r.color_idx)
                    .unwrap_or(0),
            };
            (wf, mapped, false)
        }),
        _ => None,
    });

    if let Some((file, pos, from_working_diff)) = target {
        if from_working_diff {
            source_open::open_from_working_diff(file, pos);
        } else {
            open_code_file(file, Some(pos), true);
        }
    }
}

pub fn navigate_back() {
    navigate_history(false);
}

pub fn navigate_forward() {
    navigate_history(true);
}

pub(crate) fn capture_editor_views() {
    crate::code_split_runtime::capture_views();
    diff_viewport::capture_diff_view();
}

fn current_nav_target() -> Option<NavTarget> {
    app_signal().with_untracked(|s| match s.editor_tab {
        EditorTabKind::File => s.code_tabs.get(s.active_code_tab_idx).map(|t| {
            NavTarget::CodeFile {
                repo_id: t.file.repo_id.clone(),
                rel_path: t.file.rel_path.clone(),
                revision: t.revision.clone(),
                pos: t.cursor,
                scroll_y: t.scroll_y.round() as i32,
            }
        }),
        EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|w| NavTarget::WorkingDiff {
            file_key: w.file.key(),
            staged: w.data.staged_only,
            side: w.active_side,
            pos: w.active_cursor(),
            viewport: w.viewport,
        }),
        EditorTabKind::CommitDiff => s.commit_diff.as_ref().map(|c| NavTarget::CommitDiff {
            repo_id: c.commit.repo_id.clone(),
            commit_hash: c.commit.hash.clone(),
            file_path: c.file.path.clone(),
            scope: c.scope,
            side: c.active_side,
            pos: c.active_cursor(),
            viewport: c.viewport,
        }),
        EditorTabKind::Welcome | EditorTabKind::Comparison => None,
    })
}

fn push_nav_back_current() {
    if let Some(cur) = current_nav_target() {
        app_signal().update(|s| push_nav_back(s, cur));
    }
}

fn push_nav_back(s: &mut AppState, target: NavTarget) {
    if s.nav_back.last() != Some(&target) {
        s.nav_back.push(target);
        if s.nav_back.len() > 64 {
            s.nav_back.remove(0);
        }
    }
    s.nav_forward.clear();
}

pub(crate) fn code_tab_id(repo_id: &str, rel_path: &str, revision: &str) -> String {
    if revision.is_empty() {
        format!("file::{repo_id}::{rel_path}")
    } else {
        format!("rev::{repo_id}::{revision}::{rel_path}")
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

pub(crate) fn edit_working_diff_text(file_key: &str, text: &str) {
    let Some((history, before, caret, right_visible)) = app_signal().with_untracked(|s| {
        let diff = s.working_diff.as_ref().filter(|diff| {
            diff.file.key() == file_key && diff.editor_editable() && diff.current_new_text != text
        })?;
        Some((
            diff.undo_history.clone(),
            diff.current_new_text.clone(),
            diff.new_cursor,
            s.editor_tab == EditorTabKind::WorkingDiff,
        ))
    }) else {
        return;
    };

    let editor = right_visible
        .then(|| active_editor_api(FocusZone::WorkingDiffNew))
        .flatten()
        .filter(|api| api.source_value() == before);
    if let Some(editor) = editor {
        editor.set_source_value(text.to_owned());
    } else {
        history.record_replacement(&before, text, caret);
    }

    let mut refresh = None;
    app_signal().update(|s| {
        refresh = s.apply_working_diff_edit(&history, text.to_owned());
        if let Some(diff) = &mut s.working_diff
            && diff.undo_history == history
        {
            diff.content_rev += 1;
        }
    });
    if let Some(repo) = refresh {
        refresh_workspace_async(Some(HashSet::from([repo])));
    }
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
                w.editor_editable(),
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
                        w.disk_state = ReviewDiskState::Current;
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
                        t.disk_state = ReviewDiskState::Current;
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
                t.editor_editable() && t.revision.is_empty(),
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
                    t.disk_state = ReviewDiskState::Current;
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
                        w.replace_external_text(text.clone());
                        w.saved_new_text = text;
                        w.recompute_diff(ws_mode);
                        w.disk_state = ReviewDiskState::Current;
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
    let latest = match git::read_worktree_source(Path::new(&modal.abs_path)) {
        Ok(latest) => latest,
        Err(err) => {
            app_signal().update(|s| {
                if let Some(conflict) = &mut s.save_conflict {
                    conflict.warning_message = Some(format!("无法核对磁盘版本：{err:#}"));
                }
            });
            return;
        }
    };
    let latest_deleted = latest.is_none();
    let changed_again = latest_deleted != modal.disk_deleted
        || latest.as_ref().is_some_and(|disk| disk.version.sha256 != modal.disk_version.sha256);

    if changed_again {
        app_signal().update(|s| {
            if let Some(m) = &mut s.save_conflict {
                (m.disk_text, m.disk_version) = latest.map_or_else(
                    || (String::new(), ReviewVersion::default()),
                    |disk| (disk.text, disk.version),
                );
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

    let Some(latest) = latest else {
        app_signal().update(|s| s.save_conflict = None);
        match &modal.target {
            ConflictTarget::CodeTab { tab_id } => force_close_code_tabs(|tab| &tab.id == tab_id),
            ConflictTarget::WorkingDiff { .. } => close_working_diff_now(),
        }
        app_signal().update(|s| s.focus_zone = editor_focus_zone(s));
        focus_active_editor_ref();
        show_toast("info", "已放弃编辑并关闭已删除文件");
        return;
    };

    let mut refresh_diff = false;
    app_signal().update(|s| {
        s.save_conflict = None;
        let (source_id, diff_key, history) = match &modal.target {
            ConflictTarget::CodeTab { tab_id } => (Some(tab_id), None, s.code_undo.get(tab_id).cloned()),
            ConflictTarget::WorkingDiff { file_key } => {
                let history = s.working_diff.as_ref().filter(|diff| diff.file.key() == *file_key)
                    .map(|diff| diff.undo_history.clone());
                (None, Some(file_key), history)
            }
        };
        for tab in &mut s.code_tabs {
            let shared = history.as_ref().is_some_and(|history| s.code_undo.get(&tab.id) == Some(history));
            if source_id == Some(&tab.id) || shared {
                refresh::adopt_source_version(tab, latest.clone());
                s.code_undo.remove(&tab.id);
            }
        }
        if let Some(diff) = s.working_diff.as_mut().filter(|diff| {
            diff_key.is_some_and(|file_key| diff.file.key() == *file_key)
                || history.as_ref() == Some(&diff.undo_history)
        }) {
            diff.undo_history = DocumentUndo::default();
            diff.current_new_text.clone_from(&latest.text);
            diff.saved_new_text.clone_from(&latest.text);
            diff.data.new_text = latest.text;
            diff.data.line_ending = latest.line_ending;
            diff.data.version = latest.version;
            diff.data.editable = latest.editable && diff.data.old_text_block.is_none();
            diff.data.readonly_reason = latest.readonly_reason;
            diff.data.new_text_block = git::ReviewTextBlock::from_flags(latest.binary, latest.too_large);
            diff.disk_state = ReviewDiskState::for_diff(&diff.data);
            diff.content_rev += 1;
            let _ = diff.update_draft_comparison(WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode), true);
            refresh_diff = true;
        }
        s.focus_zone = if source_id.is_some() { FocusZone::CodeEditor } else { FocusZone::WorkingDiffNew };
    });
    show_toast("ok", "已加载磁盘最新版本");
    if refresh_diff {
        refresh_workspace_async(Some(HashSet::from([modal.repo_id])));
    }
}

// ============================================================================
// 关闭标签与脏标签保护
// ============================================================================

pub fn close_active_tab() {
    cancel_diff_read();
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
    let mut refresh_repo = None;
    app_signal().update(|s| {
        if let Some(diff) = s.working_diff.take().filter(WorkingDiffSession::is_dirty) {
            for tab in s.code_tabs.iter_mut().filter(|tab| {
                same_working_document(tab, &diff.file, &diff.data)
                    && tab.text == diff.current_new_text
                    && s.code_undo.get(&tab.id) == Some(&diff.undo_history)
            }) {
                tab.text.clone_from(&tab.saved_text);
                tab.content_rev += 1;
                refresh_repo = Some(tab.file.repo_id.clone());
            }
            s.code_undo.retain(|_, history| history != &diff.undo_history);
        }
        s.parked_working_diff_undo.clear();
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
    if let Some(repo) = refresh_repo {
        refresh_workspace_async(Some(HashSet::from([repo])));
    }
}

pub fn close_commit_diff_now() {
    cancel_diff_read();
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
    let mut refresh_repo = None;
    app_signal().update(|s| {
        if let Some(diff) = s.working_diff.as_mut().filter(|diff| diff.is_dirty())
            && s.code_tabs.iter().any(|tab| {
                should_close(tab) && tab.is_dirty()
                    && same_working_document(tab, &diff.file, &diff.data)
                    && tab.text == diff.current_new_text
                    && s.code_undo.get(&tab.id) == Some(&diff.undo_history)
            })
        {
            diff.undo_history = DocumentUndo::default();
            diff.current_new_text.clone_from(&diff.saved_new_text);
            let _ = diff.update_draft_comparison(WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode), true);
            diff.content_rev += 1;
            refresh_repo = Some(diff.file.repo_id.clone());
        }
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
    if let Some(repo) = refresh_repo {
        refresh_workspace_async(Some(HashSet::from([repo])));
    }
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
        | FocusZone::ComparisonNew => copy_editor_selection(focus),
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
        FocusZone::FindResults => {}
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

fn copy_editor_selection(focus: FocusZone) {
    if let Some(editor) = active_editor_api(focus) {
        let selected = editor.selected_text();
        if selected.is_empty() {
            editor.run(nsh_base::EditorCommand::Copy);
        } else {
            set_clipboard(selected);
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
        FocusZone::Modal | FocusZone::FindResults | FocusZone::ComparisonOld | FocusZone::ComparisonNew => None,
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
            app_signal().update(|s| {
                select_project_file_in_tree(s, &repo_id, &path);
                s.project_reveal_request = s.project_reveal_request.wrapping_add(1);
            });
        }
    });
}

/// 在文件树中定位（`⌥F1` 或右键「在文件树中定位」）：显示 Project 并聚焦目标。
pub fn locate_in_project_tree(repo_id: &str, rel_path: &str) {
    app_signal().update(|s| {
        s.show_left_panel = true;
        if s.left_tab != LeftTab::Project {
            s.dismiss_tree_filter();
        }
        s.left_tab = LeftTab::Project;
        let filter = s.project_filter.trim().to_ascii_lowercase();
        if !paths::normalize_slashes(rel_path).to_ascii_lowercase().contains(&filter) {
            s.project_filter.clear();
        }
        select_project_file_in_tree(s, repo_id, rel_path);
        s.project_reveal_request = s.project_reveal_request.wrapping_add(1);
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

pub fn set_whitespace_mode(mode: WhitespaceMode) {
    app_signal().update(|s| {
        s.cfg.diff_whitespace_mode = mode.as_config().to_string();
        if let Some(w) = &mut s.working_diff {
            w.recompute_diff(mode);
            w.boundary_hint = None;
            w.content_rev += 1;
        }
        if let Some(c) = &mut s.commit_diff {
            c.computed = compute_diff(&c.data.old_text, &c.data.new_text, mode);
            c.boundary_hint = None;
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
    if tab == EditorTabKind::Welcome {
        return;
    }
    let selected = if text_input_focus_signal().with_untracked(|s| s.focused) {
        None
    } else {
        active_editor_api(focus).map(|editor| editor.selected_text())
            .filter(|text| !text.is_empty() && !text.contains(['\n', '\r']))
    };
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
        EditorTabKind::Comparison => {
            app_signal().update(|s| {
                s.comparison_find_open = true;
                s.comparison_find_side = if focus == FocusZone::ComparisonOld {
                    DiffSide::Old
                } else {
                    DiffSide::New
                };
            });
        }
        EditorTabKind::Welcome => {}
    }
    if let Some(selected) = selected {
        app_signal().update(|s| {
            let search = if tab == EditorTabKind::File { &mut s.code_search } else { &mut s.diff_search };
            search.query = if search.regex { regex::escape(&selected) } else { selected }.into();
        });
    }
    after_answer(move || {
        let end = app_signal().with_untracked(|s| {
            if s.editor_tab != tab || !s.active_find_open() {
                return None;
            }
            let search = if tab == EditorTabKind::File { &s.code_search } else { &s.diff_search };
            Some(search.query.encode_utf16().count() as u32)
        });
        let input = editor_refs_signal().with_untracked(|refs| {
            if tab == EditorTabKind::File { refs.code_find_input } else { refs.diff_find_input }
        });
        if let (Some(end), Some(input)) = (end, input) {
            input.focus();
            input.set_selection(0, end);
        }
    });
}

pub(crate) fn close_find_bar() {
    let mut closed = false;
    app_signal().update(|s| closed = s.close_editor_find());
    if closed {
        set_text_input_focused(false);
        focus_active_editor_ref();
    }
}

/// 进入弹窗前记录当前非弹窗焦点，确保关闭弹窗后恢复树/编辑器焦点而非永久滞留 `FocusZone::Modal`。
pub fn enter_modal_focus(s: &mut AppState) {
    if s.focus_zone != FocusZone::Modal {
        s.prev_focus_zone = s.focus_zone;
    }
    s.focus_zone = FocusZone::Modal;
}

pub(crate) fn editor_focus_zone(s: &AppState) -> FocusZone {
    match s.editor_tab {
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
    }
}

/// 关闭弹窗后恢复先前焦点（若原焦点对应的编辑页已关闭则回退到 Commit 树）。
pub fn restore_focus_after_modal(s: &mut AppState) {
    if s.focus_zone != FocusZone::Modal {
        return;
    }
    let fallback = editor_focus_zone(s);
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
    if crate::ui::github_panel::submitting() || crate::ui::content_search::intercept_close()
        || crate::history_runtime::intercept_close()
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
        } else {
            restored_editor = s.close_editor_find();
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

/// 在当前查找范围内跳到下一处或上一处匹配（`⌘G` / `⇧⌘G`）。
pub fn run_active_find_nav(next: bool) {
    let target = app_signal().with_untracked(|s| {
        if !s.active_find_open() {
            return None;
        }
        let left = s.diff_find_side_for(s.editor_tab) == Some(DiffSide::Old);
        Some(match (s.editor_tab, left) {
            (EditorTabKind::File, _) => FocusZone::CodeEditor,
            (EditorTabKind::WorkingDiff, true) => FocusZone::WorkingDiffOld,
            (EditorTabKind::WorkingDiff, false) => FocusZone::WorkingDiffNew,
            (EditorTabKind::CommitDiff, true) => FocusZone::CommitDiffOld,
            (EditorTabKind::CommitDiff, false) => FocusZone::CommitDiffNew,
            (EditorTabKind::Comparison, true) => FocusZone::ComparisonOld,
            (EditorTabKind::Comparison, false) => FocusZone::ComparisonNew,
            (EditorTabKind::Welcome, _) => return None,
        })
    });
    if let Some(api) = target.and_then(active_editor_api) {
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
                s.project_reveal_request = s.project_reveal_request.wrapping_add(1);
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
                s.project_reveal_request = s.project_reveal_request.wrapping_add(1);
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
                    w.boundary_hint = None;
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
                    c.boundary_hint = None;
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
        EditorTabKind::WorkingDiff => reveal_in_active_diff(w_side, pos),
        EditorTabKind::CommitDiff => reveal_in_active_diff(c_side, pos),
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
                        repo_name: repo.label().to_owned(),
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
            s.file_scan_pending().then(|| (s.cfg.workspace_path.clone(), raw_input.to_owned()))
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
    run_git_action_with_refresh(work, complete, None)
}

fn run_git_action_with_refresh<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    complete: impl FnOnce(T) + 'static,
    refresh_scope: Option<HashSet<String>>,
) -> bool {
    if app_signal().with_untracked(|s| s.command_busy) {
        show_toast("warn", "其他 Git 操作正在运行");
        return false;
    }
    app_signal().update(|s| {
        s.command_busy = true;
        s.pause_workspace_refresh_for_git_write();
    });
    let task = async move {
        let result = blocking(work).await;
        app_signal().update(|s| s.command_busy = false);
        match result {
            Ok(value) => complete(value),
            Err(err) => show_toast("err", format!("任务失败：{err}")),
        }
        refresh_workspace_async(refresh_scope);
    };
    spawn_from_root(task);
    true
}

/// 暂存或取消暂存指定文件（命令期间暂缓监听刷新，结束后补刷）。
pub fn stage_files_action(files: Vec<ChangedFile>, stage: bool) {
    if files.is_empty() {
        return;
    }
    let success = match (stage, files.as_slice()) {
        (true, [file]) => format!("已暂存 {}", file.path),
        (false, [file]) => format!("已取消暂存 {}", file.path),
        (true, _) => "已暂存选中文件".into(),
        (false, _) => "已取消暂存选中文件".into(),
    };
    let mut repo_ids = HashSet::new();
    let mut by_repo: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        repo_ids.insert(file.repo_id);
        let paths = by_repo.entry(file.repo_path).or_default();
        let scope_status = if stage { file.worktree_status } else { file.index_status };
        if scope_status == 'R' && !file.orig_path.is_empty() {
            paths.push(file.orig_path);
        }
        paths.push(file.path);
    }
    run_git_action_with_refresh(
        move || {
            for (repo_path, paths) in by_repo {
                git::stage_or_unstage_files(&repo_path, &paths, stage)?;
            }
            anyhow::Ok(())
        },
        move |res| match res {
            Ok(()) => show_toast("ok", success),
            Err(err) => {
                let action = if stage { "暂存操作失败" } else { "取消暂存失败" };
                show_toast("err", format!("{action}：{err}"));
            }
        },
        Some(repo_ids),
    );
}

/// Apply the keyboard command to the selected Commit row.
pub fn stage_selected_change(stage: bool) {
    let file = app_signal().with_untracked(|s| {
        s.change_selection
            .as_ref()
            .map(|sel| sel.file.clone())
    });
    if let Some(file) = file {
        stage_files_action(vec![file], stage);
    }
}

pub fn open_local_history_for_active() {
    crate::history_runtime::open_active();
}

pub fn open_file_revision_history(repo: RepoInfo, rev: String, rel_path: String) {
    app_signal().update(|s| {
        let head_seen = file_history_head(s, &rev, &repo.path);
        s.show_git_panel = true;
        s.file_history = Some(FileRevisionHistory {
            repo,
            rel_path,
            rev,
            commits: Vec::new(),
            paths: HashMap::new(),
            show_affected: false,
            loading: true,
            error: None,
            head_seen,
        });
        s.selected_commit = None;
        s.selected_commit_file = None;
        s.selected_commit_files.clear();
    });

    crate::ui::content_search::hide_find_panel();
    crate::ui::git_pane::focus_history();
    load_file_revision_history(false);
}

fn file_history_head(s: &AppState, rev: &str, repo_path: &str) -> Option<git::RepositoryHead> {
    matches!(rev, "" | "HEAD").then(|| s.repository_head(repo_path))
}

fn load_file_revision_history(background: bool) {
    let Some((workspace, repo, rev, rel_path, head)) = app_signal().with_untracked(|s| {
        let history = s.file_history.as_ref()?;
        Some((
            s.cfg.workspace_path.clone(),
            history.repo.clone(),
            history.rev.clone(),
            history.rel_path.clone(),
            file_history_head(s, &history.rev, &history.repo.path),
        ))
    }) else {
        return;
    };
    let request_id =
        global::<FileHistoryRequest>().with_untracked(|request| request.0.wrapping_add(1));
    global::<FileHistoryRequest>().set(FileHistoryRequest(request_id));
    app_signal().update(|s| {
        if let Some(history) = s.file_history.as_mut() {
            history.head_seen = head.clone();
            history.loading = !background || history.commits.is_empty();
            history.error = None;
        }
    });

    spawn_from_root(async move {
        let repo_id = repo.id.clone();
        let repo_path = repo.path.clone();
        let expected_rev = rev.clone();
        let expected_path = rel_path.clone();
        let outcome = blocking(move || -> anyhow::Result<_> {
            let rev = match head {
                Some(git::RepositoryHead::Commit(hash)) => hash,
                Some(git::RepositoryHead::Unborn) => return Ok((Vec::new(), HashMap::new())),
                _ => rev,
            };
            let commits = git::file_history_at_revision(&repo, &rev, &rel_path, 60)?;
            let paths = crate::file_revisions::list(Path::new(&repo.path), &rev, &rel_path, 60)?
                .into_iter()
                .map(|revision| (revision.hash, revision.path))
                .collect();
            Ok((commits, paths))
        })
        .await;
        if !global::<FileHistoryRequest>().with_untracked(|request| request.0 == request_id) {
            return;
        }
        let mut next_selection = None;
        app_signal().update(|s| {
            let Some(history) = s.file_history.as_mut().filter(|history| {
                s.cfg.workspace_path == workspace
                    && history.repo.id == repo_id
                    && history.repo.path == repo_path
                    && history.rel_path == expected_path
                    && history.rev == expected_rev
            }) else {
                return;
            };
            history.loading = false;
            match outcome {
                Ok(Ok((commits, paths))) => {
                    if let Some(selected) = &s.selected_commit
                        && let Some(entry) = commits.iter().find(|entry| {
                            entry.repo_id == selected.repo_id && entry.hash == selected.hash
                        })
                    {
                        s.selected_commit = Some(entry.clone());
                    } else {
                        next_selection = Some(commits.first().cloned());
                    }
                    history.commits = commits;
                    history.paths = paths;
                }
                Ok(Err(err)) => history.error = Some(err.to_string()),
                Err(err) => history.error = Some(err.to_string()),
            }
        });
        match next_selection {
            Some(Some(commit)) => crate::ui::git_pane::load_commit_selection(commit, !background),
            Some(None) => crate::ui::git_pane::clear_commit_selection(),
            None => {}
        }
    });
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

fn reveal_in_active_diff(side: DiffSide, pos: EditorPosition) {
    let (tab, effective_pos) = app_signal().with_untracked(|s| {
        let single = s.cfg.diff_single_column;
        let mapped_pos = if single {
            let u_line = match s.editor_tab {
                EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|w| {
                    map_editable_side_line_to_unified(
                        &w.data.old_text,
                        &w.current_new_text,
                        &w.computed.hunks,
                        side,
                        pos.line as usize,
                        w.editor_editable(),
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
        api.reveal_position(effective_pos, true);
    }
}
