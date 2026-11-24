use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use nsh_core::prelude::*;

use crate::diff_model::{WhitespaceMode, compute_diff};
use crate::git::{self, LineEnding, SaveReviewOutcome, WorkspaceFile, WorktreeRead};
use crate::local_history::{LocalHistoryStore, SnapshotMeta};
use crate::state::{self, AppState, EditorTabKind, RootOwnerHolder, app_signal};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct View {
    pub entries: Arc<Vec<SnapshotMeta>>,
    pub selected_id: Option<String>,
    pub title: String,
    pub current_text: String,
    pub snapshot_text: String,
    pub loading: bool,
    pub comparison_ready: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
enum Buffer {
    Code {
        id: String,
        version: u64,
        text: String,
    },
    Diff {
        key: String,
        version: u64,
        text: String,
    },
}

impl Buffer {
    fn text(&self) -> &str {
        match self {
            Self::Code { text, .. } | Self::Diff { text, .. } => text,
        }
    }
}

#[derive(Clone)]
struct Comparison {
    disk: WorktreeRead,
    buffer: Option<Buffer>,
    snapshot: String,
    ending: LineEnding,
}

impl Comparison {
    fn current(&self) -> &str {
        self.buffer
            .as_ref()
            .map_or(self.disk.text.as_str(), Buffer::text)
    }
}

#[derive(Clone, Default)]
struct History {
    workspace: String,
    file: Option<WorkspaceFile>,
    entries: Arc<Vec<SnapshotMeta>>,
    selected_id: Option<String>,
    comparison: Option<Arc<Comparison>>,
    generation: u64,
    loading: bool,
    writing: bool,
    confirm: bool,
    error: Option<String>,
    notice: Option<String>,
}

fn signal() -> Signal<History> {
    global::<History>()
}

fn spawn_at_root(task: impl std::future::Future<Output = ()> + 'static) {
    if let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) {
        owner.run(|| spawn(task));
    }
}

pub(crate) fn open_active() {
    let file = app_signal().with_untracked(|s| match s.editor_tab {
        EditorTabKind::File => s
            .code_tabs
            .get(s.active_code_tab_idx)
            .map(|tab| tab.file.clone()),
        EditorTabKind::WorkingDiff => {
            let diff = s.working_diff.as_ref()?;
            s.workspace_files
                .iter()
                .find(|file| file.key() == diff.file.key())
                .cloned()
        }
        _ => None,
    });
    if let Some(file) = file {
        open(file);
    } else {
        state::show_toast("info", "请先打开一个文件");
    }
}

fn open(file: WorkspaceFile) {
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let generation = signal().with_untracked(|s| s.generation.wrapping_add(1));
    signal().set(History {
        workspace: workspace.clone(),
        file: Some(file.clone()),
        generation,
        loading: true,
        ..History::default()
    });
    app_signal().update(state::enter_modal_focus);
    spawn_at_root(async move {
        let entries = blocking(move || {
            LocalHistoryStore::default_store().list(&file.repo_id, &file.rel_path)
        })
        .await;
        if !is_current(&workspace, generation) {
            return;
        }
        let entries = entries
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
        match entries {
            Ok(entries) => {
                let first = entries.first().map(|entry| entry.id.clone());
                signal().update(|s| {
                    s.entries = Arc::new(entries);
                    s.loading = false;
                });
                if let Some(first) = first {
                    select(&first);
                }
            }
            Err(err) => show_error(generation, format!("读取本地历史失败：{err:#}")),
        }
    });
}

pub fn select(id: &str) {
    let target = signal().with_untracked(|s| {
        if s.writing || !s.entries.iter().any(|entry| entry.id == id) {
            return None;
        }
        Some((
            s.file.clone()?,
            s.workspace.clone(),
            s.generation.wrapping_add(1),
        ))
    });
    let Some((file, workspace, generation)) = target else {
        return;
    };
    let id = id.to_string();
    signal().update(|s| {
        s.generation = generation;
        s.selected_id = Some(id.clone());
        s.comparison = None;
        s.loading = true;
        s.confirm = false;
        s.error = None;
    });
    let read_file = file.clone();
    spawn_at_root(async move {
        let result = blocking(move || -> anyhow::Result<_> {
            let bytes = LocalHistoryStore::default_store().read_content(
                &read_file.repo_id,
                &read_file.rel_path,
                &id,
            )?;
            let ending = LineEnding::detect(&bytes);
            anyhow::ensure!(!bytes.contains(&0), "无法预览二进制快照");
            let snapshot = LineEnding::normalize_to_lf(std::str::from_utf8(&bytes)?);
            let disk = git::read_review_worktree(Path::new(&read_file.abs_path));
            Ok((snapshot, ending, disk))
        })
        .await;
        if !is_current(&workspace, generation) {
            return;
        }
        let result = result
            .map_err(anyhow::Error::from)
            .and_then(|result| result);
        match result {
            Ok((snapshot, ending, disk)) => {
                let buffer = app_signal().with_untracked(|s| live_buffer(s, &file));
                signal().update(|s| {
                    s.comparison = Some(Arc::new(Comparison {
                        disk,
                        buffer,
                        snapshot,
                        ending,
                    }));
                    s.loading = false;
                });
            }
            Err(err) => show_error(generation, format!("读取快照失败：{err:#}")),
        }
    });
}

fn live_buffer(s: &AppState, file: &WorkspaceFile) -> Option<Buffer> {
    let key = file.key();
    let code = s
        .code_tabs
        .iter()
        .find(|tab| tab.file.key() == key && tab.editor_editable() && tab.revision.is_empty());
    let diff = s
        .working_diff
        .as_ref()
        .filter(|diff| diff.file.key() == key && diff.editor_editable());
    if let Some(tab) =
        code.filter(|tab| tab.is_dirty() || !diff.is_some_and(|diff| diff.is_dirty()))
    {
        return Some(Buffer::Code {
            id: tab.id.clone(),
            version: tab.content_rev,
            text: tab.text.clone(),
        });
    }
    diff.map(|diff| Buffer::Diff {
        key,
        version: diff.content_rev,
        text: diff.current_new_text.clone(),
    })
}

fn is_current(workspace: &str, generation: u64) -> bool {
    app_signal().with_untracked(|s| s.cfg.workspace_path == workspace)
        && signal().with_untracked(|s| s.generation == generation)
}

fn show_error(generation: u64, error: String) {
    signal().update(|s| {
        if s.generation == generation {
            s.loading = false;
            s.writing = false;
            s.error = Some(error);
        }
    });
}

pub fn is_open() -> bool {
    signal().with(|s| s.file.is_some())
}

pub(crate) fn intercept_close() -> bool {
    let (writing, confirming) = signal().with_untracked(|s| (s.writing, s.confirm));
    if confirming {
        cancel_confirmation();
    }
    writing || confirming
}

pub(crate) fn close_active() -> bool {
    if !is_open() {
        return false;
    }
    let generation = signal().with_untracked(|s| s.generation.wrapping_add(1));
    signal().set(History {
        generation,
        ..History::default()
    });
    true
}

pub fn snapshot() -> Option<View> {
    signal().with(|s| {
        let file = s.file.as_ref()?;
        let (current, snapshot) = s
            .comparison
            .as_ref()
            .map(|comparison| {
                (
                    comparison.current().to_string(),
                    comparison.snapshot.clone(),
                )
            })
            .unwrap_or_default();
        Some(View {
            entries: s.entries.clone(),
            selected_id: s.selected_id.clone(),
            title: file.rel_path.clone(),
            current_text: current,
            snapshot_text: snapshot,
            loading: s.loading || s.writing,
            comparison_ready: s.comparison.is_some() && !s.loading && !s.writing,
            error: s.error.clone(),
            notice: s.notice.clone(),
        })
    })
}

pub fn confirmation() -> bool {
    signal().with(|s| s.confirm)
}

pub fn request_restore_all() {
    signal().update(|s| {
        if s.comparison.is_some() && !s.loading && !s.writing {
            s.confirm = true;
        }
    });
}

pub fn cancel_confirmation() {
    signal().update(|s| s.confirm = false);
}

pub fn confirm_restore() {
    if signal().with_untracked(|s| s.confirm) {
        restore(None);
    }
}

pub fn restore_hunk(index: usize) {
    restore(Some(index));
}

fn restore(hunk: Option<usize>) {
    let target = signal().with_untracked(|s| {
        if s.loading || s.writing || s.confirm && hunk.is_some() {
            return None;
        }
        Some((
            s.file.clone()?,
            s.workspace.clone(),
            s.generation,
            s.comparison.clone()?,
            s.selected_id.clone()?,
        ))
    });
    let Some((file, workspace, generation, comparison, selected)) = target else {
        return;
    };
    let buffer = app_signal().with_untracked(|s| live_buffer(s, &file));
    if comparison.buffer != buffer {
        show_error(generation, "编辑缓冲区已变化，请重新选择快照".into());
        return;
    }
    let text = if let Some(index) = hunk {
        let diff = compute_diff(
            &comparison.snapshot,
            comparison.current(),
            WhitespaceMode::None,
        );
        let Some(hunk) = diff.hunks.get(index) else {
            return;
        };
        let Some(text) =
            crate::history_restore::restore_hunk(&comparison.snapshot, comparison.current(), hunk)
        else {
            return;
        };
        text
    } else {
        comparison.snapshot.clone()
    };

    if hunk.is_some()
        && let Some(buffer) = buffer.as_ref()
    {
        apply_buffer(buffer, &text);
        signal().update(|s| s.notice = Some("已在编辑器恢复此变更块".into()));
        select(&selected);
        return;
    }
    if !comparison.disk.editable {
        show_error(generation, "当前工作区文件不可写".into());
        return;
    }
    let ending = if hunk.is_some() {
        comparison.disk.line_ending
    } else {
        comparison.ending
    };
    let expected = comparison.disk.version.clone();
    let read_file = file.clone();
    let output = text.clone();
    let backup = buffer
        .as_ref()
        .map(|buffer| buffer.text().to_string())
        .filter(|text| text != &comparison.disk.text);
    signal().update(|s| {
        s.confirm = false;
        s.writing = true;
        s.error = None;
        s.notice = None;
    });
    spawn_at_root(async move {
        let result = blocking(move || {
            let path = Path::new(&read_file.abs_path);
            anyhow::ensure!(
                git::read_review_worktree(path).version == expected,
                "磁盘文件已变化，请重新选择快照"
            );
            let history = LocalHistoryStore::default_store();
            if let Some(backup) = backup {
                history.record(
                    &read_file.repo_id,
                    &read_file.rel_path,
                    "恢复前的未保存编辑",
                    backup.as_bytes(),
                )?;
            }
            match git::save_review_text_in_repo(
                &read_file.repo_id,
                Some(Path::new(&read_file.repo_path)),
                &read_file.rel_path,
                path,
                &expected,
                ending,
                &output,
                Some(&history),
            ) {
                SaveReviewOutcome::Saved { version } => Ok(version),
                SaveReviewOutcome::Conflict { .. } => {
                    anyhow::bail!("磁盘文件已变化，请重新选择快照")
                }
                SaveReviewOutcome::Rejected { reason } => anyhow::bail!("{reason}"),
            }
        })
        .await;
        if !is_current(&workspace, generation) {
            return;
        }
        match result
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
        {
            Ok(version) => {
                if let Some(buffer) = &buffer
                    && app_signal()
                        .with_untracked(|s| live_buffer(s, &file).as_ref() == Some(buffer))
                {
                    apply_buffer(buffer, &text);
                    app_signal().update(|s| {
                        let key = file.key();
                        for tab in s.code_tabs.iter_mut().filter(|tab| {
                            tab.file.key() == key && tab.revision.is_empty() && tab.text == text
                        }) {
                            tab.saved_text.clone_from(&text);
                            tab.version = version.clone();
                            tab.line_ending = ending;
                            tab.disk_state = crate::state::ReviewDiskState::Current;
                        }
                        if let Some(diff) = s
                            .working_diff
                            .as_mut()
                            .filter(|diff| diff.file.key() == key && diff.current_new_text == text)
                        {
                            diff.saved_new_text = text.clone();
                            diff.data.new_text = text.clone();
                            diff.data.version = version;
                            diff.data.line_ending = ending;
                            diff.disk_state = crate::state::ReviewDiskState::Current;
                        }
                    });
                }
                signal().update(|s| {
                    s.writing = false;
                    s.notice = Some("已恢复本地历史".into());
                });
                state::refresh_workspace_async(Some(HashSet::from([file.repo_id])));
                select(&selected);
            }
            Err(err) => show_error(generation, format!("恢复失败：{err:#}")),
        }
    });
}

fn apply_buffer(buffer: &Buffer, text: &str) {
    match buffer {
        Buffer::Code { id, .. } => crate::code_split_runtime::record_editor_change(id, text),
        Buffer::Diff { key, .. } => state::edit_working_diff_text(key, text),
    }
}
