use std::ops::Range;

use nsh_base::EditorSelection;
use nsh_core::element::desktop::MenuItem;

use crate::diff_model::{DiffHunk, DiffSide, UnifiedLineRef};
use crate::git::ReviewChangeData;
use crate::selected_changes::{self, LineSelection, Selection};
use crate::state::{self, AppState, EditorTabKind, FocusZone, WorkingDiffSession, app_signal};

#[derive(Clone)]
pub(crate) struct IncludedHunks {
    pub view: ReviewChangeData,
    pub hunks: Vec<DiffHunk>,
    exact: Vec<DiffHunk>,
}

fn can_include(w: &WorkingDiffSession) -> bool {
    w.can_compare_text()
        && !w.data.staged_only
        && !w.file.staged
        && !w.file.untracked
        && !w.file.conflict
        && w.file.kind == "M"
}

#[derive(Clone)]
struct ReviewMenuTarget {
    file_key: String,
    content_rev: u64,
    staged: bool,
}

impl ReviewMenuTarget {
    fn capture(w: &WorkingDiffSession) -> Self {
        Self { file_key: w.file.key(), content_rev: w.content_rev, staged: w.data.staged_only }
    }

    fn matches(&self, w: &WorkingDiffSession) -> bool {
        self.file_key == w.file.key()
            && self.content_rev == w.content_rev
            && self.staged == w.data.staged_only
    }
}

#[derive(Clone)]
struct HunkMenuTarget {
    review: ReviewMenuTarget,
    hunk: DiffHunk,
}

impl HunkMenuTarget {
    fn matches(&self, w: &WorkingDiffSession) -> bool {
        self.review.matches(w) && w.computed.hunks.contains(&self.hunk)
    }
}

fn included_state(s: &AppState, w: &WorkingDiffSession, hunk: &DiffHunk) -> Option<bool> {
    if !can_include(w) || w.is_dirty() {
        return None;
    }
    let key = w.file.key();
    let Some(part) = s.included_hunks.get(&key) else {
        return Some(s.checked_changes.contains(&key));
    };
    if !same_versions(&part.view, &w.data) {
        return Some(false);
    }
    let index = selected_changes::exact_hunk_index(&part.exact, hunk).ok()?;
    Some(part.hunks.contains(&part.exact[index]))
}

fn same_versions(a: &ReviewChangeData, b: &ReviewChangeData) -> bool {
    a.repo_id == b.repo_id
        && a.new_path == b.new_path
        && a.staged_only == b.staged_only
        && a.version == b.version
        && a.index_snapshot == b.index_snapshot
}

fn toggle_included_hunk(target: &HunkMenuTarget) {
    app_signal().update(|s| {
        let Some(w) = s
            .working_diff
            .as_ref()
            .filter(|w| target.matches(w) && can_include(w) && !w.is_dirty())
        else {
            return;
        };
        let exact =
            selected_changes::exact_hunks(w.data.old_text.as_bytes(), w.saved_new_text.as_bytes());
        let Ok(at) = selected_changes::exact_hunk_index(&exact, &target.hunk) else {
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

fn current_review(target: &ReviewMenuTarget) -> Option<WorkingDiffSession> {
    let view = app_signal().with_untracked(|s| {
        s.working_diff.as_ref().filter(|w| s.editor_tab == EditorTabKind::WorkingDiff && target.matches(w)).cloned()
    });
    if view.is_none() {
        state::show_toast("warn", "差异内容已变化，请重新选择");
        return None;
    }
    if let Some(issue) = view.as_ref().and_then(WorkingDiffSession::read_failure_notice) {
        state::show_toast("warn", issue);
        return None;
    }
    if view.as_ref().is_some_and(|w| w.is_dirty()) {
        state::show_toast("warn", "请先保存当前编辑");
        return None;
    }
    view
}

fn selected_display_rows(clicked: usize, selections: &[EditorSelection]) -> Vec<Range<usize>> {
    let mut ranges: Vec<_> = selections.iter().filter(|selection| selection.anchor != selection.head).map(|selection| {
        let mut endpoints = [
            (selection.anchor.line, selection.anchor.column),
            (selection.head.line, selection.head.column),
        ];
        endpoints.sort_unstable();
        let [(start, _), (end, column)] = endpoints;
        start as usize..end as usize + usize::from(column > 0)
    }).collect();
    if !ranges.iter().any(|range| range.contains(&clicked)) {
        return vec![clicked..clicked + 1];
    }
    ranges.sort_unstable_by_key(|range| range.start);
    ranges
}

fn selected_changed_lines(
    w: &WorkingDiffSession,
    side: DiffSide,
    clicked: usize,
    projection: Option<&[UnifiedLineRef]>,
) -> LineSelection {
    let zone = match side {
        DiffSide::Old => FocusZone::WorkingDiffOld,
        DiffSide::New => FocusZone::WorkingDiffNew,
    };
    let selections = state::active_editor_api(zone).map_or_else(Vec::new, |editor| editor.selections());
    let mut selected = LineSelection::default();
    for rows in selected_display_rows(clicked, &selections) {
        if let Some(projection) = projection {
            for row in projection[rows.start.min(projection.len())..rows.end.min(projection.len())]
                .iter().filter(|row| row.is_changed)
            {
                selected.push(row.side, row.orig_line..row.orig_line + 1);
            }
        } else {
            let hunks = &w.computed.hunks;
            let first = hunks.partition_point(|hunk| hunk.line_range(side).end <= rows.start);
            for hunk in &hunks[first..] {
                let changed = hunk.line_range(side);
                if changed.start >= rows.end {
                    break;
                }
                selected.push(side, rows.start.max(changed.start)..rows.end.min(changed.end));
            }
        }
    }
    selected
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

pub(super) fn line_items(
    side: DiffSide,
    line: usize,
    clicked_display_row: usize,
    projection: Option<&[UnifiedLineRef]>,
) -> Vec<MenuItem> {
    app_signal().with_untracked(|s| {
        let Some(w) = s.working_diff.as_ref().filter(|w| w.can_compare_text()) else {
            return Vec::new();
        };
        let review = ReviewMenuTarget::capture(w);
        let mut items = Vec::new();
        let selected = selected_changed_lines(w, side, clicked_display_row, projection);
        if !selected.is_empty() {
            let target = review.clone();
            let label = if review.staged { "取消暂存选中行" } else { "暂存选中行" };
            items.push(MenuItem::new(label).on_select(move || {
                if let Some(w) = current_review(&target) {
                    stage(w, Selection::Lines(selected.clone()));
                }
            }));
        }
        if let Some(hunk) = w.computed.hunks.iter().find(|hunk| hunk.line_range(side).contains(&line)) {
            let target = HunkMenuTarget {
                review,
                hunk: hunk.clone(),
            };
            let stage_target = target.clone();
            let stage_label = if target.review.staged { "取消暂存此块" } else { "暂存此块" };
            items.push(MenuItem::new(stage_label).on_select(move || {
                if let Some(w) = current_review(&stage_target.review).filter(|w| stage_target.matches(w)) {
                    stage(w, Selection::Hunks(vec![stage_target.hunk.clone()]));
                }
            }));
            if w.editor_editable() {
                let rollback_target = target.clone();
                items.push(MenuItem::new("回滚当前块").on_select(move || rollback_diff(Some(&rollback_target))));
            }
            if let Some(included) = included_state(s, w, hunk) {
                items.push(
                    MenuItem::new("纳入本次提交")
                        .checked(included)
                        .on_select(move || toggle_included_hunk(&target)),
                );
            }
        }
        items
    })
}

fn replace_editor_buffer(editor: &nsh_base::EditorApi, text: String) {
    let caret = editor.selections().last().map(|selection| selection.head);
    editor.run(nsh_base::EditorCommand::SelectAll);
    editor.insert(text);
    if let Some(caret) = caret {
        editor.set_selections(vec![nsh_base::EditorSelection::caret(caret)]);
    }
}

fn rollback_diff(target: Option<&HunkMenuTarget>) {
    let (view, unified) = app_signal().with_untracked(|s| {
        let view = s.working_diff.as_ref().filter(|_| s.editor_tab == EditorTabKind::WorkingDiff).cloned();
        (view, s.cfg.diff_single_column)
    });
    let Some(w) = view.filter(|w| w.editor_editable() && w.can_compare_text()) else {
        state::show_toast("warn", "请在可编辑的工作区差异中选择变更");
        return;
    };
    let hunk = if let Some(target) = target {
        if !target.matches(&w) {
            state::show_toast("warn", "差异内容已变化，请重新选择");
            return;
        }
        Some(&target.hunk)
    } else {
        let split_text = (!unified).then_some(w.active_text());
        w.computed.hunk_navigation(w.active_side, w.active_cursor(), split_text).current
            .and_then(|index| w.computed.hunks.get(index))
    };
    let Some(hunk) = hunk else {
        state::show_toast("info", "光标处没有可回滚的改动");
        return;
    };
    let Some(editor) = state::active_editor_api(FocusZone::WorkingDiffNew) else {
        return;
    };
    if editor.source_value() != w.current_new_text {
        state::show_toast("warn", "差异内容已变化，请重新选择");
        return;
    }
    match selected_changes::apply_selection(
        w.data.old_text.as_bytes(),
        w.current_new_text.as_bytes(),
        &Selection::Hunks(vec![hunk.clone()]),
        true,
    ) {
        Ok(bytes) => {
            editor.focus();
            editor.set_source_value(String::from_utf8_lossy(&bytes).into_owned());
        }
        Err(err) => state::show_toast("warn", err.to_string()),
    }
}

pub(super) fn rollback_code() {
    let Some(tab) = app_signal().with_untracked(|s| {
        s.code_tabs
            .get(s.active_code_tab_idx)
            .filter(|t| t.editor_editable() && t.revision.is_empty())
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
    let base_entry = crate::git_commit::head_oid(dir)
        .ok()
        .flatten()
        .and_then(|head| {
            crate::git_index::tree_file(dir, &head, &tab.file.rel_path)
                .ok()
                .flatten()
        });
    let Some(base_entry) = base_entry else {
        state::show_toast("warn", "该文件没有可回滚的 HEAD 版本");
        return;
    };
    let base = crate::git::read_required_git_object_text(dir, &base_entry.oid);
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
        FocusZone::WorkingDiffOld | FocusZone::WorkingDiffNew => rollback_diff(None),
        _ => {
            if let Some(selected) = selected {
                rollback_file(selected.file);
            }
        }
    }
}
