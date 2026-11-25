use std::{cell::RefCell, collections::HashSet, path::Path, rc::Rc};

use nsh_base::{
    Editor, EditorApi, EditorContextMenuEvent, EditorHoverEvent, EditorPosition, EditorSearch,
    EditorSearchResult, EditorSelection, editor, };
use nsh_base::editor::EditorProjection;
use nsh_core::element::desktop::MenuItem;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::diff_model::{
    DiffHunk, DiffSide, InlineHighlightMode, UnifiedDiffDoc, UnifiedLineRef, WhitespaceMode, build_side_by_side_decorations,
    build_unified_diff, compute_unchanged_fold_ranges, compute_unified_layout, unified_source_eof,
};
use crate::state::{
    self, AppState, EditorTabKind, FocusZone, DocumentUndo, app_signal, current_palette, editor_refs_signal, jump_diff_hunk,
    jump_from_diff_to_source, move_change_selection, move_commit_diff_file, open_find_bar,
    set_inline_mode, set_whitespace_mode,
};
use crate::theme::Palette;
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

pub(super) fn reveal_comparison_hunk(hunk: &DiffHunk, old: ComponentRef<Editor>, new: ComponentRef<Editor>) {
    for (reference, line) in [(old, hunk.old_start), (new, hunk.new_start)] {
        if let Some(api) = reference.api::<EditorApi>() {
            let position = EditorPosition::new(line as u32, 0);
            api.set_selections(vec![EditorSelection::caret(position)]);
            api.reveal_position(position, true);
        }
    }
}

pub(super) fn bind_comparison_hunk_keys(
    enabled: impl Fn() -> bool + 'static,
    navigate: impl Fn(i32) + Copy + 'static,
) {
    let keymap = super::keymap_memo();
    effect(move || {
        if enabled() {
            let keys = keymap.get();
            let options = KeyOptions { when: When::Always, ..Default::default() };
            super::bind_action(&keys, "previousDifference", options, move |_| navigate(-1));
            super::bind_action(&keys, "nextDifference", options, move |_| navigate(1));
        }
    });
}

fn toolbar_icon_btn(
    label: impl IntoReactive<SharedString>,
    svg_src: &'static str,
    active: impl IntoReactive<bool>,
    enabled: impl IntoReactive<bool>,
    tid: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into_reactive();
    let tip = label.clone();
    let active = active.into_reactive();
    let background = active.clone();
    let enabled = enabled.into_reactive();
    let disabled = enabled.clone().map(|enabled| !enabled);
    super::controls::button()
        .test_id(tid)
        .h(px(24.0))
        .px(px(6.0))
        .gap(px(4.0))
        .rounded(px(4.0))
        .bg(move || {
            if background.get() {
                current_palette().accent_soft
            } else {
                transparent_black()
            }
        })
        .cursor(Cursor::Pointer)
        .style_when(disabled.clone(), |s| s.cursor(Cursor::Default))
        .disabled(disabled)
        .hover(|s| s.bg(move || current_palette().surface_hover))
        .label(label)
        .tooltip(dynamic(move || {
            super::tooltip_label(tip.get().to_string(), current_palette())
        }))
        .on_press(on_click)
        .child(icons::icon(svg_src, 12.0, move || {
            let p = current_palette();
            if !enabled.get() {
                p.text_muted.opacity(0.45)
            } else if active.get() {
                p.accent_soft_text
            } else {
                p.text_control
            }
        }))
}

fn toolbar_text_btn(
    label: String,
    active: impl IntoReactive<bool>,
    tid: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    let active = active.into_reactive();
    let background = active.clone();
    super::controls::button()
        .test_id(tid)
        .h(px(24.0))
        .px(px(8.0))
        .rounded(px(4.0))
        .bg(move || {
            let p = current_palette();
            if background.get() {
                p.accent_soft
            } else {
                p.surface2
            }
        })
        .cursor(Cursor::Pointer)
        .hover(|s| s.bg(move || current_palette().surface_hover))
        .label(label.clone())
        .on_press(on_click)
        .child(
            text(label)
                .text_size(px(11.0))
                .text_color(move || {
                    let p = current_palette();
                    if active.get() {
                        p.accent_soft_text
                    } else {
                        p.text_control
                    }
                })
                .number_of_lines(1u32),
        )
}

fn toolbar_shortcut_hint(keys: Memo<crate::keymap::Keymap>, label: &str, action: &str) -> String {
    keys.with(|keys| {
        keys.keys(action)
            .first()
            .and_then(|key| nsh_core::keymap::Keystroke::parse_with(key, apple_keys()).ok())
            .map_or_else(
                || label.to_owned(),
                |key| format!("{label} ({})", key.label(os(), apple_keys())),
            )
    })
}

pub(super) fn diff_find_button(id: &'static str, keys: Memo<crate::keymap::Keymap>) -> impl IntoElement {
    toolbar_icon_btn(
        move || toolbar_shortcut_hint(keys, "查找", "find"),
        ICO_SEARCH,
        move || app_signal().with(|s| s.active_find_open()),
        true,
        id,
        || open_find_bar(false),
    )
}

fn show_diff_display_menu() {
    let (whitespace, inline, wrap, show_whitespace, single, sync_scroll) = app_signal().with_untracked(|s| {
        (
            WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode),
            InlineHighlightMode::from_config(&s.cfg.diff_inline_mode),
            s.cfg.editor_soft_wrap,
            s.cfg.diff_show_whitespaces,
            s.cfg.diff_single_column,
            s.cfg.diff_sync_scroll,
        )
    });
    let whitespace_options: Vec<_> = [
        WhitespaceMode::None,
        WhitespaceMode::Trim,
        WhitespaceMode::All,
        WhitespaceMode::AllAndEmptyLines,
    ]
    .into_iter()
    .map(|mode| MenuItem::new(mode.label()).checked(mode == whitespace)
        .on_select(move || set_whitespace_mode(mode)))
    .collect();
    let inline_options: Vec<_> = [InlineHighlightMode::Word, InlineHighlightMode::Char, InlineHighlightMode::None]
        .into_iter()
        .map(|mode| MenuItem::new(mode.label()).checked(mode == inline)
            .on_select(move || set_inline_mode(mode)))
        .collect();
    let mut items = vec![
        MenuItem::submenu("空白比较", whitespace_options),
        MenuItem::submenu("行内高亮", inline_options),
        MenuItem::separator(),
        MenuItem::new("自动换行").checked(wrap).on_select(|| {
            app_signal().update(|s| s.cfg.editor_soft_wrap = !s.cfg.editor_soft_wrap);
            state::save_config_quiet();
        }),
        MenuItem::new("显示空白字符").checked(show_whitespace).on_select(|| {
            app_signal().update(|s| s.cfg.diff_show_whitespaces = !s.cfg.diff_show_whitespaces);
            state::save_config_quiet();
        }),
    ];
    if !single {
        items.push(MenuItem::new("同步滚动").checked(sync_scroll).on_select(|| {
            app_signal().update(|s| s.cfg.diff_sync_scroll = !s.cfg.diff_sync_scroll);
            state::save_config_quiet();
        }));
    }
    state::show_popup_menu(items);
}

#[derive(Clone, Default, PartialEq)]
struct DiffControls {
    visible: bool,
    repo: String,
    path: String,
    file_path: String,
    revisions: String,
    added: usize,
    deleted: usize,
    hunks: usize,
    hunk: usize,
    editable: bool,
    can_open_source: bool,
    can_switch_files: bool,
    conflict: bool,
    notice: Option<&'static str>,
    comparable: bool,
    single: bool,
    collapse: bool,
    boundary: Option<i32>,
}

fn diff_controls(
    is_commit: bool,
    toggle_layout: impl Fn() + Copy + 'static,
) -> impl IntoElement {
    let controls = computed(move || {
        app_signal().with(|s| {
            let path_label = |old: &str, new: &str| {
                if old == new {
                    new.to_owned()
                } else {
                    format!("{old} → {new}")
                }
            };
            let (single, collapse) = (s.cfg.diff_single_column, s.cfg.diff_collapse_unchanged);
            if is_commit {
                let Some(diff) = &s.commit_diff else {
                    return DiffControls::default();
                };
                let split_text = (!single).then_some(match diff.active_side {
                    DiffSide::Old => diff.data.old_text.as_str(),
                    DiffSide::New => diff.data.new_text.as_str(),
                });
                DiffControls {
                    visible: true,
                    repo: diff.commit.repo_name.clone(),
                    path: path_label(&diff.data.old_path, &diff.data.new_path),
                    file_path: diff.data.new_path.clone(),
                    revisions: format!("{} → {}", diff.data.old_title, diff.data.new_title),
                    added: diff.computed.added_lines,
                    deleted: diff.computed.deleted_lines,
                    hunks: diff.computed.hunks.len(),
                    hunk: diff.computed.hunk_navigation(diff.active_side, diff.active_cursor(), split_text).counter_index(),
                    can_open_source: diff.data.status != "D",
                    can_switch_files: diff.scope == state::CommitDiffScope::AllFiles,
                    comparable: !diff.data.binary && !diff.data.too_large,
                    single,
                    collapse,
                    boundary: diff.boundary_hint,
                    ..DiffControls::default()
                }
            } else {
                let Some(diff) = &s.working_diff else {
                    return DiffControls::default();
                };
                let split_text = (!single).then_some(diff.active_text());
                DiffControls {
                    visible: true,
                    repo: diff.file.repo_name.clone(),
                    path: path_label(&diff.data.old_path, &diff.data.new_path),
                    file_path: diff.data.new_path.clone(),
                    revisions: format!("{} → {}", diff.data.old_title, diff.data.new_title),
                    added: diff.computed.added_lines,
                    deleted: diff.computed.deleted_lines,
                    hunks: diff.computed.hunks.len(),
                    hunk: diff.computed.hunk_navigation(diff.active_side, diff.active_cursor(), split_text).counter_index(),
                    editable: diff.editor_editable(),
                    can_open_source: Path::new(&diff.file.abs_path()).is_file(),
                    can_switch_files: true,
                    conflict: diff.file.conflict,
                    notice: diff
                        .read_failure_notice()
                        .or_else(|| diff.disk_state.notice()),
                    comparable: diff.can_compare_text(),
                    single,
                    collapse,
                    boundary: diff.boundary_hint,
                }
            }
        })
    });
    // Keep the button elements mounted when a new comparison arrives. AppKit
    // must deliver mouse up to the same control that handled mouse down.
    when(
        move || controls.with(|d| d.visible),
        move || {
            view()
                .flex_col()
                .w_full()
                .bg(move || current_palette().rail_bg)
                .border_b_1()
                .border_color(move || current_palette().border)
                .child(diff_toolbar(controls, is_commit, toggle_layout))
                .child(when(
                    move || controls.with(|d| d.notice.is_some()),
                    move || {
                        view().px(px(10.0)).py(px(4.0)).test_id("diff-disk-state").child(
                            text(move || controls.with(|d| d.notice.unwrap_or_default()))
                                .text_size(px(11.0))
                                .text_color(move || current_palette().amber),
                        )
                    },
                ))
        },
    )
}

fn diff_details(controls: Memo<DiffControls>) -> impl IntoElement {
    let summary = move || controls.with(|d| {
        let details = format!("{} · {}\n{}", d.repo, d.path, d.revisions);
        if d.comparable {
            format!("{details}    +{}  −{}", d.added, d.deleted)
        } else {
            details
        }
    });
    super::controls::button()
        .test_id("diff-details")
        .w(px(24.0))
        .h(px(24.0))
        .flex_none()
        .rounded(px(4.0))
        .cursor(Cursor::Pointer)
        .label("差异详情")
        .hover(|style| style.bg(move || current_palette().surface_hover))
        .tooltip(dynamic(move || {
            let p = current_palette();
            view().max_w(px(460.0)).px(px(8.0)).py(px(5.0)).rounded(px(5.0)).bg(p.surface)
                .border_1().border_color(p.border)
                .child(text(summary()).text_left().text_size(px(11.0)).text_color(p.text_primary))
        }).into_element())
        .on_press(move || {
            let details = controls.get_untracked();
            let mut items = vec![
                MenuItem::new(format!("{} · {}", details.repo, details.path)).disabled(true),
                MenuItem::new(details.revisions).disabled(true),
            ];
            if details.comparable {
                items.push(MenuItem::new(format!("+{} / −{}", details.added, details.deleted)).disabled(true));
            }
            items.push(MenuItem::new("复制相对路径").on_select(move || set_clipboard(details.file_path.clone())));
            state::show_popup_menu(items);
        })
        .child(icons::icon(ICO_INFO, 14.0, move || current_palette().text_control))
}

fn diff_toolbar(
    controls: Memo<DiffControls>,
    is_commit: bool,
    toggle_layout: impl Fn() + 'static,
) -> impl IntoElement {
    let comparable = move || controls.with(|d| d.comparable);
    let previous_file = move || controls.with(|d| d.boundary == Some(-1));
    let next_file = move || controls.with(|d| d.boundary == Some(1));
    let collapse = move || controls.with(|d| d.collapse);
    let single = move || controls.with(|d| d.single);
    let move_file = move |delta| {
        if is_commit {
            move_commit_diff_file(delta);
        } else {
            move_change_selection(delta, true);
        }
    };
    let keys = super::keymap_memo();
    let shortcut_hint = move |label: &str, action| toolbar_shortcut_hint(keys, label, action);
    view()
        .test_id("diff-toolbar")
        .h(px(32.0))
        .px(px(10.0))
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(toolbar_icon_btn(
            move || {
                shortcut_hint(
                    if previous_file() {
                        "切到上一个文件"
                    } else {
                        "上一处差异"
                    },
                    "previousDifference",
                )
            },
            ICO_CHEVRON_UP,
            previous_file,
            comparable,
            "diff-prev-hunk",
            || jump_diff_hunk(-1),
        ))
        .child(when(comparable, move || {
            text(move || {
                controls.with(|d| {
                    if d.hunks == 0 {
                        "0 处差异".to_owned()
                    } else {
                        format!("{}/{}", (d.hunk + 1).min(d.hunks), d.hunks)
                    }
                })
            })
            .test_id("diff-hunk-counter")
            .text_size(px(11.0))
            .text_color(move || current_palette().text_secondary)
        }))
        .child(toolbar_icon_btn(
            move || {
                shortcut_hint(
                    if next_file() {
                        "切到下一个文件"
                    } else {
                        "下一处差异"
                    },
                    "nextDifference",
                )
            },
            ICO_CHEVRON_DOWN,
            next_file,
            comparable,
            "diff-next-hunk",
            || jump_diff_hunk(1),
        ))
        .child(when(
            move || controls.with(|d| d.can_open_source),
            move || {
                toolbar_icon_btn(
                    move || shortcut_hint("跳转到源码", "jumpToSource"),
                    ICO_EDIT,
                    false,
                    true,
                    "diff-jump-source-btn",
                    jump_from_diff_to_source,
                )
            },
        ))
        .child(toolbar_icon_btn(
            move || {
                if collapse() {
                    "展开未改动"
                } else {
                    "折叠未改动"
                }
            },
            ICO_COLLAPSE,
            collapse,
            true,
            "diff-collapse-btn",
            || {
                app_signal()
                    .update(|s| s.cfg.diff_collapse_unchanged = !s.cfg.diff_collapse_unchanged);
                state::save_config_quiet();
            },
        ))
        .child(diff_find_button("diff-find-btn", keys))
        .child(toolbar_icon_btn(
            move || {
                if single() {
                    "切换为双栏"
                } else {
                    "切换为单栏"
                }
            },
            ICO_COLUMNS,
            single,
            true,
            "diff-layout-toggle",
            toggle_layout,
        ))
        .child((!is_commit).then(|| {
            toolbar_icon_btn(
                "HEAD、暂存区与工作区对比",
                ICO_BRANCH,
                false,
                true,
                "diff-index-review",
                super::index_review::open_active,
            )
        }))
        .child(toolbar_icon_btn(
            "差异显示选项",
            ICO_SETTINGS,
            false,
            true,
            "diff-options-btn",
            show_diff_display_menu,
        ))
        .child(diff_details(controls))
        .child(when(move || controls.with(|d| d.conflict), || {
            toolbar_text_btn("解决冲突".to_owned(), true, "diff-open-conflicts", || {
                let file = app_signal().with_untracked(|s| {
                    s.working_diff.as_ref().filter(|diff| diff.file.conflict)
                        .map(|diff| (diff.file.repo_path.clone(), diff.file.path.clone()))
                });
                if let Some((repo, path)) = file {
                    super::conflict_workbench::open_file(repo.into(), path);
                }
            })
        }))
        .child(when(move || controls.with(|d| d.single && !d.editable), || {
            text("只读").text_size(px(10.5)).text_color(move || current_palette().text_muted)
        }))
        .child(view().flex_1())
        .child(toolbar_icon_btn(
            move || shortcut_hint("上一个文件", "previousDiffFile"),
            ICO_ARROW_LEFT,
            false,
            move || controls.with(|d| d.can_switch_files),
            "diff-prev-file",
            move || move_file(-1),
        ))
        .child(toolbar_icon_btn(
            move || shortcut_hint("下一个文件", "nextDiffFile"),
            ICO_ARROW_RIGHT,
            false,
            move || controls.with(|d| d.can_switch_files),
            "diff-next-file",
            move || move_file(1),
        ))
}

/// 统一渲染工作区 Diff（`is_commit = false`）或历史提交 Diff（`is_commit = true`）。
pub fn diff_pane(is_commit: bool) -> impl IntoElement {
    let old_ref = component_ref::<Editor>();
    let new_ref = component_ref::<Editor>();
    let unified_ref = component_ref::<Editor>();
    editor_refs_signal().update(|r| {
        if is_commit {
            r.commit_old = Some(old_ref);
            r.commit_new = Some(new_ref);
            r.commit_unified = Some(unified_ref);
        } else {
            r.work_old = Some(old_ref);
            r.work_new = Some(new_ref);
            r.work_unified = Some(unified_ref);
        }
    });
    let tab = if is_commit { state::EditorTabKind::CommitDiff } else { state::EditorTabKind::WorkingDiff };
    state::restore_diff_view_after_render(tab, false);

    // 同步文本与折叠状态到双栏 Editor（按 content_rev 避免打字递归覆盖）
    let sync_memo = computed(move || {
        app_signal().with(|s| {
            let collapse = s.cfg.diff_collapse_unchanged;
            let single = s.cfg.diff_single_column;
            if is_commit {
                if let Some(c) = &s.commit_diff {
                    (c.content_rev, collapse, single)
                } else {
                    (0, collapse, single)
                }
            } else if let Some(w) = &s.working_diff {
                (w.content_rev, collapse, single)
            } else {
                (0, collapse, single)
            }
        })
    });
    let scrolled_side = stored(None);
    let remember_scroll = move |side: Option<DiffSide>| {
        scrolled_side.replace(side.map(|side| (sync_memo.with_untracked(|s| s.0), side)));
    };
    let last_rev = stored(0u64);
    let last_collapse = stored(false);
    effect(move || {
        let (rev, collapse, single) = sync_memo.get();
        if rev == 0 {
            return;
        }
        let (old_text, new_text, hunks) = app_signal().with_untracked(|s| {
            if is_commit {
                s.commit_diff
                    .as_ref()
                    .map(|c| {
                        (
                            c.data.old_text.clone(),
                            c.data.new_text.clone(),
                            c.computed.hunks.clone(),
                        )
                    })
                    .unwrap_or_default()
            } else {
                s.working_diff
                    .as_ref()
                    .map(|w| {
                        (
                            w.data.old_text.clone(),
                            w.current_new_text.clone(),
                            w.computed.hunks.clone(),
                        )
                    })
                    .unwrap_or_default()
            }
        });
        let rev_changed = last_rev.get() != rev;
        let collapse_changed = last_collapse.get() != collapse;
        if rev_changed {
            last_rev.replace(rev);
            if !single {
                if let Some(api) = old_ref.api::<EditorApi>() {
                    if api.value() != old_text {
                        api.set_value(old_text.clone());
                    }
                }
                if let Some(api) = new_ref.api::<EditorApi>() {
                    if api.value() != new_text {
                        if is_commit {
                            api.set_value(new_text.clone());
                        } else {
                            api.reload_value(new_text.clone());
                        }
                    }
                }
                if !crate::history_runtime::is_open()
                    && !state::text_input_focus_signal().with_untracked(|f| f.focused)
                {
                    let zone = app_signal().with_untracked(|s| s.focus_zone);
                    match (is_commit, zone) {
                        (true, FocusZone::CommitDiffOld) | (false, FocusZone::WorkingDiffOld) => {
                            if let Some(api) = old_ref.api::<EditorApi>() {
                                api.focus();
                            }
                        }
                        (true, FocusZone::CommitDiffNew) | (false, FocusZone::WorkingDiffNew) => {
                            if let Some(api) = new_ref.api::<EditorApi>() {
                                api.focus();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if rev_changed || collapse_changed {
            last_collapse.replace(collapse);
            if !single {
                let old_lines = crate::diff_model::logical_line_count(&old_text);
                let new_lines = crate::diff_model::logical_line_count(&new_text);
                let old_ranges: Vec<_> = hunks.iter().map(|h| h.old_start..h.old_end).collect();
                let new_ranges: Vec<_> = hunks.iter().map(|h| h.new_start..h.new_end).collect();
                let old_folds = if collapse {
                    compute_unchanged_fold_ranges(old_lines, &old_ranges, 3)
                } else {
                    Vec::new()
                };
                let new_folds = if collapse {
                    compute_unchanged_fold_ranges(new_lines, &new_ranges, 3)
                } else {
                    Vec::new()
                };
                if let Some(api) = old_ref.api::<EditorApi>() {
                    api.set_folds(old_folds);
                }
                if let Some(api) = new_ref.api::<EditorApi>() {
                    api.set_folds(new_folds);
                }
            } else if let Some(api) = unified_ref.api::<EditorApi>() {
                let unified_folds = if collapse {
                    compute_unified_layout(&old_text, &new_text, &hunks).2
                } else {
                    Vec::new()
                };
                api.set_folds(unified_folds);
            }
        }
    });

    let body_mode_memo = computed(move || {
        app_signal().with(|s| {
            let single = s.cfg.diff_single_column;
            let (msg, rev_for_single, undo_history) = if is_commit {
                s.commit_diff
                    .as_ref()
                    .map(|c| {
                        let m = if c.data.too_large {
                            Some("提交文件超过 2 MiB 审阅上限，已只读拦截")
                        } else if c.data.binary {
                            Some("二进制或非 UTF-8 文件，无法以文本对比")
                        } else {
                            None
                        };
                        (m, if single { c.content_rev } else { 0 }, None)
                    })
                    .unwrap_or((None, 0, None))
            } else {
                s.working_diff
                    .as_ref()
                    .map(|w| {
                        let m = w.data.text_block().map(crate::git::ReviewTextBlock::comparison_notice);
                        (m, if single { w.content_rev } else { 0 }, Some(w.undo_history.clone()))
                    })
                    .unwrap_or((None, 0, None))
            };
            (single, msg, rev_for_single, undo_history)
        })
    });

    view()
        .test_id(if is_commit { "commit-diff-pane" } else { "working-diff-pane" })
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .bg(move || current_palette().surface)
        .child(diff_controls(is_commit, move || state::toggle_diff_layout(scrolled_side.get())))
        .child(diff_find_bar(tab))
        // 主体内容：仅在双栏/单栏切换或拦截态变化时重建，保持双 Editor 实例稳定不循环重建！
        .child(dynamic(move || {
            let p = current_palette();
            let (single, blocked_msg, _single_rev, undo_history) = body_mode_memo.get();

            if let Some(msg) = blocked_msg {
                return view()
                    .test_id("diff-blocked-notice")
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(text(msg).text_size(px(12.5)).text_color(p.text_muted))
                    .into_element();
            }

            if single {
                render_single_column_diff(is_commit, unified_ref, p).into_element()
            } else {
                render_side_by_side_diff(is_commit, old_ref, new_ref, p, remember_scroll, undo_history)
                    .into_element()
            }
        }))
}

pub(super) fn diff_find_bar(tab: EditorTabKind) -> impl IntoElement {
    let find_input = element_ref();
    editor_refs_signal().update(|refs| refs.diff_find_input = Some(find_input));
    when(
        move || app_signal().with(|s| s.editor_tab == tab && s.active_find_open()),
        move || {
            let p = current_palette();
            view()
                .test_id("diff-find-bar")
                .h(px(34.0))
                .px(px(12.0))
                .gap(px(6.0))
                .flex_row()
                .items_center()
                .bg(move || current_palette().rail_bg)
                .border_b_1()
                .border_color(move || current_palette().border)
                .child(
                    view()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(move || current_palette().accent_soft)
                        .child(
                            text(move || {
                                match app_signal().with(|s| s.diff_find_side_for(tab)) {
                                    None => "查找单栏",
                                    Some(DiffSide::Old) => "查找左侧",
                                    Some(DiffSide::New) => "查找右侧",
                                }
                            })
                            .test_id("diff-find-side-badge")
                            .text_size(px(10.5))
                            .text_color(move || current_palette().accent_soft_text),
                        ),
                )
                .child(
                    super::controls::text_field(24.0, 11.5, 8.0)
                        .test_id("diff-find-input")
                        .element_ref(&find_input)
                        .flex_1()
                        .min_w(px(80.0))
                        .max_w(px(220.0))
                        .rounded(px(4.0))
                        .bg(move || current_palette().surface)
                        .border_1()
                        .border_color(move || current_palette().border)
                        .text_color(move || current_palette().text_primary)
                        .placeholder("在当前聚焦侧查找…")
                        .value(move || app_signal().with(|s| s.diff_search.query.to_string()))
                        .on_focus(|| state::set_text_input_focused(true))
                        .on_blur(|| state::set_text_input_focused(false))
                        .on_key_press(super::editor_find::handle_input_key)
                        .on_change(|v: &str| {
                            app_signal().update(|s| s.diff_search.query = v.to_owned().into());
                        })
                        .on_submit(move |_: &str| state::run_active_find_nav(true)),
                )
                .child(super::editor_find::FindBar::Diff.options())
                .child(
                    text(move || app_signal().with(|s| super::editor_find::result_text(&s.diff_find_matches)))
                    .test_id("diff-find-matches")
                    .text_size(px(11.0))
                    .text_color(move || current_palette().text_muted),
                )
                .child(toolbar_text_btn(
                    "上一个".to_string(),
                    false,
                    "diff-find-prev",
                    || state::run_active_find_nav(false),
                ))
                .child(toolbar_text_btn(
                    "下一个".to_string(),
                    false,
                    "diff-find-next",
                    || state::run_active_find_nav(true),
                ))
                .child(view().flex_1())
                .child(
                    super::controls::button()
                        .w(px(20.0))
                        .h(px(20.0))
                        .cursor(Cursor::Pointer)
                        .on_press(state::close_find_bar)
                        .child(icons::icon(ICO_CLOSE, 10.0, p.text_muted)),
                )
        },
    )
}

fn find_is_on(s: &AppState, tab: EditorTabKind, side: Option<DiffSide>) -> bool {
    s.editor_tab == tab && s.active_find_open() && s.diff_find_side_for(tab) == side
}

pub(super) fn diff_search(tab: EditorTabKind, side: Option<DiffSide>) -> Option<EditorSearch> {
    app_signal().with(|s| {
        (find_is_on(s, tab, side) && !s.diff_search.query.is_empty()).then(|| s.diff_search.clone())
    })
}

pub(super) fn record_diff_search(tab: EditorTabKind, side: Option<DiffSide>, result: &EditorSearchResult) {
    if app_signal().with_untracked(|s| find_is_on(s, tab, side) && s.diff_find_matches != *result) {
        app_signal().update(|s| s.diff_find_matches = result.clone());
    }
}

fn render_side_by_side_diff(
    is_commit: bool,
    old_ref: ComponentRef<Editor>,
    new_ref: ComponentRef<Editor>,
    p: Palette,
    remember_scroll: impl Fn(Option<DiffSide>) + Copy + 'static,
    undo_history: Option<DocumentUndo>,
) -> impl IntoElement {
    let tab = if is_commit { EditorTabKind::CommitDiff } else { EditorTabKind::WorkingDiff };
    if let Some(history) = undo_history.clone() {
        let mounted = Rc::new(RefCell::new(None));
        let editor_for_cleanup = mounted.clone();
        let history_for_cleanup = history.clone();
        after_answer(move || {
            if let Some(api) = new_ref.api::<EditorApi>() {
                history.resume(&api);
                *mounted.borrow_mut() = Some(api);
            }
        });
        cleanup(move || {
            if let Some(api) = editor_for_cleanup.borrow_mut().take() {
                history_for_cleanup.park(&api);
            }
        });
    }
    let old_hover = super::diff_blame::Hover::new(is_commit, old_ref);
    let new_hover = super::diff_blame::Hover::new(is_commit, new_ref);
    after_answer(move || {
        if let (Some(old), Some(new)) = (old_ref.api::<EditorApi>(), new_ref.api::<EditorApi>()) {
            state::sync_diff_scroll(tab, old, new);
        }
    });
    let (init_old, init_new) = app_signal().with_untracked(|s| {
        if is_commit {
            s.commit_diff
                .as_ref()
                .map(|c| (c.data.old_text.clone(), c.data.new_text.clone()))
                .unwrap_or_default()
        } else {
            s.working_diff
                .as_ref()
                .map(|w| (w.data.old_text.clone(), w.current_new_text.clone()))
                .unwrap_or_default()
        }
    });

    let old_header_text = computed(move || {
        app_signal().with(|s| {
            if is_commit {
                s.commit_diff
                    .as_ref()
                    .map(|c| format!("{} · {}", c.data.old_title, c.data.old_path))
                    .unwrap_or_default()
            } else {
                s.working_diff
                    .as_ref()
                    .map(|w| {
                        let title = &w.data.old_title;
                        let path = &w.data.old_path;
                        if matches!(w.git_baseline_issue, Some(state::GitBaselineIssue::TextBlocked(_))) {
                            format!("上次有效版本 · {title} · {path}")
                        } else {
                            format!("{title} · {path}")
                        }
                    })
                    .unwrap_or_default()
            }
        })
    });
    let new_header_text = computed(move || {
        app_signal().with(|s| {
            if is_commit {
                s.commit_diff
                    .as_ref()
                    .map(|c| format!("{} · {}", c.data.new_title, c.data.new_path))
                    .unwrap_or_default()
            } else {
                s.working_diff
                    .as_ref()
                    .map(|w| format!("{} · {}", w.data.new_title, w.data.new_path))
                    .unwrap_or_default()
            }
        })
    });
    let old_lang_memo = computed(move || {
        app_signal().with(|s| {
            let path = if is_commit {
                s.commit_diff.as_ref().map(|c| c.data.old_path.clone())
            } else {
                s.working_diff.as_ref().map(|w| w.data.old_path.clone())
            };
            path.map(SharedString::from)
        })
    });
    let new_lang_memo = computed(move || {
        app_signal().with(|s| {
            let path = if is_commit {
                s.commit_diff.as_ref().map(|c| c.data.new_path.clone())
            } else {
                s.working_diff.as_ref().map(|w| w.data.new_path.clone())
            };
            path.map(SharedString::from)
        })
    });

    let old_decs = computed(move || {
        let pal = current_palette();
        app_signal().with(|s| {
            let inline_m = InlineHighlightMode::from_config(&s.cfg.diff_inline_mode);
            if is_commit {
                s.commit_diff
                    .as_ref()
                    .map(|c| {
                        build_side_by_side_decorations(
                            &c.data.old_text,
                            &c.data.new_text,
                            &c.computed.hunks,
                            inline_m,
                            pal,
                        )
                        .0
                    })
                    .unwrap_or_default()
            } else {
                s.working_diff
                    .as_ref()
                    .map(|w| {
                        build_side_by_side_decorations(
                            &w.data.old_text,
                            &w.current_new_text,
                            &w.computed.hunks,
                            inline_m,
                            pal,
                        )
                        .0
                    })
                    .unwrap_or_default()
            }
        })
    });

    let new_decs = computed(move || {
        let pal = current_palette();
        app_signal().with(|s| {
            let inline_m = InlineHighlightMode::from_config(&s.cfg.diff_inline_mode);
            if is_commit {
                s.commit_diff
                    .as_ref()
                    .map(|c| {
                        build_side_by_side_decorations(
                            &c.data.old_text,
                            &c.data.new_text,
                            &c.computed.hunks,
                            inline_m,
                            pal,
                        )
                        .1
                    })
                    .unwrap_or_default()
            } else {
                s.working_diff
                    .as_ref()
                    .map(|w| {
                        build_side_by_side_decorations(
                            &w.data.old_text,
                            &w.current_new_text,
                            &w.computed.hunks,
                            inline_m,
                            pal,
                        )
                        .1
                    })
                    .unwrap_or_default()
            }
        })
    });

    view()
        .flex_1()
        .flex_row()
        .overflow_hidden()
        // 左侧（旧版本，只读）
        .child(
            view()
                .flex_1()
                .flex_col()
                .overflow_hidden()
                .on_pinch(super::font_zoom::FontArea::Diff.pinch())
                .on_wheel(move |ev: &WheelEvent| {
                    remember_scroll(Some(DiffSide::Old));
                    old_hover.scroll(ev);
                })
                .border_r_1()
                .border_color(p.border)
                .child(
                    view()
                        .test_id("diff-old-pane-bar")
                        .h(px(24.0))
                        .px(px(10.0))
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .bg(p.surface2)
                        .border_b_1()
                        .border_color(p.border)
                        .cursor(Cursor::Pointer)
                        .on_press(move || {
                            state::focus_diff_side(tab, DiffSide::Old);
                            if let Some(api) = old_ref.api::<EditorApi>() {
                                api.focus();
                            }
                        })
                        .child(
                            text(move || old_header_text.get())
                                .test_id("diff-old-pane-header")
                                .text_size(px(10.5))
                                .text_color(p.text_secondary)
                                .number_of_lines(1u32),
                        )
                        .child(text("只读").text_size(px(9.5)).text_color(p.text_muted)),
                )
                .child(
                    editor()
                        .styles(super::review_editor_styles)
                        .component_ref(&old_ref)
                        .test_id(if is_commit {
                            "commit-diff-old-editor"
                        } else {
                            "working-diff-old-editor"
                        })
                        .flex_1()
                        .auto_focus(app_signal().with_untracked(|s| {
                            if is_commit {
                                s.focus_zone == FocusZone::CommitDiffOld
                            } else {
                                s.focus_zone == FocusZone::WorkingDiffOld
                            }
                        }))
                        .default_value(Some(SharedString::from(init_old)))
                        .language(move || old_lang_memo.get())
                        .read_only(true)
                        .soft_wrap(move || app_signal().with(|s| s.cfg.editor_soft_wrap))
                        .show_whitespaces(move || app_signal().with(|s| s.cfg.diff_show_whitespaces))
                        .font_size(move || px(super::font_zoom::FontArea::Diff.current()))
                        .decorations(move || old_decs.get())
                        .search(move || diff_search(tab, Some(DiffSide::Old)))
                        .on_focus(move || {
                            remember_scroll(None);
                            state::set_text_input_focused(false);
                            state::focus_diff_side(tab, DiffSide::Old);
                        })
                        .on_selection_change(move |sels: &Vec<EditorSelection>| {
                            if let Some(last) = sels.last() {
                                state::record_diff_caret(tab, DiffSide::Old, last.head);
                            }
                        })
                        .on_vertical_scroll_drag_start(move || remember_scroll(Some(DiffSide::Old)))
                        .on_search_change(move |res: &EditorSearchResult| record_diff_search(tab, Some(DiffSide::Old), res))
                        .on_hover(move |ev: &EditorHoverEvent| {
                            old_hover.show(is_commit, DiffSide::Old, ev.position.line as usize, ev);
                        })
                        .on_hover_end(move || old_hover.clear())
                        .on_context_menu(move |ev: &EditorContextMenuEvent| {
                            old_hover.clear();
                            show_diff_editor_context_menu(
                                tab,
                                DiffSide::Old,
                                ev.position,
                                ev,
                                None,
                            );
                        }),
                )
                .child(old_hover.tooltip()),
        )
        // 右侧（工作区未暂存可编辑；暂存区与历史提交只读）
        .child(
            view()
                .flex_1()
                .flex_col()
                .overflow_hidden()
                .on_pinch(super::font_zoom::FontArea::Diff.pinch())
                .on_wheel(move |ev: &WheelEvent| {
                    remember_scroll(Some(DiffSide::New));
                    new_hover.scroll(ev);
                })
                .child(
                    view()
                        .test_id("diff-new-pane-bar")
                        .h(px(24.0))
                        .px(px(10.0))
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .bg(p.surface2)
                        .border_b_1()
                        .border_color(p.border)
                        .cursor(Cursor::Pointer)
                        .on_press(move || {
                            state::focus_diff_side(tab, DiffSide::New);
                            if let Some(api) = new_ref.api::<EditorApi>() {
                                api.focus();
                            }
                        })
                        .child(
                            text(move || new_header_text.get())
                                .test_id("diff-new-pane-header")
                                .text_size(px(10.5))
                                .text_color(p.text_secondary)
                                .number_of_lines(1u32),
                        )
                        .child(when(
                            move || app_signal().with(|s| {
                                is_commit || s.working_diff.as_ref().is_none_or(|w| !w.editor_editable())
                            }),
                            move || text("只读").text_size(px(9.5)).text_color(p.text_muted),
                        )),
                )
                .child(
                    editor()
                        .styles(super::review_editor_styles)
                        .component_ref(&new_ref)
                        .test_id(if is_commit {
                            "commit-diff-new-editor"
                        } else {
                            "working-diff-new-editor"
                        })
                        .flex_1()
                        .auto_focus(app_signal().with_untracked(|s| {
                            if is_commit {
                                s.focus_zone == FocusZone::CommitDiffNew
                            } else {
                                s.focus_zone == FocusZone::WorkingDiffNew
                            }
                        }))
                        .default_value(Some(SharedString::from(init_new)))
                        .language(move || new_lang_memo.get())
                        .read_only(move || {
                            if is_commit {
                                true
                            } else {
                                app_signal().with(|s| {
                                    !s.working_diff
                                        .as_ref()
                                        .is_some_and(|w| w.editor_editable())
                                })
                            }
                        })
                        .soft_wrap(move || app_signal().with(|s| s.cfg.editor_soft_wrap))
                        .show_whitespaces(move || app_signal().with(|s| s.cfg.diff_show_whitespaces))
                        .font_size(move || px(super::font_zoom::FontArea::Diff.current()))
                        .decorations(move || new_decs.get())
                        .search(move || diff_search(tab, Some(DiffSide::New)))
                        .on_focus(move || {
                            remember_scroll(None);
                            state::set_text_input_focused(false);
                            state::focus_diff_side(tab, DiffSide::New);
                        })
                        .on_change(move |val: &SharedString| {
                            if is_commit {
                                return;
                            }
                            new_hover.invalidate();
                            let new_str = val.to_string();
                            if let Some(history) = &undo_history {
                                let mut refresh_repo = None;
                                app_signal().update(|s| refresh_repo = s.apply_working_diff_edit(history, new_str));
                                if let Some(repo) = refresh_repo {
                                    state::refresh_workspace_async(Some(HashSet::from([repo])));
                                }
                            }
                        })
                        .on_selection_change(move |sels: &Vec<EditorSelection>| {
                            if let Some(last) = sels.last() {
                                state::record_diff_caret(tab, DiffSide::New, last.head);
                            }
                        })
                        .on_vertical_scroll_drag_start(move || remember_scroll(Some(DiffSide::New)))
                        .on_search_change(move |res: &EditorSearchResult| record_diff_search(tab, Some(DiffSide::New), res))
                        .on_hover(move |ev: &EditorHoverEvent| {
                            new_hover.show(is_commit, DiffSide::New, ev.position.line as usize, ev);
                        })
                        .on_hover_end(move || new_hover.clear())
                        .on_context_menu(move |ev: &EditorContextMenuEvent| {
                            new_hover.clear();
                            show_diff_editor_context_menu(
                                tab,
                                DiffSide::New,
                                ev.position,
                                ev,
                                None,
                            );
                        }),
                )
                .child(new_hover.tooltip()),
        )
}

fn render_single_column_diff(
    is_commit: bool,
    unified_ref: ComponentRef<Editor>,
    p: Palette,
) -> impl IntoElement {
    let tab = if is_commit { EditorTabKind::CommitDiff } else { EditorTabKind::WorkingDiff };
    let (doc, lang, source) = app_signal().with_untracked(|s| {
        if is_commit {
            if let Some(c) = &s.commit_diff {
                (
                    build_unified_diff(&c.data.old_text, &c.data.new_text, &c.computed.hunks, p),
                    c.data.new_path.clone(),
                    None,
                )
            } else {
                (build_unified_diff("", "", &[], p), String::new(), None)
            }
        } else if let Some(w) = &s.working_diff {
            (
                working_unified_doc(w, p),
                w.data.new_path.clone(),
                w.editor_editable().then(|| (w.current_new_text.clone(), w.undo_history.clone())),
            )
        } else {
            (build_unified_diff("", "", &[], p), String::new(), None)
        }
    });
    let hover = super::diff_blame::Hover::new(is_commit, unified_ref);
    let initial = SharedString::from(doc.text.clone());
    let doc = signal(doc);
    let inline_mode = computed(|| app_signal().with(|s| {
        InlineHighlightMode::from_config(&s.cfg.diff_inline_mode)
    }));
    let projection_ready = signal(false);
    if let Some((source_text, history)) = source {
        let mounted = Rc::new(RefCell::new(None));
        let editor_for_cleanup = mounted.clone();
        let history_for_cleanup = history.clone();
        after_answer(move || {
            if let Some(api) = unified_ref.api::<EditorApi>() {
                let identity = history.clone();
                api.project_document(&source_text, doc.with_untracked(editor_projection), move |before, after| {
                    let mut next = None;
                    let mut refresh = None;
                    app_signal().update(|s| {
                        if s.editor_tab != state::EditorTabKind::WorkingDiff || !s.cfg.diff_single_column {
                            return;
                        }
                        if !s.working_diff.as_ref().is_some_and(|w| {
                            w.undo_history == identity && w.editor_editable() && w.current_new_text == before
                        }) {
                            return;
                        }
                        refresh = s.apply_working_diff_edit(&identity, after.to_owned());
                        next = s.working_diff.as_ref().map(|w| working_unified_doc(w, p));
                    });
                    let next = next?;
                    let view = editor_projection(&next);
                    hover.invalidate();
                    doc.set(next);
                    if let Some(repo) = refresh {
                        state::refresh_workspace_async(Some(HashSet::from([repo])));
                    }
                    Some(view)
                });
                history.resume(&api);
                *mounted.borrow_mut() = Some(api);
                projection_ready.set(true);
            }
        });
        cleanup(move || {
            if let Some(api) = editor_for_cleanup.borrow_mut().take() {
                history_for_cleanup.park(&api);
            }
        });
    }

    effect(move || {
        if app_signal().with(|s| s.cfg.diff_collapse_unchanged) {
            let folds = doc.with(|doc| doc.unchanged_folds.clone());
            after_answer(move || {
                if let Some(api) = unified_ref.api::<EditorApi>() {
                    api.set_folds(folds);
                }
            });
        }
    });

    let content = editor()
        .styles(super::review_editor_styles)
        .component_ref(&unified_ref)
        .test_id(if is_commit {
            "commit-diff-unified-editor"
        } else {
            "working-diff-unified-editor"
        })
        .flex_1()
        .auto_focus(app_signal().with_untracked(|s| {
            if is_commit {
                matches!(s.focus_zone, FocusZone::CommitDiffOld | FocusZone::CommitDiffNew)
            } else {
                matches!(s.focus_zone, FocusZone::WorkingDiffOld | FocusZone::WorkingDiffNew)
            }
        }))
        .default_value(Some(initial))
        .language(Some(SharedString::from(lang)))
        .read_only(move || !projection_ready.get() || !app_signal().with(|s| {
            s.working_diff.as_ref().is_some_and(|diff| diff.editor_editable())
        }))
        .soft_wrap(move || app_signal().with(|s| s.cfg.editor_soft_wrap))
        .show_whitespaces(move || app_signal().with(|s| s.cfg.diff_show_whitespaces))
        .font_size(move || px(super::font_zoom::FontArea::Diff.current()))
        .decorations(move || doc.with(|doc| doc.decorations_with_inline(inline_mode.get(), p)))
        .search(move || diff_search(tab, None))
        .on_search_change(move |res: &EditorSearchResult| record_diff_search(tab, None, res))
        .on_focus(move || {
            state::set_text_input_focused(false);
            let side = app_signal().with_untracked(|s| {
                if is_commit {
                    s.commit_diff.as_ref().map(|diff| diff.active_side)
                } else {
                    s.working_diff.as_ref().map(|diff| diff.active_side)
                }
            });
            if let Some(side) = side {
                state::focus_diff_side(tab, side);
            }
        })
        .on_selection_change(move |sels: &Vec<EditorSelection>| {
            if let Some(last) = sels.last()
                && let Some(row) = doc.with_untracked(|doc| doc.line_map.get(last.head.line as usize).copied())
            {
                state::record_diff_caret(tab, row.side, EditorPosition::new(
                    row.orig_line as u32, last.head.column.saturating_sub(2),
                ));
            }
        })
        .on_hover(move |ev: &EditorHoverEvent| {
            doc.with_untracked(|doc| hover.show_unified(is_commit, &doc.line_map, ev));
        })
        .on_hover_end(move || hover.clear())
        .on_context_menu(move |ev: &EditorContextMenuEvent| {
            hover.clear();
            doc.with_untracked(|doc| {
                if let Some(row) = doc.line_map.get(ev.position.line as usize) {
                    let position = EditorPosition::new(row.orig_line as u32, ev.position.column.saturating_sub(2));
                    show_diff_editor_context_menu(tab, row.side, position, ev, Some(&doc.line_map));
                }
            });
        });
    view()
        .flex_1()
        .overflow_hidden()
        .on_pinch(super::font_zoom::FontArea::Diff.pinch())
        .on_wheel(move |ev: &WheelEvent| hover.scroll(ev))
        .child(content)
        .child(hover.tooltip())
}

fn working_unified_doc(diff: &state::WorkingDiffSession, p: Palette) -> UnifiedDiffDoc {
    let source = &diff.current_new_text;
    let mut doc = build_unified_diff(&diff.data.old_text, source, &diff.computed.hunks, p);
    if diff.editor_editable()
        && let Some(eof) = unified_source_eof(source)
    {
        if !doc.line_map.is_empty() {
            doc.text.push('\n');
        }
        doc.text.push_str("  ");
        doc.line_map.push(eof);
    }
    doc
}

fn editor_projection(doc: &UnifiedDiffDoc) -> EditorProjection {
    EditorProjection {
        text: doc.text.clone(),
        source_lines: doc.line_map.iter().map(|line| {
            (line.side == DiffSide::New).then_some(line.orig_line as u32)
        }).collect(),
        prefix_columns: 2,
    }
}

fn show_diff_editor_context_menu(
    tab: EditorTabKind,
    side: DiffSide,
    position: EditorPosition,
    ev: &EditorContextMenuEvent,
    projection: Option<&[UnifiedLineRef]>,
) {
    let is_commit = tab == EditorTabKind::CommitDiff;
    let line_1based = position.line + 1;
    let info = app_signal().with_untracked(|s| {
        if s.editor_tab != tab {
            return None;
        }
        if is_commit {
            s.commit_diff.as_ref().map(|c| {
                let rel = match side {
                    DiffSide::Old => c.data.old_path.clone(),
                    DiffSide::New => c.data.new_path.clone(),
                };
                let abs = Path::new(&c.commit.repo_path)
                    .join(&rel)
                    .to_string_lossy()
                    .to_string();
                (c.commit.repo_id.clone(), c.commit.repo_path.clone(), rel, abs)
            })
        } else {
            s.working_diff.as_ref().map(|w| {
                let rel = match side {
                    DiffSide::Old => w.data.old_path.clone(),
                    DiffSide::New => w.data.new_path.clone(),
                };
                let abs = Path::new(&w.file.repo_path)
                    .join(&rel)
                    .to_string_lossy()
                    .to_string();
                (w.file.repo_id.clone(), w.file.repo_path.clone(), rel, abs)
            })
        }
    });
    let Some((repo_id, repo_path, rel_path, abs_path)) = info else {
        return;
    };
    let mut items = vec![
        MenuItem::new("复制").on_select(move || state::copy_diff_selection(tab, side)),
        MenuItem::new("跳转到源码 (F4)").on_select(move || state::jump_from_diff_line(tab, side, position)),
        MenuItem::separator(),
    ];
    if !is_commit {
        items.extend(super::git_review::line_items(side, position.line as usize, ev.position.line as usize, projection));
    }
    items.extend(build_location_menu_items(
        repo_id,
        repo_path,
        rel_path,
        abs_path,
        line_1based,
        false,
    ));
    crate::state::show_popup_menu_at(items, point(ev.x, ev.y));
}
