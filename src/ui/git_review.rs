use std::ops::Range;

use nsh_core::element::desktop::MenuItem;

use crate::diff_model::{DiffHunk, DiffSide};
use crate::git::ReviewChangeData;
use crate::selected_changes::{self, Selection, Side};
use crate::state::{self, AppState, FocusZone, WorkingDiffSession, app_signal};

#[derive(Clone)]
pub(crate) struct IncludedHunks {
    pub view: ReviewChangeData,
    pub hunks: Vec<DiffHunk>,
    exact: Vec<DiffHunk>,
}

pub(super) fn can_include(w: &WorkingDiffSession) -> bool {
    !w.data.staged_only
        && !w.file.staged
        && !w.file.untracked
        && !w.file.conflict
        && w.file.kind == "M"
        && !w.data.binary
        && !w.data.too_large
}

fn displayed_exact(w: &WorkingDiffSession) -> Option<(Vec<DiffHunk>, usize)> {
    let visible = w.computed.hunks.get(w.current_hunk)?;
    let exact =
        selected_changes::exact_hunks(w.data.old_text.as_bytes(), w.saved_new_text.as_bytes());
    let index = selected_changes::exact_hunk_index(&exact, visible).ok()?;
    Some((exact, index))
}

pub(super) fn included_state(s: &AppState) -> Option<bool> {
    let w = s
        .working_diff
        .as_ref()
        .filter(|w| can_include(w) && !w.is_dirty())?;
    let visible = w.computed.hunks.get(w.current_hunk)?;
    let key = w.file.key();
    let Some(part) = s.included_hunks.get(&key) else {
        return Some(s.checked_changes.contains(&key));
    };
    if !same_versions(&part.view, &w.data) {
        return Some(false);
    }
    let index = selected_changes::exact_hunk_index(&part.exact, visible).ok()?;
    Some(part.hunks.contains(&part.exact[index]))
}

fn same_versions(a: &ReviewChangeData, b: &ReviewChangeData) -> bool {
    a.repo_id == b.repo_id
        && a.new_path == b.new_path
        && a.staged_only == b.staged_only
        && a.version == b.version
        && a.index_snapshot == b.index_snapshot
}

pub(super) fn toggle_included_hunk() {
    app_signal().update(|s| {
        let Some(w) = s
            .working_diff
            .as_ref()
            .filter(|w| can_include(w) && !w.is_dirty())
        else {
            return;
        };
        let Some((exact, at)) = displayed_exact(w) else {
            log::debug!("找不到选中差异块");
            return;
        };
        let key = w.file.key();
        let mut included = s
            .included_hunks
            .get(&key)
            .filter(|part| same_versions(&part.view, &w.data))
            .map(|part| part.hunks.clone())
            .unwrap_or_else(|| {
                if s.checked_changes.contains(&key) {
                    exact.clone()
                } else {
                    Vec::new()
                }
            });
        let clicked = &exact[at];
        if included.contains(clicked) {
            included.retain(|hunk| hunk != clicked);
        } else {
            included.push(clicked.clone());
        }
        log::debug!("变更块勾选结果 {}/{}", included.len(), exact.len());
        if included.is_empty() {
            s.checked_changes.remove(&key);
            s.included_hunks.remove(&key);
        } else {
            s.checked_changes.insert(key.clone());
            if included.len() == exact.len() {
                s.included_hunks.remove(&key);
            } else {
                s.included_hunks.insert(
                    key,
                    IncludedHunks {
                        view: w.data.clone(),
                        hunks: included,
                        exact,
                    },
                );
            }
        }
    });
}

fn current_review() -> Option<WorkingDiffSession> {
    let view = app_signal().with_untracked(|s| s.working_diff.clone());
    if view.as_ref().is_some_and(|w| w.is_dirty()) {
        state::show_toast("warn", "请先保存当前编辑");
        return None;
    }
    view
}

fn lines_for(side: DiffSide) -> Option<Range<usize>> {
    let zone = match side {
        DiffSide::Old => FocusZone::WorkingDiffOld,
        DiffSide::New => FocusZone::WorkingDiffNew,
    };
    let editor = state::active_editor_api(zone)?;
    let selected = editor.selections().into_iter().last()?;
    let (start, end) = if (selected.anchor.line, selected.anchor.column)
        <= (selected.head.line, selected.head.column)
    {
        (selected.anchor, selected.head)
    } else {
        (selected.head, selected.anchor)
    };
    let end_line = (end.line as usize) + usize::from(start.line == end.line || end.column > 0);
    Some(start.line as usize..end_line)
}

fn stage(w: WorkingDiffSession, selection: Selection) {
    let staged = w.data.staged_only;
    state::run_git_action(
        move || crate::git_review_actions::stage_selection(&w.file, &w.data, &selection),
        move |result| match result {
            Ok(()) => state::show_toast(
                "ok",
                if staged {
                    "已取消暂存所选变更"
                } else {
                    "已暂存所选变更"
                },
            ),
            Err(err) => {
                log::warn!("选中差异的暂存操作失败：{err}");
                state::show_toast("warn", err.to_string());
            }
        },
    );
}

pub(super) fn stage_hunk() {
    let Some(w) = current_review() else {
        return;
    };
    if let Some(hunk) = w.computed.hunks.get(w.current_hunk).cloned() {
        stage(w, Selection::Hunks(vec![hunk]));
    }
}

fn stage_lines(side: DiffSide) {
    let Some(w) = current_review() else {
        return;
    };
    if app_signal().with_untracked(|s| s.cfg.diff_single_column) {
        state::show_toast("warn", "请在双栏差异中选择行");
        return;
    }
    let Some(range) = lines_for(side) else {
        return;
    };
    stage(
        w,
        Selection::Lines {
            side: match side {
                DiffSide::Old => Side::Old,
                DiffSide::New => Side::New,
            },
            range,
        },
    );
}

pub(super) fn line_item(side: DiffSide, staged: bool) -> MenuItem {
    MenuItem::new(if staged {
        "取消暂存选中行"
    } else {
        "暂存选中行"
    })
    .on_select(move || stage_lines(side))
}

pub(super) fn line_items(side: DiffSide, staged: bool) -> Vec<MenuItem> {
    let mut items = vec![line_item(side, staged)];
    if !staged {
        items.push(MenuItem::new("回滚当前块").on_select(rollback_diff));
    }
    items
}

fn replace_editor_buffer(editor: &nsh_base::EditorApi, text: String) {
    let caret = editor.selections().last().map(|selection| selection.head);
    editor.run(nsh_base::EditorCommand::SelectAll);
    editor.insert(text);
    if let Some(caret) = caret {
        editor.set_selections(vec![nsh_base::EditorSelection::caret(caret)]);
    }
}

fn rollback_diff() {
    let view = app_signal().with_untracked(|s| s.working_diff.clone());
    let Some(w) = view.filter(|w| w.data.editable && !w.data.staged_only) else {
        state::show_toast("warn", "请在可编辑的工作区差异中选择变更");
        return;
    };
    let Some(hunk) = w.computed.hunks.get(w.current_hunk).cloned() else {
        return;
    };
    let Some(editor) = state::active_editor_api(FocusZone::WorkingDiffNew) else {
        return;
    };
    if editor.value() != w.current_new_text {
        state::show_toast("warn", "差异内容已变化，请重新选择");
        return;
    }
    match selected_changes::apply_selection(
        w.data.old_text.as_bytes(),
        w.current_new_text.as_bytes(),
        &Selection::Hunks(vec![hunk]),
        true,
    ) {
        Ok(bytes) => {
            editor.focus();
            replace_editor_buffer(&editor, String::from_utf8_lossy(&bytes).into_owned());
        }
        Err(err) => state::show_toast("warn", err.to_string()),
    }
}

pub(super) fn rollback_code() {
    let Some(tab) = app_signal().with_untracked(|s| {
        s.code_tabs
            .get(s.active_code_tab_idx)
            .filter(|t| t.editable && t.revision.is_empty())
            .cloned()
    }) else {
        return;
    };
    let Some(editor) = state::active_editor_api(FocusZone::CodeEditor) else {
        return;
    };
    let Some(selection) = editor.selections().last().copied() else {
        return;
    };
    let selected = (selection.anchor.line.min(selection.head.line) as usize)
        ..=(selection.anchor.line.max(selection.head.line) as usize);
    let dir = std::path::Path::new(&tab.file.repo_path);
    let has_base = crate::git_commit::head_oid(dir)
        .ok()
        .flatten()
        .and_then(|head| {
            crate::git_index::tree_file(dir, &head, &tab.file.rel_path)
                .ok()
                .flatten()
        })
        .is_some();
    if !has_base {
        state::show_toast("warn", "该文件没有可回滚的 HEAD 版本");
        return;
    }
    let base = crate::git::read_git_object_text(dir, &format!("HEAD:{}", tab.file.rel_path));
    let Ok((base, false, false)) = base else {
        state::show_toast("warn", "无法读取可回滚的 HEAD 文本");
        return;
    };
    let current = editor.value();
    let selected_hunks = selected_changes::exact_hunks(base.as_bytes(), current.as_bytes())
        .into_iter()
        .filter(|hunk| {
            if hunk.new_start == hunk.new_end {
                selected.contains(&hunk.new_start)
            } else {
                hunk.new_start <= *selected.end() && *selected.start() < hunk.new_end
            }
        })
        .collect::<Vec<_>>();
    if selected_hunks.is_empty() {
        state::show_toast("info", "选区没有可回滚的改动");
        return;
    }
    match selected_changes::apply_selection(
        base.as_bytes(),
        current.as_bytes(),
        &Selection::Hunks(selected_hunks),
        true,
    ) {
        Ok(bytes) => replace_editor_buffer(&editor, String::from_utf8_lossy(&bytes).into_owned()),
        Err(err) => state::show_toast("warn", err.to_string()),
    }
}

pub(super) fn rollback_file(file: crate::git::ChangedFile) {
    if app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .any(|tab| tab.file.key() == file.key() && tab.is_dirty())
            || s.working_diff
                .as_ref()
                .is_some_and(|w| w.file.key() == file.key() && w.is_dirty())
    }) {
        state::show_toast("warn", "该文件有未保存编辑，请先保存");
        return;
    }
    let path = std::path::Path::new(&file.repo_path);
    let head = crate::git_commit::head_oid(path);
    let index = crate::git_index::read_index_entry(path, &file.path);
    let (Ok(head), Ok(index)) = (head, index) else {
        state::show_toast("warn", "文件的 Git 版本无法用于回滚");
        return;
    };
    let snapshot = crate::git::read_review_worktree(std::path::Path::new(&file.abs_path())).version;
    let action = if file.untracked {
        "删除未跟踪文件"
    } else {
        "将暂存区和工作区恢复到 HEAD"
    };
    super::git_prompt::confirm(format!("{action} · {}？", file.path), move || {
        let (file, snapshot, head, index) =
            (file.clone(), snapshot.clone(), head.clone(), index.clone());
        state::run_git_action(
            move || {
                crate::git_review_actions::discard_file(
                    &file,
                    &snapshot,
                    head.as_deref(),
                    index.as_ref(),
                )
            },
            |result| match result {
                Ok(()) => state::show_toast("ok", "已回滚文件"),
                Err(err) => state::show_toast("warn", err.to_string()),
            },
        );
    });
}

pub(super) fn rollback_current() {
    let (focus, selected) =
        app_signal().with_untracked(|s| (s.focus_zone, s.change_selection.clone()));
    match focus {
        FocusZone::CodeEditor => rollback_code(),
        FocusZone::WorkingDiffOld | FocusZone::WorkingDiffNew => rollback_diff(),
        _ => {
            if let Some(selected) = selected {
                rollback_file(selected.file);
            }
        }
    }
}
