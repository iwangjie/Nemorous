use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::paths::basename;
use crate::state::{
    self, EditorTabKind, FocusZone, PickerKind, app_signal, close_commit_diff_now, current_palette,
    request_close_all_code_tabs, request_close_code_tab, request_close_other_code_tabs,
    request_close_working_diff,
};
use crate::theme::{repo_badge_colors, status_text_color};
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

pub fn editor_tab_bar() -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let snap = app_signal().with(|s| {
            (
                s.code_tabs.clone(),
                s.active_code_tab_idx,
                s.editor_tab,
                s.working_diff.as_ref().map(|w| {
                    (
                        w.file.clone(),
                        w.is_dirty(),
                        w.new_cursor.line + 1,
                    )
                }),
                s.commit_diff.as_ref().map(|c| {
                    (
                        c.commit.clone(),
                        c.data.new_path.clone(),
                        c.new_cursor.line + 1,
                    )
                }),
            )
        });
        let (code_tabs, active_idx, editor_tab, work_diff, commit_diff) = snap;
        if code_tabs.is_empty() && work_diff.is_none() && commit_diff.is_none() {
            return view().h(px(0.0)).into_element();
        }

        let mut tab_els: Vec<AnyElement> = Vec::new();
        for (idx, tab) in code_tabs.iter().enumerate() {
            let active = editor_tab == EditorTabKind::File && idx == active_idx;
            let dirty = tab.is_dirty();
            let badge = repo_badge_colors(tab.file.color_idx, p.is_dark);
            let title = tab.title();
            let tid_close = tab.id.clone();
            let tid_cm = tab.id.clone();
            let f_cm = tab.file.clone();
            let line_cm = tab.cursor.line + 1;

            let title_el = text(title)
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if active { p.text_primary } else { p.text_secondary })
                .number_of_lines(1u32);
            let title_el = if tab.preview { title_el.italic() } else { title_el };
            tab_els.push(
                view()
                    .test_id(&format!("editor-tab-file-{}", tab.file.rel_path))
                    .h(px(28.0))
                    .px(px(8.0))
                    .gap(px(6.0))
                    .flex_row()
                    .items_center()
                    .rounded(px(6.0))
                    .bg(if active { p.surface } else { transparent_black() })
                    .border_1()
                    .border_color(if active { p.border } else { transparent_black() })
                    .cursor(Cursor::Pointer)
                    .hover(move |s| if active { s } else { s.bg(p.surface2) })
                    .on_press(move |e: &PressEvent| {
                        crate::code_split_runtime::capture_views();
                        state::set_text_input_focused(false);
                        app_signal().update(|s| {
                            crate::code_split_runtime::select_tab_in_model(s, idx);
                            state::sync_dirty_working_diff_into_code_tab(s, idx);
                            s.editor_tab = EditorTabKind::File;
                            s.focus_zone = FocusZone::CodeEditor;
                            if e.clicks >= 2 && let Some(tab) = s.code_tabs.get_mut(idx) {
                                tab.preview = false;
                            }
                        });
                    })
                    .on_context_menu(move |e: &ContextMenuEvent| {
                        let id1 = tid_cm.clone();
                        let id2 = tid_cm.clone();
                        let mut items = build_location_menu_items(
                            f_cm.repo_id.clone(),
                            f_cm.repo_path.clone(),
                            f_cm.rel_path.clone(),
                            f_cm.abs_path.clone(),
                            line_cm,
                            false,
                        );
                        items.push(MenuItem::separator());
                        let split_id = tid_cm.clone();
                        items.push(MenuItem::new("向右拆分").on_select(move || {
                            crate::code_split_runtime::split_for_tab(&split_id);
                        }));
                        items.push(MenuItem::new("关闭").on_select(move || {
                            request_close_code_tab(&id1);
                        }));
                        items.push(MenuItem::new("关闭其他").on_select(move || {
                            request_close_other_code_tabs(&id2);
                        }));
                        items.push(MenuItem::new("关闭全部").on_select(|| {
                            request_close_all_code_tabs();
                        }));
                        crate::state::show_popup_menu_at(items, e.page_position);
                    })
                    .child(
                        view()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(badge.text),
                    )
                    .child(title_el)
                    .child(dirty.then(|| {
                        view()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded(px(3.0))
                            .bg(p.amber)
                    }))
                    .child(
                        view()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(px(3.0))
                            .items_center()
                            .justify_center()
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(move || request_close_code_tab(&tid_close))
                            .child(icons::icon(ICO_CLOSE, 10.0, p.text_muted)),
                    )
                    .into_element(),
            );
        }

        if let Some((cf, dirty, line_1)) = work_diff {
            let active = editor_tab == EditorTabKind::WorkingDiff;
            let cf_cm = cf.clone();
            let title = format!("Diff: {}", basename(&cf.path));
            tab_els.push(
                view()
                    .test_id("editor-tab-working-diff")
                    .h(px(28.0))
                    .px(px(8.0))
                    .gap(px(6.0))
                    .flex_row()
                    .items_center()
                    .rounded(px(6.0))
                    .bg(if active { p.surface } else { transparent_black() })
                    .border_1()
                    .border_color(if active { p.border } else { transparent_black() })
                    .cursor(Cursor::Pointer)
                    .hover(move |s| if active { s } else { s.bg(p.surface2) })
                    .on_press(|| {
                        state::set_text_input_focused(false);
                        app_signal().update(|s| {
                            state::sync_dirty_code_tab_into_working_diff(s);
                            s.editor_tab = EditorTabKind::WorkingDiff;
                            let side = s
                                .working_diff
                                .as_ref()
                                .map(|w| w.active_side)
                                .unwrap_or(crate::diff_model::DiffSide::New);
                            s.focus_zone = match side {
                                crate::diff_model::DiffSide::Old => FocusZone::WorkingDiffOld,
                                crate::diff_model::DiffSide::New => FocusZone::WorkingDiffNew,
                            };
                        });
                    })
                    .on_context_menu(move |e: &ContextMenuEvent| {
                        let mut items = build_location_menu_items(
                            cf_cm.repo_id.clone(),
                            cf_cm.repo_path.clone(),
                            cf_cm.path.clone(),
                            cf_cm.abs_path(),
                            line_1,
                            false,
                        );
                        items.push(MenuItem::separator());
                        items.push(MenuItem::new("关闭差异").on_select(request_close_working_diff));
                        crate::state::show_popup_menu_at(items, e.page_position);
                    })
                    .child(icons::icon(ICO_COLUMNS, 11.0, status_text_color(&cf.kind, p)))
                    .child(
                        text(title)
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text_primary)
                            .number_of_lines(1u32),
                    )
                    .child(dirty.then(|| {
                        view().w(px(6.0)).h(px(6.0)).rounded(px(3.0)).bg(p.amber)
                    }))
                    .child(
                        view()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(px(3.0))
                            .items_center()
                            .justify_center()
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(request_close_working_diff)
                            .child(icons::icon(ICO_CLOSE, 10.0, p.text_muted)),
                    )
                    .into_element(),
            );
        }

        if let Some((commit, new_path, line_1)) = commit_diff {
            let active = editor_tab == EditorTabKind::CommitDiff;
            let title = format!("Diff: {}", basename(&new_path));
            let abs = std::path::Path::new(&commit.repo_path)
                .join(&new_path)
                .to_string_lossy()
                .to_string();
            tab_els.push(
                view()
                    .test_id("editor-tab-commit-diff")
                    .h(px(28.0))
                    .px(px(8.0))
                    .gap(px(6.0))
                    .flex_row()
                    .items_center()
                    .rounded(px(6.0))
                    .bg(if active { p.surface } else { transparent_black() })
                    .border_1()
                    .border_color(if active { p.border } else { transparent_black() })
                    .cursor(Cursor::Pointer)
                    .hover(move |s| if active { s } else { s.bg(p.surface2) })
                    .on_press(|| {
                        app_signal().update(|s| {
                            s.editor_tab = EditorTabKind::CommitDiff;
                            let side = s
                                .commit_diff
                                .as_ref()
                                .map(|c| c.active_side)
                                .unwrap_or(crate::diff_model::DiffSide::New);
                            s.focus_zone = match side {
                                crate::diff_model::DiffSide::Old => FocusZone::CommitDiffOld,
                                crate::diff_model::DiffSide::New => FocusZone::CommitDiffNew,
                            };
                        });
                    })
                    .on_context_menu(move |e: &ContextMenuEvent| {
                        let mut items = build_location_menu_items(
                            commit.repo_id.clone(),
                            commit.repo_path.clone(),
                            new_path.clone(),
                            abs.clone(),
                            line_1,
                            false,
                        );
                        items.push(MenuItem::separator());
                        items.push(MenuItem::new("关闭差异").on_select(close_commit_diff_now));
                        crate::state::show_popup_menu_at(items, e.page_position);
                    })
                    .child(icons::icon(ICO_COLUMNS, 11.0, p.blue))
                    .child(
                        text(title)
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text_primary)
                            .number_of_lines(1u32),
                    )
                    .child(
                        view()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(px(3.0))
                            .items_center()
                            .justify_center()
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(close_commit_diff_now)
                            .child(icons::icon(ICO_CLOSE, 10.0, p.text_muted)),
                    )
                    .into_element(),
            );
        }

        view()
            .test_id("editor-tab-bar")
            .h(px(36.0))
            .w_full()
            .px(px(6.0))
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .bg(p.rail_bg)
            .border_b_1()
            .border_color(p.border)
            .child(
                scroll()
                    .horizontal()
                    .flex_1()
                    .child(view().flex_row().items_center().gap(px(4.0)).children(tab_els)),
            )
            .child(
                view()
                    .test_id("editor-all-tabs-btn")
                    .w(px(24.0))
                    .h(px(24.0))
                    .rounded(px(4.0))
                    .items_center()
                    .justify_center()
                    .cursor(Cursor::Pointer)
                    .hover(move |s| s.bg(p.surface_hover))
                    .label("全部标签")
                    .on_press(|| {
                        let mut menu_items = Vec::new();
                        let (tabs, active_i, cur_tab, wd, cd) = app_signal().with_untracked(|s| {
                            (
                                s.code_tabs.iter().map(|t| t.title()).collect::<Vec<_>>(),
                                s.active_code_tab_idx,
                                s.editor_tab,
                                s.working_diff.as_ref().map(|w| basename(&w.file.path)),
                                s.commit_diff.as_ref().map(|c| basename(&c.data.new_path)),
                            )
                        });
                        for (i, title) in tabs.into_iter().enumerate() {
                            menu_items.push(
                                MenuItem::new(title)
                                    .checked(cur_tab == EditorTabKind::File && i == active_i)
                                    .on_select(move || {
                                        state::set_text_input_focused(false);
                                        app_signal().update(|s| {
                                            crate::code_split_runtime::select_tab_in_model(s, i);
                                            state::sync_dirty_working_diff_into_code_tab(s, i);
                                            s.editor_tab = EditorTabKind::File;
                                            s.focus_zone = FocusZone::CodeEditor;
                                        });
                                    }),
                            );
                        }
                        if let Some(name) = wd {
                            menu_items.push(
                                MenuItem::new(format!("Diff: {name}"))
                                    .checked(cur_tab == EditorTabKind::WorkingDiff)
                                    .on_select(|| {
                                        state::set_text_input_focused(false);
                                        app_signal().update(|s| {
                                            state::sync_dirty_code_tab_into_working_diff(s);
                                            s.editor_tab = EditorTabKind::WorkingDiff;
                                            let side = s
                                                .working_diff
                                                .as_ref()
                                                .map(|w| w.active_side)
                                                .unwrap_or(crate::diff_model::DiffSide::New);
                                            s.focus_zone = match side {
                                                crate::diff_model::DiffSide::Old => {
                                                    FocusZone::WorkingDiffOld
                                                }
                                                crate::diff_model::DiffSide::New => {
                                                    FocusZone::WorkingDiffNew
                                                }
                                            };
                                        });
                                    }),
                            );
                        }
                        if let Some(name) = cd {
                            menu_items.push(
                                MenuItem::new(format!("Diff: {name}"))
                                    .checked(cur_tab == EditorTabKind::CommitDiff)
                                    .on_select(|| {
                                        state::set_text_input_focused(false);
                                        app_signal().update(|s| {
                                            s.editor_tab = EditorTabKind::CommitDiff;
                                            let side = s
                                                .commit_diff
                                                .as_ref()
                                                .map(|c| c.active_side)
                                                .unwrap_or(crate::diff_model::DiffSide::New);
                                            s.focus_zone = match side {
                                                crate::diff_model::DiffSide::Old => {
                                                    FocusZone::CommitDiffOld
                                                }
                                                crate::diff_model::DiffSide::New => {
                                                    FocusZone::CommitDiffNew
                                                }
                                            };
                                        });
                                    }),
                            );
                        }
                        crate::state::show_popup_menu(menu_items);
                    })
                    .child(icons::icon(ICO_CHEVRON_DOWN, 11.0, p.text_muted)),
            )
            .into_element()
    })
}

pub fn welcome_view() -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let row = |label: &'static str, hint: &'static str, on_click: fn()| {
            view()
                .w(px(260.0))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(6.0))
                .flex_row()
                .items_center()
                .justify_between()
                .cursor(Cursor::Pointer)
                .hover(move |s| s.bg(p.surface_hover))
                .on_press(on_click)
                .child(text(label).text_size(px(12.0)).text_color(p.text_muted))
                .child(
                    view()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(p.surface2)
                        .child(text(hint).text_size(px(10.5)).text_color(p.text_control)),
                )
        };

        view()
            .flex_1()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .bg(p.app_bg)
            .child(
                text("双击左侧变更文件或按 ⌘D 查看差异")
                    .text_size(px(13.0))
                    .text_color(p.text_secondary),
            )
            .child(row("转到文件", "⌘⇧O", || {
                app_signal().update(|s| {
                    s.picker_open = Some(PickerKind::GoToFile);
                    s.picker_query.clear();
                    state::enter_modal_focus(s);
                });
            }))
            .child(row("最近位置", "⌘⇧E", || {
                app_signal().update(|s| {
                    s.picker_open = Some(PickerKind::RecentLocations);
                    s.picker_query.clear();
                    state::enter_modal_focus(s);
                });
            }))
            .child(row("Git 提交历史", "⌘9", || {
                app_signal().update(|s| s.show_git_panel = !s.show_git_panel);
            }))
            .child(row("刷新工作区", "⌘⌥Y", || {
                state::refresh_workspace_async(None);
            }))
    })
}
