use super::refresh::{DiffCalculation, working_file_from_statuses};
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffReadKind {
    Explicit,
    CommitPreview,
}

#[derive(Clone, Copy, Default)]
struct DiffReadActivity {
    generation: u64,
    pending: Option<DiffReadKind>,
}

pub(crate) fn diff_read_pending() -> bool {
    global::<DiffReadActivity>().with(|read| read.pending.is_some())
}

pub(crate) fn cancel_diff_read() {
    global::<DiffReadActivity>().update(|read| {
        read.generation = read.generation.wrapping_add(1);
        read.pending = None;
    });
}

fn cancel_commit_diff_preview() {
    if global::<DiffReadActivity>().with_untracked(|read| read.pending == Some(DiffReadKind::CommitPreview)) {
        cancel_diff_read();
    }
}

#[derive(Clone, PartialEq, Eq)]
enum EditorPage {
    Empty(EditorTabKind),
    Source(String),
    Working(String, bool),
    Commit(String, String, String),
}

impl EditorPage {
    fn active(state: &AppState) -> Self {
        match state.editor_tab {
            EditorTabKind::File => state
                .code_tabs
                .get(state.active_code_tab_idx)
                .map(|tab| Self::Source(tab.id.clone())),
            EditorTabKind::WorkingDiff => state
                .working_diff
                .as_ref()
                .map(|diff| Self::Working(diff.file.key(), diff.data.staged_only)),
            EditorTabKind::CommitDiff => state.commit_diff.as_ref().map(|diff| {
                Self::Commit(
                    diff.commit.repo_id.clone(),
                    diff.commit.hash.clone(),
                    diff.file.path.clone(),
                )
            }),
            _ => None,
        }
        .unwrap_or(Self::Empty(state.editor_tab))
    }
}

#[derive(Clone)]
pub(super) struct DiffReadRequest {
    generation: u64,
    workspace: String,
    page: EditorPage,
    focus: FocusZone,
}

impl DiffReadRequest {
    pub(super) fn start() -> Self {
        Self::start_with_kind(DiffReadKind::Explicit)
    }

    fn start_with_kind(kind: DiffReadKind) -> Self {
        let activity = global::<DiffReadActivity>();
        let generation = activity.with_untracked(|read| read.generation.wrapping_add(1));
        activity.set(DiffReadActivity {
            generation,
            pending: Some(kind),
        });
        app_signal().with_untracked(|s| Self {
            generation,
            workspace: s.cfg.workspace_path.clone(),
            page: EditorPage::active(s),
            focus: s.focus_zone,
        })
    }

    pub(super) fn pending(&self) -> bool {
        global::<DiffReadActivity>()
            .with_untracked(|read| read.pending.is_some() && read.generation == self.generation)
    }

    pub(super) fn latest(&self) -> bool {
        global::<DiffReadActivity>().with_untracked(|read| read.generation == self.generation)
    }

    pub(super) fn matches(&self, state: &AppState) -> bool {
        state.cfg.workspace_path == self.workspace
            && EditorPage::active(state) == self.page
            && state.focus_zone != FocusZone::Modal
    }

    pub(super) fn finish(&self) {
        if self.pending() {
            global::<DiffReadActivity>().update(|read| read.pending = None);
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum WorkingSelection {
    File(String, bool),
    Directory(String, String),
    Empty,
}

impl WorkingSelection {
    fn current(s: &AppState) -> Self {
        if let Some(row) = &s.change_selection {
            Self::File(row.file.key(), row.staged)
        } else if let Some((repo, path, _)) = &s.change_selected_dir {
            Self::Directory(repo.clone(), path.clone())
        } else {
            Self::Empty
        }
    }
}

#[derive(Clone)]
struct WorkingDiffLoad {
    file: ChangedFile,
    staged: bool,
    focus: bool,
    last_hunk: bool,
    selection: WorkingSelection,
}

impl WorkingDiffLoad {
    fn matches(&self, request: &DiffReadRequest, s: &AppState) -> bool {
        request.matches(s)
            && WorkingSelection::current(s) == self.selection
            && (self.focus
                || (s.show_left_panel
                    && s.left_tab == LeftTab::Commit
                    && s.cfg.change_diff_preview))
            && s.repos
                .iter()
                .any(|repo| repo.id == self.file.repo_id && repo.path == self.file.repo_path)
    }

    fn source(&self, request: &DiffReadRequest) -> Option<(u64, ChangedFile)> {
        if !request.pending() {
            return None;
        }
        app_signal().with_untracked(|s| {
            self.matches(request, s).then(|| {
                (
                    s.refresh_gen,
                    working_file_from_statuses(&self.file, &s.statuses),
                )
            })
        })
    }

    fn save_before_open(&self, request: &DiffReadRequest) -> bool {
        let load = self.clone();
        let waiting = request.clone();
        let saved = save_before_switching_working_diff(&self.file.key(), self.staged, move || {
            if waiting.latest() && app_signal().with_untracked(|s| load.matches(&waiting, s)) {
                load.start();
            }
        });
        if !saved {
            request.finish();
        }
        saved
    }

    fn start(self) {
        let request = DiffReadRequest::start();
        if self.save_before_open(&request) {
            spawn_from_root(self.read(request));
        }
    }

    fn keep_dirty_target(&self, data: &ReviewChangeData, focus: bool) -> bool {
        let (preserve, already_visible) = app_signal().with_untracked(|s| {
            let preserve = !self.staged
                && s.working_diff.as_ref().is_some_and(|diff| {
                    diff.file.repo_id == self.file.repo_id
                        && diff.file.repo_path == self.file.repo_path
                        && diff.file.path == self.file.path
                        && diff.is_dirty()
                        && (!data.editable || data.version.sha256 != diff.data.version.sha256)
                });
            (preserve, s.editor_tab == EditorTabKind::WorkingDiff)
        });
        if !preserve {
            return false;
        }
        if !already_visible {
            capture_editor_views();
            push_nav_back_current();
        }
        app_signal().update(|s| {
            s.editor_tab = EditorTabKind::WorkingDiff;
            if focus {
                s.focus_zone = editor_focus_zone(s);
            }
        });
        if focus {
            focus_active_editor_ref();
        }
        refresh_workspace_async(Some(HashSet::from([self.file.repo_id.clone()])));
        true
    }

    async fn read(self, request: DiffReadRequest) {
        loop {
            let prepared = match prepare_working_diff(self.staged, || self.source(&request)).await {
                Ok(Some(prepared)) => prepared,
                Ok(None) => break,
                Err(err) => {
                    if self.source(&request).is_some() {
                        show_toast("err", format!("读取工作区差异失败：{err:#}"));
                    }
                    break;
                }
            };
            // The previous Diff is still usable while Git reads the next one.
            if !self.save_before_open(&request) {
                return;
            }
            if self.source(&request).as_ref() != Some(&prepared.source) {
                continue;
            }
            let focus =
                self.focus && app_signal().with_untracked(|s| s.focus_zone == request.focus);
            request.finish();
            if !self.keep_dirty_target(&prepared.data, focus) {
                install_working_diff(
                    prepared.source.1,
                    prepared.data,
                    prepared.draft,
                    prepared.computed,
                    focus,
                    self.last_hunk,
                );
            }
            return;
        }
        request.finish();
    }
}

pub(super) struct PreparedWorkingDiff {
    pub source: (u64, ChangedFile),
    pub data: ReviewChangeData,
    pub draft: Option<String>,
    pub computed: ComputedDiff,
}

pub(super) async fn prepare_working_diff(
    staged: bool,
    source: impl Fn() -> Option<(u64, ChangedFile)>,
) -> anyhow::Result<Option<PreparedWorkingDiff>> {
    'read_source: while let Some(input) = source() {
        let file = input.1.clone();
        let data = blocking(move || git::read_review_change(&file, staged)).await?;
        if source().as_ref() != Some(&input) {
            continue;
        }
        let mut data = data?;
        loop {
            if source().as_ref() != Some(&input) {
                continue 'read_source;
            }
            let plan = working_diff_calculation(&input.1, &data);
            let (snapshot, plan, computed) = blocking(move || {
                let computed = plan.compute(&data);
                (data, plan, computed)
            })
            .await?;
            data = snapshot;
            if source().as_ref() != Some(&input) {
                continue 'read_source;
            }
            if plan == working_diff_calculation(&input.1, &data) {
                return Ok(Some(PreparedWorkingDiff {
                    source: input,
                    data,
                    draft: plan.draft,
                    computed,
                }));
            }
        }
    }
    Ok(None)
}

pub(super) fn open_working_diff_with_hunk(
    file: ChangedFile,
    staged: bool,
    focus: bool,
    last_hunk: bool,
) {
    let selection = app_signal().with_untracked(WorkingSelection::current);
    WorkingDiffLoad {
        file,
        staged,
        focus,
        last_hunk,
        selection,
    }
    .start();
}

fn working_diff_calculation(file: &ChangedFile, data: &ReviewChangeData) -> DiffCalculation {
    app_signal().with_untracked(|s| {
        let draft = if data.editable && !data.staged_only {
            s.code_tabs
                .iter()
                .find(|tab| {
                    tab.revision.is_empty()
                        && tab.file.repo_id == file.repo_id
                        && tab.file.rel_path == file.path
                        && tab.is_dirty()
                        && tab.version.sha256 == data.version.sha256
                })
                .map(|tab| tab.text.clone())
                .or_else(|| {
                    s.working_diff
                        .as_ref()
                        .filter(|current| {
                            current.file.repo_id == file.repo_id
                                && current.file.path == file.path
                                && current.is_dirty()
                                && current.data.version.sha256 == data.version.sha256
                        })
                        .map(|current| current.current_new_text.clone())
                })
        } else {
            None
        };
        DiffCalculation {
            mode: WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode),
            draft,
        }
    })
}

#[derive(PartialEq, Eq)]
struct LogSelection {
    commit: Option<(String, String)>,
    path: Option<String>,
}

impl LogSelection {
    fn current(s: &AppState) -> Self {
        Self {
            commit: s
                .selected_commit
                .as_ref()
                .map(|c| (c.repo_id.clone(), c.hash.clone())),
            path: s
                .selected_commit_file
                .as_ref()
                .map(|file| file.path.clone()),
        }
    }
}

enum CommitDiffTarget {
    File(CommitFileEntry),
    Adjacent {
        path: String,
        delta: i32,
        grouped: bool,
    },
}

fn commit_diff_scope(s: &AppState, commit: &CommitEntry, file: &CommitFileEntry) -> CommitDiffScope {
    let single_file = s.file_history.as_ref().is_some_and(|history| !history.show_affected)
        || s.selected_commit_files.len() == 1;
    if single_file && crate::ui::git_pane::file_is_visible(s, commit, file) {
        CommitDiffScope::SingleFile
    } else {
        CommitDiffScope::AllFiles
    }
}

pub fn open_commit_diff(commit: CommitEntry, file: CommitFileEntry, focus_diff: bool) {
    let scope = app_signal().with_untracked(|s| commit_diff_scope(s, &commit, &file));
    read_commit_diff(
        commit,
        CommitDiffTarget::File(file),
        CommitDiffOpenOptions { scope, focus_diff, ..CommitDiffOpenOptions::default() },
        false,
        None,
    );
}

pub(crate) fn set_commit_diff_preview(enabled: bool) {
    app_signal().update(|s| s.cfg.git_log_diff_preview = enabled);
    if !enabled {
        cancel_commit_diff_preview();
    }
    save_config_quiet();
    // Toggling the option must leave an explicit open or navigation request in control.
    if enabled && global::<DiffReadActivity>().with_untracked(|read| read.pending != Some(DiffReadKind::Explicit)) {
        let selected = app_signal().with_untracked(|s| s.selected_commit.clone().zip(s.selected_commit_file.clone()));
        if let Some((commit, file)) = selected {
            preview_commit_diff(commit, file);
        }
    }
}

pub(crate) fn preview_commit_diff(commit: CommitEntry, file: CommitFileEntry) {
    let (selected, already_open, scope) = app_signal().with_untracked(|s| {
        let selected = s.show_git_panel
            && s.cfg.git_log_diff_preview
            && s.selected_commit_file.as_ref() == Some(&file)
            && s.selected_commit
                .as_ref()
                .is_some_and(|c| c.repo_id == commit.repo_id && c.hash == commit.hash);
        let already_open = s.editor_tab == EditorTabKind::CommitDiff && s.commit_diff.as_ref().is_some_and(|diff| {
            diff.commit.repo_id == commit.repo_id && diff.commit.hash == commit.hash && diff.file.path == file.path
        });
        (selected, already_open, commit_diff_scope(s, &commit, &file))
    });
    if !selected {
        return;
    }
    if already_open {
        cancel_commit_diff_preview();
        return;
    }
    read_commit_diff(
        commit,
        CommitDiffTarget::File(file),
        CommitDiffOpenOptions { scope, ..CommitDiffOpenOptions::default() },
        true,
        None,
    );
}

pub fn move_commit_diff_file(delta: i32) -> bool {
    move_commit_diff_file_with_hunk(delta, false, false)
}

pub(super) fn move_commit_diff_file_with_hunk(
    delta: i32,
    land_on_last_hunk: bool,
    report_boundary: bool,
) -> bool {
    let current = app_signal().with_untracked(|s| {
        let diff = s.commit_diff.as_ref().filter(|diff| diff.scope == CommitDiffScope::AllFiles)?;
        let files = s
            .selected_commit
            .as_ref()
            .filter(|selected| {
                selected.repo_id == diff.commit.repo_id && selected.hash == diff.commit.hash
            })
            .filter(|_| s.selected_commit_files.contains(&diff.file))
            .map(|_| s.selected_commit_files.clone());
        Some((
            diff.commit.clone(),
            diff.file.path.clone(),
            files,
            s.cfg.git_log_group_files,
        ))
    });
    let Some((commit, path, files, grouped)) = current else {
        return false;
    };
    let target = match files {
        Some(mut files) => {
            crate::ui::git_pane::order_files(&mut files, grouped);
            let Some(file) = adjacent_commit_file(files, &path, delta) else {
                return false;
            };
            CommitDiffTarget::File(file)
        }
        None => CommitDiffTarget::Adjacent {
            path,
            delta,
            grouped,
        },
    };
    read_commit_diff(
        commit,
        target,
        CommitDiffOpenOptions { focus_diff: true, land_on_last_hunk, ..CommitDiffOpenOptions::default() },
        false,
        report_boundary.then_some(delta),
    );
    true
}

fn adjacent_commit_file(
    files: Vec<CommitFileEntry>,
    path: &str,
    delta: i32,
) -> Option<CommitFileEntry> {
    let idx = files.iter().position(|file| file.path == path)?;
    let next = idx.checked_add_signed(delta as isize)?;
    files.into_iter().nth(next)
}

fn read_commit_diff(
    commit: CommitEntry,
    target: CommitDiffTarget,
    mut options: CommitDiffOpenOptions,
    preview: bool,
    boundary: Option<i32>,
) {
    options.record_navigation = true;
    let kind = if preview { DiffReadKind::CommitPreview } else { DiffReadKind::Explicit };
    let request = DiffReadRequest::start_with_kind(kind);
    let (selection, mut whitespace) = app_signal().with_untracked(|s| {
        (
            LogSelection::current(s),
            WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode),
        )
    });
    let source = commit.clone();
    spawn_from_root(async move {
        let mut result = blocking(move || -> anyhow::Result<_> {
            let file = match target {
                CommitDiffTarget::File(file) => file,
                CommitDiffTarget::Adjacent {
                    path,
                    delta,
                    grouped,
                } => {
                    let mut files = git::commit_files(&source.repo_path, &source.hash)?;
                    crate::ui::git_pane::order_files(&mut files, grouped);
                    let Some(file) = adjacent_commit_file(files, &path, delta) else {
                        return Ok(None);
                    };
                    file
                }
            };
            let data = git::read_commit_diff(&source, &file)?;
            let computed = compute_diff(&data.old_text, &data.new_text, whitespace);
            Ok(Some((file, data, computed)))
        })
        .await;
        loop {
            if !request.pending() {
                return;
            }
            let (valid, focused, same_selection, latest_whitespace) = app_signal().with_untracked(|s| {
                let same_selection = LogSelection::current(s) == selection;
                let valid = request.matches(s)
                    && (!preview || (same_selection && s.show_git_panel && s.cfg.git_log_diff_preview));
                (
                    valid,
                    s.focus_zone == request.focus,
                    same_selection,
                    WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode),
                )
            });
            if !valid {
                request.finish();
                return;
            }
            if whitespace != latest_whitespace
                && let Ok(Ok(Some((file, data, _)))) = result
            {
                whitespace = latest_whitespace;
                result = blocking(move || {
                    let computed = compute_diff(&data.old_text, &data.new_text, whitespace);
                    Ok(Some((file, data, computed)))
                })
                .await;
                continue;
            }
            request.finish();
            match result {
                Ok(Ok(Some((file, data, computed)))) => {
                    options.focus_diff &= focused && same_selection;
                    options.select_log_file = same_selection;
                    install_commit_diff(commit, file, data, computed, options);
                }
                Ok(Ok(None)) => {
                    if let Some(delta) = boundary {
                        show_toast(
                            "info",
                            if delta < 0 {
                                "已是本提交第一个文件"
                            } else {
                                "已是本提交最后一个文件"
                            },
                        );
                    }
                }
                Ok(Err(err)) => report_commit_diff_error(preview, err),
                Err(err) => report_commit_diff_error(preview, err),
            }
            return;
        }
    });
}

fn report_commit_diff_error(preview: bool, error: impl std::fmt::Display) {
    if preview {
        show_toast("warn", format!("读取差异预览失败：{error}"));
    } else {
        show_toast("err", format!("读取提交差异失败：{error}"));
    }
}
