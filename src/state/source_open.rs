use super::diff_open::DiffReadRequest;
use super::*;

#[derive(Clone, Copy)]
enum SourceIntent {
    Open,
    Preview,
    FromWorkingDiff,
}

pub fn open_code_file(file: WorkspaceFile, target_pos: Option<EditorPosition>, focus_editor: bool) {
    if focus_editor {
        queue_open(file, target_pos, SourceIntent::Open);
    } else {
        open_without_focus(file, target_pos);
    }
}

pub(crate) fn prepare_workspace_edit_tab(file: WorkspaceFile) -> bool {
    open_without_focus(file, None)
}

fn open_without_focus(file: WorkspaceFile, target_pos: Option<EditorPosition>) -> bool {
    // Workspace edits need the validated model before applying a batch of changes.
    cancel_diff_read();
    let Some(read) = source_or_toast(&file, git::read_worktree_source(Path::new(&file.abs_path)))
    else {
        return false;
    };
    install(file, read, target_pos, SourceIntent::Open, false);
    true
}

pub fn open_code_file_preview(file: WorkspaceFile) {
    queue_open(file, None, SourceIntent::Preview);
}

pub(super) fn open_from_working_diff(file: WorkspaceFile, pos: EditorPosition) {
    queue_open(file, Some(pos), SourceIntent::FromWorkingDiff);
}

fn queue_open(file: WorkspaceFile, target_pos: Option<EditorPosition>, intent: SourceIntent) {
    let request = DiffReadRequest::start();
    let path = file.abs_path.clone();
    spawn_from_root(async move {
        let result = blocking(move || git::read_worktree_source(Path::new(&path))).await;
        if !request.pending()
            || !app_signal().with_untracked(|s| {
                request.matches(s)
                    && s.contains_project_root(&file.repo_id, &file.repo_path)
            })
        {
            request.finish();
            return;
        }
        request.finish();
        let Some(read) = source_or_toast(
            &file,
            result.map_err(anyhow::Error::from).and_then(|read| read),
        ) else {
            return;
        };
        let id = install(file, read, target_pos, intent, true);
        if let Some(pos) = target_pos {
            reveal_source_after_render(request, id, pos, 0.0);
        }
    });
}

fn source_or_toast(
    file: &WorkspaceFile,
    result: anyhow::Result<Option<git::WorktreeRead>>,
) -> Option<git::WorktreeRead> {
    match result {
        Ok(Some(read)) => Some(read),
        Ok(None) => {
            show_toast("warn", format!("文件已移动或删除：{}", file.rel_path));
            None
        }
        Err(err) => {
            show_toast("warn", format!("无法打开 {}：{err:#}", file.rel_path));
            None
        }
    }
}

fn install(
    file: WorkspaceFile,
    read: git::WorktreeRead,
    target_pos: Option<EditorPosition>,
    intent: SourceIntent,
    focus: bool,
) -> String {
    capture_editor_views();
    push_nav_back_current();
    let id = code_tab_id(&file.repo_id, &file.rel_path, "");
    let preview = matches!(intent, SourceIntent::Preview);
    let pos = target_pos.unwrap_or_else(|| EditorPosition::new(0, 0));
    let data_version = read.version.clone();
    app_signal().update(|s| {
        let existing = s.code_tabs.iter().position(|tab| tab.id == id);
        let index = if let Some(index) = existing {
            let tab = &mut s.code_tabs[index];
            if !tab.is_dirty() && tab.version != read.version
                && super::refresh::adopt_source_version(tab, read)
            {
                s.code_undo.remove(&id);
            }
            if !preview {
                tab.preview = false;
            }
            index
        } else {
            if preview {
                s.code_tabs.retain_mut(|tab| {
                    if tab.is_dirty() {
                        tab.preview = false;
                    }
                    !tab.preview
                });
            }
            s.code_tabs.push(CodeTab::from_read(
                file.clone(),
                String::new(),
                read,
                pos,
                preview,
            ));
            s.code_tabs.len() - 1
        };
        crate::code_split_runtime::select_tab_in_model(s, index);
        if preview && existing.is_none() {
            crate::code_split_runtime::prune_closed_tabs(s);
        }
        let index = s.active_code_tab_idx;
        sync_dirty_working_diff_into_code_tab(s, index);
        if matches!(intent, SourceIntent::FromWorkingDiff) {
            copy_active_diff_draft(s, index, &data_version);
        }
        if target_pos.is_some() {
            crate::code_split_runtime::set_active_position(s, pos);
        }
        s.editor_tab = EditorTabKind::File;
        if focus {
            s.focus_zone = FocusZone::CodeEditor;
        }
    });
    record_recent_location(NavTarget::CodeFile {
        repo_id: file.repo_id,
        rel_path: file.rel_path,
        revision: String::new(),
        pos,
        scroll_y: 0,
    });
    if focus {
        focus_active_editor_ref();
    }
    id
}

fn copy_active_diff_draft(s: &mut AppState, index: usize, disk: &ReviewVersion) {
    let tab = &mut s.code_tabs[index];
    let Some(diff) = s.working_diff.as_ref().filter(|diff| {
        diff.is_dirty()
            && diff.file.repo_id == tab.file.repo_id
            && diff.file.path == tab.file.rel_path
    }) else {
        return;
    };
    if tab.text != diff.current_new_text {
        tab.text.clone_from(&diff.current_new_text);
    }
    s.code_undo.insert(tab.id.clone(), diff.undo_history.clone());
    tab.saved_text.clone_from(&diff.saved_new_text);
    tab.version.clone_from(&diff.data.version);
    tab.line_ending = diff.data.line_ending;
    tab.disk_state = if super::refresh::content_changed(disk, &diff.data.version)
        || disk.mode != diff.data.version.mode
    {
        ReviewDiskState::Modified
    } else {
        ReviewDiskState::Current
    };
    tab.content_rev += 1;
}

pub(super) fn reveal_source_after_render(
    request: DiffReadRequest,
    id: String,
    pos: EditorPosition,
    scroll_y: f32,
) {
    after_answer(move || {
        if !request.latest()
            || !app_signal().with_untracked(|s| {
                s.editor_tab == EditorTabKind::File
                    && s.code_tabs
                        .get(s.active_code_tab_idx)
                        .is_some_and(|tab| tab.id == id)
            })
        {
            return;
        }
        reveal_in_code_editor(pos);
        if scroll_y > 0.0
            && let Some(api) = active_editor_api(FocusZone::CodeEditor)
        {
            api.set_scroll_offset(point(api.scroll_offset().x, px(scroll_y)));
        }
    });
}
