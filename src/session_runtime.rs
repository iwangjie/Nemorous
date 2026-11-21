use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nsh_base::EditorPosition;
use nsh_core::prelude::*;

use crate::git::{self, WorkspaceFile};
use crate::session::{self, FileEntry, State};
use crate::state::{
    AppState, CodeTab, EditorTabKind, FocusZone, LeftTab, RootOwnerHolder, app_signal,
};

const MAX_TABS: usize = 20;
const SAVE_DELAY: Duration = Duration::from_millis(400);

#[derive(Clone, Default)]
struct Runtime {
    workspace: String,
    ready: bool,
    generation: u64,
    queued: Option<(PathBuf, State)>,
    writing: bool,
}

#[derive(PartialEq)]
struct ChoiceFingerprint {
    left_tab: LeftTab,
    editor_tab: EditorTabKind,
    focus: FocusZone,
    show_git_panel: bool,
    changes_split: f32,
    code_split: f32,
}

impl ChoiceFingerprint {
    fn from_app(s: &AppState) -> Self {
        Self {
            left_tab: s.left_tab,
            editor_tab: s.editor_tab,
            focus: s.focus_zone,
            show_git_panel: s.show_git_panel,
            changes_split: s.cfg.commit_left_width,
            code_split: s.cfg.code_left_width,
        }
    }
}

pub(crate) fn observe() {
    let snapshot = computed(|| {
        let (ready, workspace) = global::<Runtime>().with(|s| (s.ready, s.workspace.clone()));
        app_signal().with(|s| (ready && workspace == s.cfg.workspace_path).then(|| live_state(s)))
    });
    let root = Owner::current();
    let timer = stored(None::<Spawned<()>>);
    effect(move || {
        let next = snapshot.get();
        timer.with(|pending: &mut Option<_>| {
            if let Some(handle) = pending.take() {
                handle.cancel();
            }
            if let Some(snapshot) = next {
                let workspace = snapshot.workspace.as_deref().unwrap_or_default();
                let path = session::session_path(workspace);
                *pending =
                    Some(root.run(|| set_timeout(SAVE_DELAY, move || queue_write(path, snapshot))));
            }
        });
    });
}

fn queue_write(path: PathBuf, snapshot: State) {
    let runtime = global::<Runtime>();
    let mut start = false;
    runtime.update(|s| {
        s.queued = Some((path, snapshot));
        if !s.writing {
            s.writing = true;
            start = true;
        }
    });
    if !start {
        return;
    }
    spawn(async move {
        loop {
            let mut next = None;
            runtime.update(|s| {
                next = s.queued.take();
                if next.is_none() {
                    s.writing = false;
                }
            });
            let Some((path, snapshot)) = next else { return };
            match blocking(move || session::save_if_changed(&path, &snapshot)).await {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => log::warn!("保存工作区会话失败: {err}"),
                Err(err) => log::warn!("会话写入任务中断: {err}"),
            }
        }
    });
}

fn live_state(s: &AppState) -> State {
    let mut snapshot = State {
        workspace: Some(s.cfg.workspace_path.clone()),
        active_tab: Some(
            match s.editor_tab {
                EditorTabKind::File => "code",
                EditorTabKind::WorkingDiff | EditorTabKind::CommitDiff => "changes",
                EditorTabKind::Welcome | EditorTabKind::Comparison => {
                    if s.left_tab == LeftTab::Project {
                        "code"
                    } else {
                        "changes"
                    }
                }
            }
            .into(),
        ),
        active_file: Some(-1),
        show_git_panel: s.show_git_panel,
        changes_split: s.cfg.commit_left_width,
        code_split: s.cfg.code_left_width,
        ..State::default()
    };
    for (index, tab) in s
        .code_tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| tab.revision.is_empty())
        .take(MAX_TABS)
    {
        if index == s.active_code_tab_idx {
            snapshot.active_file = Some(snapshot.open_files.len() as i64);
        }
        snapshot.open_files.push(FileEntry {
            repo_id: tab.file.repo_id.clone(),
            rel_path: tab.file.rel_path.clone(),
            line: i64::from(tab.cursor.line) + 1,
        });
    }
    snapshot
}

pub(crate) fn restore_after_refresh() {
    let runtime = global::<Runtime>();
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    if workspace.is_empty() || runtime.with_untracked(|s| s.workspace == workspace) {
        return;
    }

    let Some(root) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let (files, choice) = app_signal()
        .with_untracked(|s| (s.workspace_files.clone(), ChoiceFingerprint::from_app(s)));
    let generation = runtime.with_untracked(|s| s.generation + 1);
    runtime.update(|s| {
        s.workspace = workspace.clone();
        s.generation = generation;
        s.ready = false;
    });

    let path = session::session_path(&workspace);
    let read_workspace = workspace.clone();
    root.run(|| {
        spawn(async move {
            let plan = blocking(move || read_plan(&path, &read_workspace, files)).await;
            if !runtime.with_untracked(|s| s.generation == generation)
                || !app_signal().with_untracked(|s| s.cfg.workspace_path == workspace)
            {
                return;
            }
            if let Ok(Some(plan)) = plan {
                app_signal().update(|s| {
                    if s.code_tabs.is_empty() && ChoiceFingerprint::from_app(s) == choice {
                        let modal = s.focus_zone == FocusZone::Modal;
                        apply_plan(s, plan);
                        if modal {
                            s.prev_focus_zone = s.focus_zone;
                            s.focus_zone = FocusZone::Modal;
                        }
                    }
                });
            }
            runtime.update(|s| s.ready = true);
        })
    });
}

struct RestorePlan {
    tabs: Vec<CodeTab>,
    active: usize,
    saved: State,
}

fn read_plan(path: &Path, workspace: &str, files: Vec<WorkspaceFile>) -> Option<RestorePlan> {
    let saved = session::load(path);
    if saved.workspace.as_deref().is_some_and(|saved| {
        !saved.is_empty() && session::lexically_clean(saved) != session::lexically_clean(workspace)
    }) || (saved.active_tab.is_none() && saved.open_files.is_empty())
    {
        return None;
    }

    let index: HashMap<_, _> = files
        .into_iter()
        .map(|f| ((f.repo_id.clone(), f.rel_path.clone()), f))
        .collect();
    let mut tabs: Vec<CodeTab> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut active = None;
    let saved_active = saved.active_file.unwrap_or(0);
    for (original, entry) in saved.open_files.iter().take(MAX_TABS).enumerate() {
        let Some(file) = index.get(&(entry.repo_id.clone(), entry.rel_path.clone())) else {
            continue;
        };
        let id = format!("file::{}", file.key());
        let tab_index = if let Some(&existing) = seen.get(&id) {
            existing
        } else {
            let wt = git::read_review_worktree(Path::new(&file.abs_path));
            if !wt.version.exists {
                continue;
            }
            let i = tabs.len();
            seen.insert(id.clone(), i);
            tabs.push(CodeTab {
                id,
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
                cursor: EditorPosition::new(0, 0),
                scroll_y: 0.0,
                scroll_x: 0.0,
                external_changed: false,
                preview: false,
                content_rev: 1,
            });
            i
        };
        let tab = &mut tabs[tab_index];
        let last_line = tab.text.bytes().filter(|byte| *byte == b'\n').count() as u32;
        let line = u32::try_from(entry.line.max(1) - 1)
            .unwrap_or(u32::MAX)
            .min(last_line);
        tab.cursor = EditorPosition::new(line, 0);
        if saved_active == original as i64 {
            active = Some(tab_index);
        }
    }

    let active = active.unwrap_or_else(|| tabs.len().saturating_sub(1));
    Some(RestorePlan {
        tabs,
        active,
        saved,
    })
}

fn apply_plan(s: &mut AppState, plan: RestorePlan) {
    s.code_tabs = plan.tabs;
    s.active_code_tab_idx = plan.active;
    s.show_git_panel = plan.saved.show_git_panel;
    if let Some(size) = restored_width(plan.saved.changes_split) {
        s.cfg.commit_left_width = size;
    }
    if let Some(size) = restored_width(plan.saved.code_split) {
        s.cfg.code_left_width = size;
    }
    match plan.saved.active_tab.as_deref() {
        Some("code") => {
            s.left_tab = LeftTab::Project;
            s.focus_zone = FocusZone::ProjectTree;
            if !s.code_tabs.is_empty() {
                s.editor_tab = EditorTabKind::File;
                s.focus_zone = FocusZone::CodeEditor;
            }
        }
        Some("changes") => {
            s.left_tab = LeftTab::Commit;
            s.editor_tab = EditorTabKind::Welcome;
            s.focus_zone = FocusZone::CommitTree;
        }
        _ if !s.code_tabs.is_empty() => {
            s.editor_tab = EditorTabKind::File;
            s.focus_zone = FocusZone::CodeEditor;
        }
        _ => {}
    }
}

fn restored_width(value: f32) -> Option<f32> {
    (value.is_finite() && value > 80.0).then(|| value.clamp(90.0, 640.0))
}
