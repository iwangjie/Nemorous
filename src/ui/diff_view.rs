use std::path::Path;

use nsh_base::{
    Editor, EditorApi, EditorCommand, EditorContextMenuEvent, EditorSearch, EditorSearchResult,
    EditorSelection, editor, input,
};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::diff_model::{
    DiffSide, InlineHighlightMode, WhitespaceMode, build_side_by_side_decorations,
    build_unified_diff, compute_diff, compute_unchanged_fold_ranges, compute_unified_layout,
};
use crate::state::{
    self, FocusZone, app_signal, current_palette, editor_refs_signal, jump_diff_hunk,
    jump_from_diff_to_source, move_change_selection, move_commit_diff_file, open_find_bar,
    save_working_diff_with_format, set_inline_mode, set_whitespace_mode, switch_working_diff_scope,
};
use crate::theme::Palette;
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

fn toolbar_icon_btn(
    label: &'static str,
    svg_src: &'static str,
    active: bool,
    enabled: bool,
    p: Palette,
    tid: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    let fg = if !enabled {
        p.text_muted.opacity(0.45)
    } else if active {
        p.accent_soft_text
    } else {
        p.text_control
    };
    let bg = if active { p.accent_soft } else { transparent_black() };
    view()
        .test_id(tid)
        .h(px(24.0))
        .px(px(6.0))
        .gap(px(4.0))
        .flex_row()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(bg)
        .cursor(if enabled { Cursor::Pointer } else { Cursor::Default })
        .hover(move |s| if enabled { s.bg(p.surface_hover) } else { s })
        .label(label)
        .on_press(move || {
            if enabled {
                on_click();
            }
        })
        .child(icons::icon(svg_src, 12.0, fg))
}

fn toolbar_text_btn(
    label: String,
    active: bool,
    p: Palette,
    tid: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    let bg = if active { p.accent_soft } else { p.surface2 };
    let fg = if active { p.accent_soft_text } else { p.text_control };
    view()
        .test_id(tid)
        .h(px(24.0))
        .px(px(8.0))
        .flex_row()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(bg)
        .cursor(Cursor::Pointer)
        .hover(move |s| s.bg(p.surface_hover))
        .label(label.clone())
        .on_press(on_click)
        .child(text(label).text_size(px(11.0)).text_color(fg).number_of_lines(1u32))
}

/// 统一渲染工作区 Diff（`is_commit = false`）或历史提交 Diff（`is_commit = true`）。
pub fn diff_pane(is_commit: bool) -> impl IntoElement {
    let old_ref = component_ref::<Editor>();
    let new_ref = component_ref::<Editor>();
    let unified_ref = component_ref::<Editor>();
    let hunk_inclusion = computed(move || {
        if is_commit { None } else { app_signal().with(super::git_review::included_state) }
    });

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
                        api.set_value(new_text.clone());
                    }
                }
                if !state::text_input_focus_signal().with_untracked(|f| f.focused) {
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
            let (msg, rev_for_single) = if is_commit {
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
                        (m, if single { c.content_rev } else { 0 })
                    })
                    .unwrap_or((None, 0))
            } else {
                s.working_diff
                    .as_ref()
                    .map(|w| {
                        let m = if w.data.too_large {
                            Some("文件超过 2 MiB 审阅上限，已只读拦截，避免界面卡顿")
                        } else if w.data.binary {
                            Some("二进制或非 UTF-8 文件，无法以文本对比")
                        } else {
                            None
                        };
                        (m, if single { w.content_rev } else { 0 })
                    })
                    .unwrap_or((None, 0))
            };
            (single, msg, rev_for_single)
        })
    });

    view()
        .test_id(if is_commit { "commit-diff-pane" } else { "working-diff-pane" })
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .bg(move || current_palette().surface)
        // 顶栏 + 工具栏
        .child(dynamic(move || {
            let p = current_palette();
            let snap = app_signal().with(|s| {
                let ws_mode = WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode);
                let single = s.cfg.diff_single_column;
                let collapse = s.cfg.diff_collapse_unchanged;
                let show_opts = s.show_diff_options;
                if is_commit {
                    s.commit_diff.as_ref().map(|c| {
                        let path_label = if c.data.old_path != c.data.new_path {
                            format!("{} → {}", c.data.old_path, c.data.new_path)
                        } else {
                            c.data.new_path.clone()
                        };
                        (
                            c.commit.repo_name.clone(),
                            path_label,
                            format!("{} → {}", c.data.old_title, c.data.new_title),
                            c.computed.added_lines,
                            c.computed.deleted_lines,
                            c.computed.hunks.len(),
                            c.current_hunk,
                            false, // dirty
                            false, // staged_only
                            false, // editable
                            c.data.status != "D",
                            ws_mode,
                            single,
                            collapse,
                            show_opts,
                            c.boundary_hint,
                        )
                    })
                } else {
                    s.working_diff.as_ref().map(|w| {
                        let path_label = if w.data.old_path != w.data.new_path {
                            format!("{} → {}", w.data.old_path, w.data.new_path)
                        } else {
                            w.data.new_path.clone()
                        };
                        (
                            w.file.repo_name.clone(),
                            path_label,
                            format!("{} → {}", w.data.old_title, w.data.new_title),
                            w.computed.added_lines,
                            w.computed.deleted_lines,
                            w.computed.hunks.len(),
                            w.current_hunk,
                            w.is_dirty(),
                            w.data.staged_only,
                            w.data.editable && !w.data.staged_only,
                            Path::new(&w.file.abs_path()).exists(),
                            ws_mode,
                            single,
                            collapse,
                            show_opts,
                            w.boundary_hint,
                        )
                    })
                }
            });

            let Some((
                repo_name,
                path_label,
                rev_label,
                added,
                deleted,
                hunk_count,
                cur_hunk,
                is_dirty,
                staged_only,
                editable,
                can_open_source,
                ws_mode,
                single,
                collapse,
                show_opts,
                boundary_hint,
            )) = snap
            else {
                return view().h(px(0.0)).into_element();
            };

            let hunk_counter = if hunk_count == 0 {
                "0 处差异".to_string()
            } else {
                format!("{}/{}", (cur_hunk + 1).min(hunk_count), hunk_count)
            };

            view()
                .flex_col()
                .w_full()
                .bg(p.rail_bg)
                .border_b_1()
                .border_color(p.border)
                // 行 1：仓库、路径、状态、Scope 切换
                .child(
                    view()
                        .h(px(36.0))
                        .px(px(12.0))
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .border_b_1()
                        .border_color(p.border)
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .overflow_hidden()
                                .child(
                                    view()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(p.accent_soft)
                                        .child(
                                            text(repo_name)
                                                .text_size(px(10.5))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(p.accent_soft_text),
                                        ),
                                )
                                .child(
                                    text(path_label)
                                        .test_id("diff-header-path")
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_primary)
                                        .number_of_lines(1u32),
                                )
                                .child(
                                    text(rev_label)
                                        .text_size(px(10.5))
                                        .text_color(p.text_muted)
                                        .number_of_lines(1u32),
                                )
                                .child(
                                    text(format!("+{added}"))
                                        .test_id("diff-stat-added")
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(p.green),
                                )
                                .child(
                                    text(format!("-{deleted}"))
                                        .test_id("diff-stat-deleted")
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(p.danger),
                                )
                                .child((!is_commit).then(|| toolbar_text_btn(
                                    "三栏".to_owned(), false, p,
                                    "diff-index-review", super::index_review::open_active,
                                )))
                                .child((!is_commit).then(|| dynamic(move || {
                                    let repo = app_signal().with(|s| {
                                        s.working_diff.as_ref().filter(|w| w.file.conflict).map(|w| w.file.repo_path.clone())
                                    });
                                    repo.map(|repo| toolbar_text_btn(
                                        "解决冲突".to_owned(), true, p, "diff-open-conflicts",
                                        move || super::conflict_workbench::open(super::conflict_workbench::Source::Repo(repo.clone().into())),
                                    ).into_element()).unwrap_or_else(|| view().into_element())
                                })))
                                .child(is_dirty.then(|| {
                                    view()
                                        .test_id("diff-dirty-badge")
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(p.amber_soft)
                                        .child(
                                            text("未保存")
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(p.amber),
                                        )
                                }))
                                .child((!editable).then(|| {
                                    view()
                                        .px(px(5.0))
                                        .py(px(1.5))
                                        .rounded(px(4.0))
                                        .bg(p.surface2)
                                        .child(text("只读").text_size(px(10.0)).text_color(p.text_muted))
                                })),
                        )
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child((!is_commit).then(|| {
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(3.0))
                                        .child(toolbar_text_btn(
                                            "未暂存".to_string(),
                                            !staged_only,
                                            p,
                                            "diff-scope-unstaged",
                                            || switch_working_diff_scope(false),
                                        ))
                                        .child(toolbar_text_btn(
                                            "暂存区".to_string(),
                                            staged_only,
                                            p,
                                            "diff-scope-staged",
                                            || switch_working_diff_scope(true),
                                        ))
                                }))
                                .child(is_dirty.then(|| {
                                    view()
                                        .test_id("diff-save-btn")
                                        .h(px(24.0))
                                        .px(px(10.0))
                                        .rounded(px(5.0))
                                        .bg(p.accent)
                                        .items_center()
                                        .justify_center()
                                        .cursor(Cursor::Pointer)
                                        .hover(move |s| s.bg(p.accent_hover))
                                        .on_press(|| {
                                            save_working_diff_with_format();
                                        })
                                        .child(
                                            text("保存 ⌘S")
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(p.accent_text),
                                        )
                                })),
                        ),
                )
                // 行 2：Diff 工具栏（上下差异、计数、修改 F4、折叠、查找、忽略空白、上下文件、双栏/单栏、显示选项）
                .child(
                    view()
                        .test_id("diff-toolbar")
                        .h(px(32.0))
                        .px(px(10.0))
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(toolbar_icon_btn(
                            "上一处差异 (⇧F7)",
                            ICO_CHEVRON_UP,
                            false,
                            hunk_count > 0,
                            p,
                            "diff-prev-hunk",
                            || jump_diff_hunk(-1),
                        ))
                        .child(
                            text(hunk_counter)
                                .test_id("diff-hunk-counter")
                                .text_size(px(11.0))
                                .text_color(p.text_secondary),
                        )
                        .child(toolbar_icon_btn(
                            "下一处差异 (F7)",
                            ICO_CHEVRON_DOWN,
                            false,
                            hunk_count > 0,
                            p,
                            "diff-next-hunk",
                            || jump_diff_hunk(1),
                        ))
                        .child((!is_commit && hunk_count > 0).then(|| {
                            toolbar_text_btn(
                                if staged_only { "取消暂存块" } else { "暂存块" }.to_owned(),
                                false, p, "diff-stage-hunk", super::git_review::stage_hunk,
                            )
                        }))
                        .child((!is_commit && hunk_count > 0).then(|| {
                            toolbar_text_btn("行操作…".to_owned(), false, p, "diff-lines-menu",
                                super::git_review::open_line_menu)
                        }))
                        .child(when(move || hunk_inclusion.get().is_some(), move || {
                            view()
                                .test_id("diff-include-hunk")
                                .h(px(24.0)).px(px(8.0))
                                .flex_row().items_center().justify_center().rounded(px(4.0))
                                .bg(move || if hunk_inclusion.get() == Some(true) { p.accent_soft } else { p.surface2 })
                                .cursor(Cursor::Pointer)
                                .hover(move |s| s.bg(p.surface_hover))
                                .label("纳入提交")
                                .on_press(super::git_review::toggle_included_hunk)
                                .child(text(move || {
                                    if hunk_inclusion.get() == Some(true) { "✓ 纳入提交" } else { "纳入提交" }
                                }).text_size(px(11.0)).text_color(p.text_primary).number_of_lines(1u32))
                        }))
                        .child(
                            toolbar_text_btn(
                                "修改 (F4)".to_string(),
                                false,
                                p,
                                "diff-jump-source-btn",
                                move || {
                                    if can_open_source {
                                        jump_from_diff_to_source();
                                    }
                                },
                            ),
                        )
                        .child(toolbar_icon_btn(
                            if collapse { "展开未改动" } else { "折叠未改动" },
                            ICO_COLLAPSE,
                            collapse,
                            true,
                            p,
                            "diff-collapse-btn",
                            || {
                                app_signal().update(|s| {
                                    s.cfg.diff_collapse_unchanged = !s.cfg.diff_collapse_unchanged;
                                });
                                state::save_config_quiet();
                            },
                        ))
                        .child(toolbar_icon_btn(
                            "查找 (⌘F)",
                            ICO_SEARCH,
                            false,
                            true,
                            p,
                            "diff-find-btn",
                            || open_find_bar(false),
                        ))
                        .child(toolbar_text_btn(
                            ws_mode.label().to_string(),
                            ws_mode != WhitespaceMode::None,
                            p,
                            "diff-whitespace-btn",
                            move || {
                                crate::state::show_popup_menu(vec![
                                    MenuItem::new("不忽略空白")
                                        .checked(ws_mode == WhitespaceMode::None)
                                        .on_select(|| set_whitespace_mode(WhitespaceMode::None)),
                                    MenuItem::new("忽略首尾空白")
                                        .checked(ws_mode == WhitespaceMode::Trim)
                                        .on_select(|| set_whitespace_mode(WhitespaceMode::Trim)),
                                    MenuItem::new("忽略空白")
                                        .checked(ws_mode == WhitespaceMode::All)
                                        .on_select(|| set_whitespace_mode(WhitespaceMode::All)),
                                    MenuItem::new("忽略空白和空行")
                                        .checked(ws_mode == WhitespaceMode::AllAndEmptyLines)
                                        .on_select(|| {
                                            set_whitespace_mode(WhitespaceMode::AllAndEmptyLines)
                                        }),
                                ]);
                            },
                        ))
                        .child(toolbar_text_btn(
                            if single { "单栏" } else { "双栏" }.to_string(),
                            single,
                            p,
                            "diff-layout-toggle",
                            || {
                                app_signal().update(|s| {
                                    s.cfg.diff_single_column = !s.cfg.diff_single_column;
                                });
                                state::save_config_quiet();
                            },
                        ))
                        .child(toolbar_icon_btn(
                            "显示选项",
                            ICO_MORE,
                            show_opts,
                            true,
                            p,
                            "diff-options-btn",
                            || {
                                app_signal().update(|s| s.show_diff_options = !s.show_diff_options);
                            },
                        ))
                        .child(view().flex_1())
                        .child(boundary_hint.map(|dir| {
                            text(if dir > 0 {
                                "再按 F7 切到下一文件"
                            } else {
                                "再按 ⇧F7 切到上一文件"
                            })
                            .text_size(px(10.5))
                            .text_color(p.amber)
                        }))
                        .child(toolbar_icon_btn(
                            "上一个文件 (⌥←)",
                            ICO_ARROW_LEFT,
                            false,
                            true,
                            p,
                            "diff-prev-file",
                            move || {
                                if is_commit {
                                    move_commit_diff_file(-1);
                                } else {
                                    move_change_selection(-1, true);
                                }
                            },
                        ))
                        .child(toolbar_icon_btn(
                            "下一个文件 (⌥→)",
                            ICO_ARROW_RIGHT,
                            false,
                            true,
                            p,
                            "diff-next-file",
                            move || {
                                if is_commit {
                                    move_commit_diff_file(1);
                                } else {
                                    move_change_selection(1, true);
                                }
                            },
                        )),
                )
                .into_element()
        }))
        // 显示选项折叠栏
        .child(when(
            move || app_signal().with(|s| s.show_diff_options),
            move || {
                dynamic(move || {
                    let p = current_palette();
                    let (wrap, show_ws, inline_m, font_sz) = app_signal().with(|s| {
                        (
                            s.cfg.editor_soft_wrap,
                            s.cfg.diff_show_whitespaces,
                            InlineHighlightMode::from_config(&s.cfg.diff_inline_mode),
                            s.cfg.editor_font_size,
                        )
                    });
                    view()
                        .h(px(32.0))
                        .px(px(12.0))
                        .gap(px(6.0))
                        .flex_row()
                        .items_center()
                        .bg(p.rail_bg)
                        .border_b_1()
                        .border_color(p.border)
                        .child(text("显示选项：").text_size(px(11.0)).text_color(p.text_muted))
                        .child(toolbar_text_btn(
                            if wrap { "自动换行：开" } else { "自动换行：关" }.to_string(),
                            wrap,
                            p,
                            "diff-opt-wrap",
                            || {
                                app_signal().update(|s| {
                                    s.cfg.editor_soft_wrap = !s.cfg.editor_soft_wrap;
                                });
                                state::save_config_quiet();
                            },
                        ))
                        .child(toolbar_text_btn(
                            if show_ws {
                                "空白字符：显"
                            } else {
                                "空白字符：隐"
                            }
                            .to_string(),
                            show_ws,
                            p,
                            "diff-opt-show-ws",
                            || {
                                app_signal().update(|s| {
                                    s.cfg.diff_show_whitespaces = !s.cfg.diff_show_whitespaces;
                                });
                                state::save_config_quiet();
                            },
                        ))
                        .child(toolbar_text_btn(
                            format!("行内高亮：{}", inline_m.label()),
                            inline_m != InlineHighlightMode::None,
                            p,
                            "diff-opt-inline",
                            move || {
                                let next = match inline_m {
                                    InlineHighlightMode::Word => InlineHighlightMode::Char,
                                    InlineHighlightMode::Char => InlineHighlightMode::None,
                                    InlineHighlightMode::None => InlineHighlightMode::Word,
                                };
                                set_inline_mode(next);
                            },
                        ))
                        .child(toolbar_text_btn(
                            format!("字号：{:.0}pt", font_sz),
                            false,
                            p,
                            "diff-opt-fontsize",
                            || {
                                app_signal().update(|s| {
                                    s.cfg.editor_font_size += 1.0;
                                    if s.cfg.editor_font_size > 17.0 {
                                        s.cfg.editor_font_size = 12.0;
                                    }
                                });
                                state::save_config_quiet();
                            },
                        ))
                })
            },
        ))
        // 按聚焦侧查找条（保持输入框实例稳定，仅响应 side / matches 文字更新！）
        .child(when(
            move || app_signal().with(|s| s.diff_find_open),
            move || {
                let p = current_palette();
                let run_find = move |next: bool| {
                    let (side, single) =
                        app_signal().with_untracked(|s| (s.diff_find_side, s.cfg.diff_single_column));
                    let target_ref = if single {
                        unified_ref
                    } else {
                        match side {
                            DiffSide::Old => old_ref,
                            DiffSide::New => new_ref,
                        }
                    };
                    if let Some(api) = target_ref.api::<EditorApi>() {
                        api.run(if next {
                            EditorCommand::FindNext
                        } else {
                            EditorCommand::FindPrevious
                        });
                    }
                };
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
                                    let (single, side) = app_signal()
                                        .with(|s| (s.cfg.diff_single_column, s.diff_find_side));
                                    if single {
                                        "查找单栏"
                                    } else {
                                        match side {
                                            DiffSide::Old => "查找左侧",
                                            DiffSide::New => "查找右侧",
                                        }
                                    }
                                })
                                .test_id("diff-find-side-badge")
                                .text_size(px(10.5))
                                .text_color(move || current_palette().accent_soft_text),
                            ),
                    )
                    .child(
                        input()
                            .test_id("diff-find-input")
                            .w(px(220.0))
                            .h(px(24.0))
                            .px(px(8.0))
                            .rounded(px(4.0))
                            .bg(move || current_palette().surface)
                            .border_1()
                            .border_color(move || current_palette().border)
                            .text_size(px(11.5))
                            .text_color(move || current_palette().text_primary)
                            .placeholder("在当前聚焦侧查找…")
                            .value(move || app_signal().with(|s| s.diff_find_query.clone()))
                            .on_focus(|| state::set_text_input_focused(true))
                            .on_blur(|| state::set_text_input_focused(false))
                            .on_change(|v: &str| {
                                app_signal().update(|s| s.diff_find_query = v.to_string());
                            })
                            .on_submit(move |_: &str| run_find(true)),
                    )
                    .child(
                        text(move || {
                            let matches = app_signal().with(|s| s.diff_find_matches);
                            if matches.0 == 0 {
                                "0 个结果".to_string()
                            } else if let Some(cur) = matches.1 {
                                format!("{}/{}", cur + 1, matches.0)
                            } else {
                                format!("{} 个结果", matches.0)
                            }
                        })
                        .test_id("diff-find-matches")
                        .text_size(px(11.0))
                        .text_color(move || current_palette().text_muted),
                    )
                    .child(toolbar_text_btn(
                        "上一个".to_string(),
                        false,
                        p,
                        "diff-find-prev",
                        move || run_find(false),
                    ))
                    .child(toolbar_text_btn(
                        "下一个".to_string(),
                        false,
                        p,
                        "diff-find-next",
                        move || run_find(true),
                    ))
                    .child(view().flex_1())
                    .child(
                        view()
                            .w(px(20.0))
                            .h(px(20.0))
                            .items_center()
                            .justify_center()
                            .cursor(Cursor::Pointer)
                            .on_press(|| {
                                state::set_text_input_focused(false);
                                app_signal().update(|s| s.diff_find_open = false);
                            })
                            .child(icons::icon(ICO_CLOSE, 10.0, p.text_muted)),
                    )
            },
        ))
        // 主体内容：仅在双栏/单栏切换或拦截态变化时重建，保持双 Editor 实例稳定不循环重建！
        .child(dynamic(move || {
            let p = current_palette();
            let (single, blocked_msg, _single_rev) = body_mode_memo.get();

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
                render_side_by_side_diff(is_commit, old_ref, new_ref, p).into_element()
            }
        }))
}

fn render_side_by_side_diff(
    is_commit: bool,
    old_ref: ComponentRef<Editor>,
    new_ref: ComponentRef<Editor>,
    p: Palette,
) -> impl IntoElement {
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
                    .map(|w| format!("{} · {}", w.data.old_title, w.data.old_path))
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
                            app_signal().update(|s| {
                                if is_commit {
                                    if let Some(c) = &mut s.commit_diff {
                                        c.active_side = DiffSide::Old;
                                    }
                                    s.focus_zone = FocusZone::CommitDiffOld;
                                } else {
                                    if let Some(w) = &mut s.working_diff {
                                        w.active_side = DiffSide::Old;
                                    }
                                    s.focus_zone = FocusZone::WorkingDiffOld;
                                }
                                if s.diff_find_open {
                                    s.diff_find_side = DiffSide::Old;
                                }
                            });
                            if let Some(api) = old_ref.api::<EditorApi>() {
                                api.focus();
                            }
                            state::request_shortcut_rebind();
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
                        .font_size(move || px(app_signal().with(|s| s.cfg.editor_font_size)))
                        .decorations(move || old_decs.get())
                        .search(move || {
                            app_signal().with(|s| {
                                if s.diff_find_open
                                    && s.diff_find_side == DiffSide::Old
                                    && !s.diff_find_query.is_empty()
                                {
                                    Some(EditorSearch::new(s.diff_find_query.clone()))
                                } else {
                                    None
                                }
                            })
                        })
                        .on_focus(move || {
                            state::set_text_input_focused(false);
                            app_signal().update(|s| {
                                if is_commit {
                                    if let Some(c) = &mut s.commit_diff {
                                        c.active_side = DiffSide::Old;
                                    }
                                    s.focus_zone = FocusZone::CommitDiffOld;
                                } else {
                                    if let Some(w) = &mut s.working_diff {
                                        w.active_side = DiffSide::Old;
                                    }
                                    s.focus_zone = FocusZone::WorkingDiffOld;
                                }
                                if s.diff_find_open {
                                    s.diff_find_side = DiffSide::Old;
                                }
                            });
                            state::request_shortcut_rebind();
                        })
                        .on_selection_change(move |sels: &Vec<EditorSelection>| {
                            if let Some(last) = sels.last() {
                                let head = last.head;
                                app_signal().update(|s| {
                                    if is_commit {
                                        if let Some(c) = &mut s.commit_diff {
                                            c.old_cursor = head;
                                        }
                                    } else if let Some(w) = &mut s.working_diff {
                                        w.old_cursor = head;
                                    }
                                });
                            }
                        })
                        .on_search_change(move |res: &EditorSearchResult| {
                            let next = (res.count, res.current);
                            let should = app_signal().with_untracked(|s| {
                                s.diff_find_side == DiffSide::Old && s.diff_find_matches != next
                            });
                            if should {
                                app_signal().update(|s| {
                                    s.diff_find_matches = next;
                                });
                            }
                        })
                        .on_context_menu(move |ev: &EditorContextMenuEvent| {
                            show_diff_editor_context_menu(
                                is_commit,
                                DiffSide::Old,
                                ev.position.line + 1,
                                ev,
                            );
                        }),
                ),
        )
        // 右侧（工作区未暂存可编辑；暂存区与历史提交只读）
        .child(
            view()
                .flex_1()
                .flex_col()
                .overflow_hidden()
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
                            app_signal().update(|s| {
                                if is_commit {
                                    if let Some(c) = &mut s.commit_diff {
                                        c.active_side = DiffSide::New;
                                    }
                                    s.focus_zone = FocusZone::CommitDiffNew;
                                } else {
                                    if let Some(w) = &mut s.working_diff {
                                        w.active_side = DiffSide::New;
                                    }
                                    s.focus_zone = FocusZone::WorkingDiffNew;
                                }
                                if s.diff_find_open {
                                    s.diff_find_side = DiffSide::New;
                                }
                            });
                            if let Some(api) = new_ref.api::<EditorApi>() {
                                api.focus();
                            }
                            state::request_shortcut_rebind();
                        })
                        .child(
                            text(move || new_header_text.get())
                                .test_id("diff-new-pane-header")
                                .text_size(px(10.5))
                                .text_color(p.text_secondary)
                                .number_of_lines(1u32),
                        )
                        .child(
                            text(move || {
                                let ed = app_signal().with(|s| {
                                    !is_commit
                                        && s.working_diff
                                            .as_ref()
                                            .is_some_and(|w| w.data.editable && !w.data.staged_only)
                                });
                                if ed { "可轻编辑" } else { "只读" }
                            })
                            .text_size(px(9.5))
                            .text_color(p.text_muted),
                        ),
                )
                .child(
                    editor()
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
                                        .is_some_and(|w| w.data.editable && !w.data.staged_only)
                                })
                            }
                        })
                        .soft_wrap(move || app_signal().with(|s| s.cfg.editor_soft_wrap))
                        .show_whitespaces(move || app_signal().with(|s| s.cfg.diff_show_whitespaces))
                        .font_size(move || px(app_signal().with(|s| s.cfg.editor_font_size)))
                        .decorations(move || new_decs.get())
                        .search(move || {
                            app_signal().with(|s| {
                                if s.diff_find_open
                                    && s.diff_find_side == DiffSide::New
                                    && !s.diff_find_query.is_empty()
                                {
                                    Some(EditorSearch::new(s.diff_find_query.clone()))
                                } else {
                                    None
                                }
                            })
                        })
                        .on_focus(move || {
                            state::set_text_input_focused(false);
                            app_signal().update(|s| {
                                if is_commit {
                                    if let Some(c) = &mut s.commit_diff {
                                        c.active_side = DiffSide::New;
                                    }
                                    s.focus_zone = FocusZone::CommitDiffNew;
                                } else {
                                    if let Some(w) = &mut s.working_diff {
                                        w.active_side = DiffSide::New;
                                    }
                                    s.focus_zone = FocusZone::WorkingDiffNew;
                                }
                                if s.diff_find_open {
                                    s.diff_find_side = DiffSide::New;
                                }
                            });
                            state::request_shortcut_rebind();
                        })
                        .on_change(move |val: &SharedString| {
                            if is_commit {
                                return;
                            }
                            let new_str = val.to_string();
                            app_signal().update(|s| {
                                let ws_mode = WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode);
                                if let Some(w) = &mut s.working_diff {
                                    if w.data.editable && !w.data.staged_only {
                                        w.current_new_text = new_str;
                                        w.computed = compute_diff(
                                            &w.data.old_text,
                                            &w.current_new_text,
                                            ws_mode,
                                        );
                                    }
                                }
                            });
                        })
                        .on_selection_change(move |sels: &Vec<EditorSelection>| {
                            if let Some(last) = sels.last() {
                                let head = last.head;
                                app_signal().update(|s| {
                                    if is_commit {
                                        if let Some(c) = &mut s.commit_diff {
                                            c.new_cursor = head;
                                        }
                                    } else if let Some(w) = &mut s.working_diff {
                                        w.new_cursor = head;
                                    }
                                });
                            }
                        })
                        .on_search_change(move |res: &EditorSearchResult| {
                            let next = (res.count, res.current);
                            let should = app_signal().with_untracked(|s| {
                                s.diff_find_side == DiffSide::New && s.diff_find_matches != next
                            });
                            if should {
                                app_signal().update(|s| {
                                    s.diff_find_matches = next;
                                });
                            }
                        })
                        .on_context_menu(move |ev: &EditorContextMenuEvent| {
                            show_diff_editor_context_menu(
                                is_commit,
                                DiffSide::New,
                                ev.position.line + 1,
                                ev,
                            );
                        }),
                ),
        )
}

fn render_single_column_diff(
    is_commit: bool,
    unified_ref: ComponentRef<Editor>,
    p: Palette,
) -> impl IntoElement {
    let (doc, lang) = app_signal().with_untracked(|s| {
        if is_commit {
            if let Some(c) = &s.commit_diff {
                (
                    build_unified_diff(&c.data.old_text, &c.data.new_text, &c.computed.hunks, p),
                    c.data.new_path.clone(),
                )
            } else {
                (build_unified_diff("", "", &[], p), String::new())
            }
        } else if let Some(w) = &s.working_diff {
            (
                build_unified_diff(&w.data.old_text, &w.current_new_text, &w.computed.hunks, p),
                w.data.new_path.clone(),
            )
        } else {
            (build_unified_diff("", "", &[], p), String::new())
        }
    });
    let line_map = doc.line_map.clone();
    let line_map_cm = doc.line_map.clone();
    let decs = doc.decorations.clone();
    let folds = doc.unchanged_folds.clone();

    let collapse = app_signal().with_untracked(|s| s.cfg.diff_collapse_unchanged);
    {
        let f_clone = if collapse { folds.clone() } else { Vec::new() };
        set_timeout(
            std::time::Duration::from_millis(16),
            move || {
                if let Some(api) = unified_ref.api::<EditorApi>() {
                    api.set_folds(f_clone);
                }
            },
        );
    }

    editor()
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
        .default_value(Some(SharedString::from(doc.text)))
        .language(Some(SharedString::from(lang)))
        .read_only(true)
        .soft_wrap(move || app_signal().with(|s| s.cfg.editor_soft_wrap))
        .show_whitespaces(move || app_signal().with(|s| s.cfg.diff_show_whitespaces))
        .font_size(move || px(app_signal().with(|s| s.cfg.editor_font_size)))
        .decorations(decs)
        .search(move || {
            app_signal().with(|s| {
                if s.diff_find_open && !s.diff_find_query.is_empty() {
                    Some(EditorSearch::new(s.diff_find_query.clone()))
                } else {
                    None
                }
            })
        })
        .on_search_change(move |res: &EditorSearchResult| {
            let next = (res.count, res.current);
            if app_signal().with_untracked(|s| s.diff_find_matches != next) {
                app_signal().update(|s| {
                    s.diff_find_matches = next;
                });
            }
        })
        .on_focus(move || {
            state::set_text_input_focused(false);
            app_signal().update(|s| {
                let side = if is_commit {
                    s.commit_diff
                        .as_ref()
                        .map(|c| c.active_side)
                        .unwrap_or(DiffSide::New)
                } else {
                    s.working_diff
                        .as_ref()
                        .map(|w| w.active_side)
                        .unwrap_or(DiffSide::New)
                };
                s.focus_zone = match (is_commit, side) {
                    (true, DiffSide::Old) => FocusZone::CommitDiffOld,
                    (true, DiffSide::New) => FocusZone::CommitDiffNew,
                    (false, DiffSide::Old) => FocusZone::WorkingDiffOld,
                    (false, DiffSide::New) => FocusZone::WorkingDiffNew,
                };
            });
            state::request_shortcut_rebind();
        })
        .on_selection_change(move |sels: &Vec<EditorSelection>| {
            if let Some(last) = sels.last() {
                if let Some(mapped) = line_map.get(last.head.line as usize) {
                    let col = last.head.column.saturating_sub(2);
                    let pos = nsh_base::EditorPosition::new(mapped.orig_line as u32, col);
                    app_signal().update(|s| {
                        if is_commit {
                            if let Some(c) = &mut s.commit_diff {
                                c.active_side = mapped.side;
                                match mapped.side {
                                    DiffSide::Old => c.old_cursor = pos,
                                    DiffSide::New => c.new_cursor = pos,
                                }
                            }
                            s.focus_zone = match mapped.side {
                                DiffSide::Old => FocusZone::CommitDiffOld,
                                DiffSide::New => FocusZone::CommitDiffNew,
                            };
                        } else {
                            if let Some(w) = &mut s.working_diff {
                                w.active_side = mapped.side;
                                match mapped.side {
                                    DiffSide::Old => w.old_cursor = pos,
                                    DiffSide::New => w.new_cursor = pos,
                                }
                            }
                            s.focus_zone = match mapped.side {
                                DiffSide::Old => FocusZone::WorkingDiffOld,
                                DiffSide::New => FocusZone::WorkingDiffNew,
                            };
                        }
                    });
                }
            }
        })
        .on_context_menu(move |ev: &EditorContextMenuEvent| {
            let (side, line_1based) = if let Some(mapped) = line_map_cm.get(ev.position.line as usize) {
                let col = ev.position.column.saturating_sub(2);
                let pos = nsh_base::EditorPosition::new(mapped.orig_line as u32, col);
                app_signal().update(|s| {
                    if is_commit {
                        if let Some(c) = &mut s.commit_diff {
                            c.active_side = mapped.side;
                            match mapped.side {
                                DiffSide::Old => c.old_cursor = pos,
                                DiffSide::New => c.new_cursor = pos,
                            }
                        }
                        s.focus_zone = match mapped.side {
                            DiffSide::Old => FocusZone::CommitDiffOld,
                            DiffSide::New => FocusZone::CommitDiffNew,
                        };
                    } else {
                        if let Some(w) = &mut s.working_diff {
                            w.active_side = mapped.side;
                            match mapped.side {
                                DiffSide::Old => w.old_cursor = pos,
                                DiffSide::New => w.new_cursor = pos,
                            }
                        }
                        s.focus_zone = match mapped.side {
                            DiffSide::Old => FocusZone::WorkingDiffOld,
                            DiffSide::New => FocusZone::WorkingDiffNew,
                        };
                    }
                });
                (mapped.side, mapped.orig_line as u32 + 1)
            } else {
                let fallback_side = app_signal().with_untracked(|s| {
                    if is_commit {
                        s.commit_diff
                            .as_ref()
                            .map(|c| c.active_side)
                            .unwrap_or(DiffSide::New)
                    } else {
                        s.working_diff
                            .as_ref()
                            .map(|w| w.active_side)
                            .unwrap_or(DiffSide::New)
                    }
                });
                (fallback_side, ev.position.line + 1)
            };
            show_diff_editor_context_menu(is_commit, side, line_1based, ev);
        })
}

fn show_diff_editor_context_menu(
    is_commit: bool,
    side: DiffSide,
    line_1based: u32,
    ev: &EditorContextMenuEvent,
) {
    let info = app_signal().with_untracked(|s| {
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
        MenuItem::new("复制").on_select(state::copy_for_current_focus),
        MenuItem::new("跳转到源码 (F4)").on_select(jump_from_diff_to_source),
        MenuItem::separator(),
    ];
    if !is_commit {
        let staged = app_signal().with_untracked(|s| s.working_diff.as_ref().is_some_and(|w| w.data.staged_only));
        items.push(super::git_review::line_item(side, staged));
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
