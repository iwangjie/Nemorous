use std::collections::HashSet;
use std::path::Path;

use nsh_core::prelude::blocking;

use super::{AppState, CodeTab, DocumentUndo, GitBaselineIssue, ReviewDiskState, WorkingDiffSession, app_signal, compute_working_text_diff};
use crate::diff_model::{ComputedDiff, WhitespaceMode};
use crate::git::{self, ChangedFile, RepoStatus, ReviewChangeData, ReviewVersion, WorktreeRead};
use crate::git_index::IndexEntry;

pub(super) struct OpenReviewDocuments {
    sources: Vec<SourceTarget>,
    working: Option<WorkingTarget>,
}

impl OpenReviewDocuments {
    pub(super) fn capture(state: &AppState, repo_ids: Option<&HashSet<String>>) -> Self {
        let includes = |id: &str| repo_ids.is_none_or(|ids| ids.contains(id));
        Self {
            sources: state
                .code_tabs
                .iter()
                .filter(|tab| tab.revision.is_empty() && includes(&tab.file.repo_id))
                .map(SourceTarget::new)
                .collect(),
            working: state
                .working_diff
                .as_ref()
                .filter(|diff| includes(&diff.file.repo_id))
                .map(WorkingTarget::new),
        }
    }

    pub(super) fn read(self, statuses: &[RepoStatus]) -> ReviewReads {
        ReviewReads {
            sources: self
                .sources
                .into_iter()
                .filter_map(SourceRead::read)
                .collect(),
            working: self
                .working
                .map(|target| WorkingRead::read(target, statuses)),
        }
    }
}

struct SourceTarget {
    id: String,
    path: String,
    version: ReviewVersion,
    editable: bool,
    readonly_reason: Option<String>,
    binary: bool,
    too_large: bool,
    disk_state: ReviewDiskState,
}

impl SourceTarget {
    fn new(tab: &CodeTab) -> Self {
        Self {
            id: tab.id.clone(),
            path: tab.file.abs_path.clone(),
            version: tab.version.clone(),
            editable: tab.editable,
            readonly_reason: tab.readonly_reason.clone(),
            binary: tab.binary,
            too_large: tab.too_large,
            disk_state: tab.disk_state,
        }
    }

    fn disk_changed(&self, disk: &WorktreeRead) -> bool {
        content_changed(&self.version, &disk.version)
            || self.editable != disk.editable
            || self.readonly_reason != disk.readonly_reason
            || self.binary != disk.binary
            || self.too_large != disk.too_large
    }

    fn matches(&self, tab: &CodeTab) -> bool {
        tab.revision.is_empty()
            && self.id == tab.id
            && self.path == tab.file.abs_path
            && self.version == tab.version
            && self.editable == tab.editable
            && self.readonly_reason == tab.readonly_reason
            && self.binary == tab.binary
            && self.too_large == tab.too_large
    }
}

struct SourceRead {
    target: SourceTarget,
    state: ReviewDiskState,
    disk: Option<WorktreeRead>,
}

impl SourceRead {
    fn read(target: SourceTarget) -> Option<Self> {
        let (state, disk) = match git::read_worktree_source(Path::new(&target.path)) {
            Ok(Some(disk)) if target.disk_changed(&disk) => (ReviewDiskState::Modified, Some(disk)),
            Ok(Some(_)) => (ReviewDiskState::Current, None),
            Ok(None) => (ReviewDiskState::Missing, None),
            Err(_) => (ReviewDiskState::Unreadable, None),
        };
        (disk.is_some() || state != target.disk_state).then_some(Self { target, state, disk })
    }

    fn apply(self, state: &mut AppState) {
        let Some(tab) = state
            .code_tabs
            .iter_mut()
            .find(|tab| self.target.matches(tab))
        else {
            return;
        };
        if tab.is_dirty() {
            tab.disk_state = self.state;
            return;
        }
        let Some(disk) = self.disk else {
            tab.disk_state = self.state;
            return;
        };
        if adopt_source_version(tab, disk) {
            state.code_undo.remove(&tab.id);
        }
    }
}

pub(super) fn adopt_source_version(tab: &mut CodeTab, disk: WorktreeRead) -> bool {
    let invalidate_history = content_changed(&tab.version, &disk.version);
    tab.disk_state = ReviewDiskState::Current;
    tab.text = disk.text.clone();
    tab.saved_text = disk.text;
    tab.line_ending = disk.line_ending;
    tab.version = disk.version;
    tab.editable = disk.editable;
    tab.readonly_reason = disk.readonly_reason;
    tab.binary = disk.binary;
    tab.too_large = disk.too_large;
    tab.content_rev += 1;
    invalidate_history
}

struct WorkingTarget {
    file: ChangedFile,
    staged_only: bool,
    version: ReviewVersion,
    index: Result<Option<IndexEntry>, String>,
    head: Option<String>,
}

impl WorkingTarget {
    fn new(diff: &WorkingDiffSession) -> Self {
        Self {
            file: diff.file.clone(),
            staged_only: diff.data.staged_only,
            version: diff.data.version.clone(),
            index: diff.data.index_snapshot.clone(),
            head: diff.data.head_snapshot.clone(),
        }
    }

    fn matches(&self, diff: &WorkingDiffSession) -> bool {
        self.file.repo_id == diff.file.repo_id
            && self.file.repo_path == diff.file.repo_path
            && self.file.path == diff.file.path
            && self.staged_only == diff.data.staged_only
            && self.version == diff.data.version
            && self.index == diff.data.index_snapshot
            && self.head == diff.data.head_snapshot
    }
}

struct WorkingRead {
    target: WorkingTarget,
    file: ChangedFile,
    disk: anyhow::Result<ReviewChangeData>,
    computed: Option<ComputedDiff>,
}

impl WorkingRead {
    fn read(target: WorkingTarget, statuses: &[RepoStatus]) -> Self {
        let file = working_file_from_statuses(&target.file, statuses);
        let disk = git::read_review_change(&file, target.staged_only);
        Self {
            target,
            file,
            disk,
            computed: None,
        }
    }

    fn current<'a>(&self, state: &'a AppState) -> Option<&'a WorkingDiffSession> {
        state
            .working_diff
            .as_ref()
            .filter(|diff| self.target.matches(diff))
    }

    fn calculation(&self, state: &AppState) -> Option<DiffCalculation> {
        let current = self.current(state)?;
        let disk = self.disk.as_ref().ok()?;
        let draft = current.is_dirty().then_some(&current.current_new_text);
        if draft.is_some() && disk.old_text_block.is_some() {
            return None;
        }
        let changes = DiffChanges::between(&current.data, disk);
        (changes.old || (changes.disk && draft.is_none())).then(|| DiffCalculation {
            mode: WhitespaceMode::from_config(&state.cfg.diff_whitespace_mode),
            draft: draft.cloned(),
        })
    }

    fn apply(self, state: &mut AppState) {
        let Some(current) = state
            .working_diff
            .as_mut()
            .filter(|diff| self.target.matches(diff))
        else {
            return;
        };
        let disk = match self.disk {
            Ok(disk) => disk,
            Err(err) => {
                let worktree_failed = err.is::<git::WorktreeDiffReadError>();
                current.git_baseline_issue = (!worktree_failed).then_some(GitBaselineIssue::ReadFailed);
                if worktree_failed {
                    current.disk_state = ReviewDiskState::Unreadable;
                }
                return;
            }
        };
        let changes = DiffChanges::between(&current.data, &disk);
        let dirty = current.is_dirty();
        let blocked = disk.old_text_block.filter(|_| dirty);
        let needs_computation = blocked.is_none() && (changes.old || (changes.disk && !dirty));
        if needs_computation && self.computed.is_none() {
            return;
        }

        current.file = self.file;
        current.git_baseline_issue = blocked.map(GitBaselineIssue::TextBlocked);
        current.disk_state = if dirty && disk.version.exists && (changes.disk || changes.metadata) {
            ReviewDiskState::Modified
        } else {
            ReviewDiskState::for_diff(&disk)
        };
        if blocked.is_some() {
            current.boundary_hint = None;
            return;
        }
        if let Some(computed) = self.computed {
            current.boundary_hint = None;
            current.computed = computed;
        }
        if dirty {
            if changes.old {
                current.data.old_text = disk.old_text;
                current.data.old_path = disk.old_path;
                current.data.old_title = disk.old_title;
                current.data.index_snapshot = disk.index_snapshot;
                current.data.head_snapshot = disk.head_snapshot;
                current.content_rev += 1;
            }
        } else if changes.old || changes.disk || changes.metadata {
            if changes.disk {
                current.undo_history = DocumentUndo::default();
            }
            current.current_new_text.clone_from(&disk.new_text);
            current.saved_new_text = disk.new_text.clone();
            current.data = disk;
            current.content_rev += 1;
        }
    }
}

pub(super) fn working_file_from_statuses(file: &ChangedFile, statuses: &[RepoStatus]) -> ChangedFile {
    let Some(status) = statuses.iter().find(|status| {
        status.repo.id == file.repo_id && status.repo.path == file.repo_path
    }) else {
        return file.clone();
    };
    if let Some(latest) = status.files.iter().find(|latest| latest.path == file.path) {
        return latest.clone();
    }
    let mut file = file.clone();
    if status.error.is_none() {
        file.repo_name = status.repo.label().to_owned();
        file.orig_path.clear();
        file.kind.clear();
        file.index_status = ' ';
        file.worktree_status = ' ';
        file.staged = false;
        file.unstaged = false;
        file.untracked = false;
        file.conflict = false;
        file.added = 0;
        file.deleted = 0;
    }
    file
}

struct DiffChanges {
    old: bool,
    disk: bool,
    metadata: bool,
}

impl DiffChanges {
    fn between(before: &ReviewChangeData, after: &ReviewChangeData) -> Self {
        Self {
            old: before.old_text != after.old_text
                || before.old_text_block != after.old_text_block
                || before.old_path != after.old_path
                || before.old_title != after.old_title
                || before.head_snapshot != after.head_snapshot
                || before.index_snapshot != after.index_snapshot,
            disk: content_changed(&before.version, &after.version)
                || before.new_text != after.new_text
                || before.new_text_block != after.new_text_block,
            metadata: before.version.mode != after.version.mode
                || before.readonly_reason != after.readonly_reason,
        }
    }
}

pub(super) fn content_changed(before: &ReviewVersion, after: &ReviewVersion) -> bool {
    before.sha256 != after.sha256 || before.exists != after.exists || before.ino != after.ino
}

#[derive(PartialEq, Eq)]
pub(super) struct DiffCalculation {
    pub(super) mode: WhitespaceMode,
    pub(super) draft: Option<String>,
}

impl DiffCalculation {
    pub(super) fn compute(&self, disk: &ReviewChangeData) -> ComputedDiff {
        compute_working_text_diff(disk, self.draft.as_deref(), self.mode)
    }

    fn matches(&self, diff: &WorkingDiffSession, state: &AppState) -> bool {
        self.mode == WhitespaceMode::from_config(&state.cfg.diff_whitespace_mode)
            && self.draft.as_deref() == diff.is_dirty().then_some(diff.current_new_text.as_str())
    }
}

pub(super) struct ReviewReads {
    sources: Vec<SourceRead>,
    working: Option<WorkingRead>,
}

impl ReviewReads {
    pub(super) async fn prepare_diff(&mut self, generation: u64) {
        let Some(mut read) = self.working.take() else {
            return;
        };
        loop {
            let plan = app_signal().with_untracked(|state| {
                (state.refresh_gen == generation)
                    .then(|| read.calculation(state))
                    .flatten()
            });
            let Some(plan) = plan else {
                self.working = Some(read);
                return;
            };
            let result = blocking(move || {
                let computed = read.disk.as_ref().ok().map(|disk| plan.compute(disk));
                (read, plan, computed)
            })
            .await;
            let Ok((returned, plan, computed)) = result else {
                return;
            };
            read = returned;
            let matches = app_signal().with_untracked(|state| {
                state.refresh_gen == generation
                    && read
                        .current(state)
                        .is_some_and(|diff| plan.matches(diff, state))
            });
            if matches {
                read.computed = computed;
                self.working = Some(read);
                return;
            }
            // An edit or a whitespace option changed while computing. Reuse the Git/disk
            // result and render it against the latest draft, or stop if the target changed.
        }
    }

    pub(super) fn apply(self, state: &mut AppState) {
        for source in self.sources {
            source.apply(state);
        }
        if let Some(read) = self.working {
            read.apply(state);
        }
    }
}
