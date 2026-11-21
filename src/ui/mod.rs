pub mod bookmarks;
pub mod code_editor;
pub mod commit_pane;
pub mod content_search;
pub(crate) mod comparison;
mod replacement_flow;
mod key_gestures;
pub(crate) mod navigation_picker;
mod split_code_view;
pub(crate) mod revision_picker;
pub mod diff_view;
pub mod editor_area;
pub mod editor_tools;
pub mod git_pane;
pub mod icons;
pub mod modals;
pub mod project_pane;

use nsh_base::{EditorCommand, h_resizable, resizable_panel};
use nsh_core::desktop::{MenuRole, TitleBarOptions, TitleBarStyle, WindowAttributes};
use nsh_core::element::desktop::{
    MenuItem, current_window, set_application_menu, title_bar, window_state,
};
use nsh_core::keymap::When;
use nsh_core::platform::Lifecycle;
use nsh_core::prelude::*;

use crate::paths::{basename, display_path};
use crate::state::{
    self, EditorTabKind, FocusZone, LeftTab, PickerKind, app_signal, close_active_tab,
    copy_absolute_path_current, copy_for_current_focus, copy_reference_current, current_palette,
    init_workspace, jump_diff_hunk, jump_from_diff_to_source, locate_current_in_project_tree,
    move_change_selection, move_commit_diff_file, navigate_back, navigate_forward,
    open_commit_diff, open_file_revision_history, open_find_bar, open_goto_line_modal,
    open_working_diff, refresh_workspace_async, save_active_document, stage_selected_change_sync,
    toggle_selected_change_checkbox,
};
use crate::theme;
use crate::ui::icons::*;

fn setup_hidden_title_bar() {
    let mut bar = TitleBarOptions::default();
    bar.style = TitleBarStyle::Hidden;
    bar.height = Some(px(44.0));
    let mut attrs = WindowAttributes::default();
    attrs.title_bar = Some(bar);
    current_window().set(attrs);
}

/// NSH 先派发根快捷键再匹配应用菜单快捷键，因此 `setup_shortcuts` 在 Modal 下的早退
/// 不足以阻止原生菜单项（含鼠标激活与陈旧菜单事件）穿透；此处以运行时焦点再兜底一次。
fn modal_owns_focus() -> bool {
    app_signal().with_untracked(|s| s.focus_zone == FocusZone::Modal)
}

/// 将应用级菜单命令（导航/Git/关闭标签）包装为模态弹窗持有焦点时直接丢弃，
/// 防止菜单重建滞后期间仍能通过键盘或鼠标触发后台命令。
fn guard_app_command(f: impl Fn() + 'static) -> impl Fn() + 'static {
    move || {
        if !modal_owns_focus() {
            f();
        }
    }
}

fn build_application_menu(text_input_focused: bool, modal_open: bool, code_undo: bool) -> Vec<MenuItem> {
    let copy_item = if text_input_focused {
        // 焦点位于原生 <input>（筛选框、查找/替换框、转到行/文件框）时，使用标准 responder copy:，绝不劫持输入框 ⌘C
        MenuItem::role(MenuRole::Copy)
    } else {
        // 焦点位于 Diff 左右侧、源码 Editor 或 Commit/Project/Git 树时，允许点击顶部原生「编辑 > 复制」直达焦点路由复制
        MenuItem::new("复制")
            .shortcut("mod+c")
            .on_select(copy_for_current_focus)
    };
    vec![
        MenuItem::submenu(
            "Nemorous",
            [
                MenuItem::role(MenuRole::About),
                MenuItem::separator(),
                MenuItem::role(MenuRole::Hide),
                MenuItem::role(MenuRole::HideOthers),
                MenuItem::role(MenuRole::ShowAll),
                MenuItem::separator(),
                MenuItem::role(MenuRole::Quit),
            ],
        ),
        MenuItem::submenu(
            "编辑",
            [
                if code_undo {
                    MenuItem::new("撤销").shortcut("mod+z")
                        .on_select(guard_app_command(|| crate::code_split_runtime::undo(false)))
                } else { MenuItem::role(MenuRole::Undo) },
                if code_undo {
                    MenuItem::new("重做").shortcut("mod+shift+z")
                        .on_select(guard_app_command(|| crate::code_split_runtime::undo(true)))
                } else { MenuItem::role(MenuRole::Redo) },
                MenuItem::separator(),
                MenuItem::role(MenuRole::Cut),
                copy_item,
                MenuItem::role(MenuRole::Paste),
                MenuItem::role(MenuRole::SelectAll),
            ],
        ),
        MenuItem::submenu("Git", [
            MenuItem::new("查看文件历史").on_select(guard_app_command(revision_picker::show_active_history)).disabled(modal_open),
            MenuItem::new("与修订版本比较…").on_select(guard_app_command(revision_picker::open_active)).disabled(modal_open),
        ]),
        MenuItem::submenu(
            "导航",
            [
                MenuItem::new("转到文件…")
                    .shortcut("mod+shift+o")
                    .on_select(guard_app_command(|| {
                        app_signal().update(|s| {
                            s.picker_open = Some(PickerKind::GoToFile);
                            s.picker_query.clear();
                            state::enter_modal_focus(s);
                        });
                    }))
                    .disabled(modal_open),
                MenuItem::new("最近位置…")
                    .shortcut("mod+shift+e")
                    .on_select(guard_app_command(|| {
                        app_signal().update(|s| {
                            s.picker_open = Some(PickerKind::RecentLocations);
                            s.picker_query.clear();
                            state::enter_modal_focus(s);
                        });
                    }))
                    .disabled(modal_open),
                MenuItem::new("转到行:列…")
                    .shortcut("mod+l")
                    .on_select(guard_app_command(open_goto_line_modal))
                    .disabled(modal_open),
                MenuItem::separator(),
                MenuItem::new("后退")
                    .shortcut("mod+[")
                    .on_select(guard_app_command(navigate_back))
                    .disabled(modal_open),
                MenuItem::new("前进")
                    .shortcut("mod+]")
                    .on_select(guard_app_command(navigate_forward))
                    .disabled(modal_open),
                MenuItem::new("在文件树中定位")
                    .shortcut("alt+f1")
                    .on_select(guard_app_command(locate_current_in_project_tree))
                    .disabled(modal_open),
            ],
        ),
        MenuItem::submenu(
            "Git",
            [
                MenuItem::new("刷新工作区")
                    .shortcut("mod+alt+y")
                    .on_select(guard_app_command(|| refresh_workspace_async(None)))
                    .disabled(modal_open),
                MenuItem::submenu(
                    "选中的文件",
                    [MenuItem::new("查看历史").on_select(guard_app_command(|| {
                        let info = app_signal().with_untracked(|s| {
                            state::current_location_info().and_then(|(rpath, rel, _, _)| {
                                s.repos
                                    .iter()
                                    .find(|r| r.path == rpath)
                                    .cloned()
                                    .map(|repo| (repo, rel))
                            })
                        });
                        if let Some((repo, rel)) = info {
                            open_file_revision_history(repo, "HEAD".to_string(), rel);
                        } else {
                            state::show_toast("info", "请先选择一个文件");
                        }
                    }))],
                )
                .disabled(modal_open),
            ],
        ),
        MenuItem::submenu(
            "窗口",
            [
                MenuItem::role(MenuRole::Minimize),
                MenuItem::new("关闭标签")
                    .shortcut("mod+w")
                    .on_select(guard_app_command(close_active_tab))
                    .disabled(modal_open),
            ],
        ),
    ]
}

fn setup_application_menu() {
    let last_mode = stored(None::<(bool, bool, bool)>);
    let modal_memo = computed(|| app_signal().with(|s| s.focus_zone == FocusZone::Modal));
    effect(move || {
        let input_focused = state::text_input_focus_signal().with(|f| f.focused);
        let modal_open = modal_memo.get();
        let code_undo = !input_focused
            && app_signal().with(|s| s.focus_zone == FocusZone::CodeEditor);
        let mode = (input_focused, modal_open, code_undo);
        if last_mode.get() != Some(mode) {
            last_mode.replace(Some(mode));
            set_application_menu(build_application_menu(input_focused, modal_open, code_undo));
        }
    });
}

fn focus_tool_window(tab: LeftTab, toggle: bool) {
    state::set_text_input_focused(false);
    app_signal().update(|s| {
        let focus = match tab {
            LeftTab::Commit => FocusZone::CommitTree,
            LeftTab::Project => FocusZone::ProjectTree,
        };
        if toggle && s.show_left_panel && s.left_tab == tab && s.focus_zone == focus {
            s.show_left_panel = false;
        } else {
            s.show_left_panel = true;
            s.left_tab = tab;
            s.focus_zone = focus;
            if tab == LeftTab::Commit && s.change_selection.is_none() {
                s.change_selection = state::visible_change_items(s).into_iter().next();
            }
        }
    });
}

/// 绑定全局及焦点感知 IDEA 快捷键。
/// 通过订阅 `focus_zone` 变化在 `effect` 中重绑，确保后注册优先级覆盖 NSH `Editor` 内置冲突键位
/// （如 `mod+l`, `mod+[`, `mod+]`, `mod+d`, `cmd+down`）。
fn setup_shortcuts() {
    let always = KeyOptions {
        when: When::Always,
        ..Default::default()
    };
    let not_editing = KeyOptions {
        when: When::NotEditing,
        ..Default::default()
    };

    let zone_memo = computed(|| app_signal().with(|s| s.focus_zone));
    let tab_memo = computed(|| app_signal().with(|s| s.editor_tab));
    let workspace_search_open = computed(content_search::is_open);
    let multiline_search = computed(content_search::query_is_multiline);
    let navigation_open = computed(navigation_picker::is_open);
    let revision_open = computed(revision_picker::is_open);
    let switcher_open = computed(navigation_picker::is_switcher);
    let dismissable_memo = computed(|| {
        app_signal().with(|s| {
            s.focus_zone == FocusZone::Modal
                || s.save_conflict.is_some()
                || s.dirty_close.is_some()
                || s.goto_line_open
                || s.picker_open.is_some()
                || s.local_history_modal.is_some()
                || s.stash_modal_open
                || s.diff_find_open
                || s.code_find_open
        })
    });
    effect(move || {
        // 在 focus_zone / editor_tab / 输入框聚焦 / 弹窗状态切换或 Editor 触发重绑后重注全局键位，
        // 确保覆盖 Editor::bind_keys 的内置冲突键位（如 mod+l），同时严守弹窗隔离与文本编辑器按词移动。
        let zone = zone_memo.get();
        let tab = tab_memo.get();
        let text_input_focused = state::text_input_focus_signal().with(|f| f.focused);
        let has_dismissable = dismissable_memo.get();
        let _rebind_rev = state::shortcut_rebind_signal().with(|r| r.rev);

        // 模态弹窗打开时由弹窗独占键盘（对齐基线 main.go:401 overlayOpen）：仅响应 Escape 关闭，绝不穿透触发后台快捷键！
        if zone == FocusZone::Modal {
            on_key_with("escape", always, |_| {
                state::close_active_modal();
            });
            if revision_open.get() {
                on_key_with("up", always, |_| revision_picker::move_selection(-1));
                on_key_with("down", always, |_| revision_picker::move_selection(1));
                on_key_with("enter", always, |_| revision_picker::accept());
            }
            if navigation_open.get() {
                on_key_with("up", always, |_| navigation_picker::move_selection(-1));
                on_key_with("down", always, |_| navigation_picker::move_selection(1));
                if switcher_open.get() {
                    on_key_with("enter", always, |_| navigation_picker::accept());
                    on_key_with("ctrl+tab", always, |_| navigation_picker::start_or_move_switcher(false));
                    on_key_with("ctrl+shift+tab", always, |_| navigation_picker::start_or_move_switcher(true));
                }
            }
            if workspace_search_open.get() {
                if multiline_search.get() {
                    on_key_with("mod+enter", always, |_| content_search::activate_selected());
                } else {
                    on_key_with("up", always, |_| content_search::move_selection(-1));
                    on_key_with("down", always, |_| content_search::move_selection(1));
                }
            }
            return;
        }

        on_key_with("mod+e", always, |_| navigation_picker::open(navigation_picker::Mode::RecentFiles));
        on_key_with("mod+shift+a", always, |_| navigation_picker::open(navigation_picker::Mode::Actions));
        on_key_with("mod+shift+p", always, |_| navigation_picker::open(navigation_picker::Mode::Actions));
        on_key_with("ctrl+tab", always, |_| navigation_picker::start_or_move_switcher(false));
        on_key_with("ctrl+shift+tab", always, |_| navigation_picker::start_or_move_switcher(true));
        on_key_with("mod+s", always, |_| {
            save_active_document();
        });
        if zone == FocusZone::CodeEditor && !text_input_focused {
            on_key_with("mod+z", always, |_| crate::code_split_runtime::undo(false));
            on_key_with("mod+shift+z", always, |_| crate::code_split_runtime::undo(true));
        }
        on_key_with("mod+w", always, |_| {
            close_active_tab();
        });
        on_key_with("mod+l", always, |_| {
            open_goto_line_modal();
        });
        on_key_with("mod+[", always, |_| {
            navigate_back();
        });
        on_key_with("mod+]", always, |_| {
            navigate_forward();
        });
        on_key_with("mod+shift+f", always, |_| {
            content_search::open_workspace();
        });
        on_key_with("mod+shift+r", always, |_| {
            content_search::open_replace();
        });
        on_key_with("mod+f12", always, |_| {
            editor_tools::open_structure();
        });
        on_key_with("mod+shift+t", always, |_| {
            editor_tools::open_todos();
        });
        on_key_with("mod+alt+l", always, |_| {
            crate::format_runtime::format_active();
        });
        on_key_with("mod+shift+b", always, |_| {
            bookmarks::toggle_current_line();
        });
        on_key_with("mod+f", always, |_| {
            open_find_bar(false);
        });
        on_key_with("mod+r", always, |_| {
            open_find_bar(true);
        });
        on_key_with("mod+alt+f", always, |_| {
            open_find_bar(true);
        });
        on_key_with("mod+g", always, |_| {
            state::run_active_find_nav(true);
        });
        on_key_with("mod+shift+g", always, |_| {
            state::run_active_find_nav(false);
        });
        on_key_with("mod+shift+o", always, |_| {
            app_signal().update(|s| {
                s.picker_open = Some(PickerKind::GoToFile);
                s.picker_query.clear();
                state::enter_modal_focus(s);
            });
        });
        on_key_with("mod+shift+e", always, |_| {
            app_signal().update(|s| {
                s.picker_open = Some(PickerKind::RecentLocations);
                s.picker_query.clear();
                state::enter_modal_focus(s);
            });
        });
        on_key_with("mod+alt+y", always, |_| {
            refresh_workspace_async(None);
        });
        on_key_with("mod+shift+c", always, |_| {
            copy_absolute_path_current();
        });
        on_key_with("mod+alt+shift+c", always, |_| {
            copy_reference_current();
        });
        on_key_with("alt+f1", always, |_| {
            locate_current_in_project_tree();
        });
        on_key_with("f4", always, |_| {
            jump_from_diff_to_source();
        });
        if matches!(
            zone,
            FocusZone::WorkingDiffOld
                | FocusZone::WorkingDiffNew
                | FocusZone::CommitDiffOld
                | FocusZone::CommitDiffNew
        ) {
            on_key_with("cmd+down", always, |_| {
                jump_from_diff_to_source();
            });
        }
        // ⌥←/⌥→ 在源码中移动光标，在 Diff 中切换文件。
        if !text_input_focused && tab == EditorTabKind::File && zone == FocusZone::CodeEditor {
            let repeat = KeyOptions {
                repeat: true,
                for_focus: true,
                ..always
            };
            for (key, command) in [
                ("alt+left", EditorCommand::MoveWordLeft),
                ("alt+right", EditorCommand::MoveWordRight),
            ] {
                on_key_with(key, repeat, move |_| {
                    if let Some(api) = state::active_editor_api(FocusZone::CodeEditor) {
                        api.run(command);
                    }
                });
            }
        }

        // Diff 专用导航（F7/⇧F7 跳块、⌥←/⌥→ 切文件）仅在非 <input> 且当前为 Diff 视图时注册，
        // 绝不劫持 CodeEditor 或任何 <input> 内的 ⌥←/⌥→ 按词移动（对齐基线 main.go:431 + review_buffers.go:234）。
        if !text_input_focused
            && matches!(tab, EditorTabKind::WorkingDiff | EditorTabKind::CommitDiff)
            && zone != FocusZone::CodeEditor
        {
            on_key_with("f7", always, |_| {
                jump_diff_hunk(1);
            });
            on_key_with("shift+f7", always, |_| {
                jump_diff_hunk(-1);
            });
            on_key_with("alt+left", always, |_| {
                let cur_tab = app_signal().with_untracked(|s| s.editor_tab);
                match cur_tab {
                    EditorTabKind::CommitDiff => {
                        move_commit_diff_file(-1);
                    }
                    EditorTabKind::WorkingDiff => {
                        move_change_selection(-1, true);
                    }
                    _ => {}
                }
            });
            on_key_with("alt+right", always, |_| {
                let cur_tab = app_signal().with_untracked(|s| s.editor_tab);
                match cur_tab {
                    EditorTabKind::CommitDiff => {
                        move_commit_diff_file(1);
                    }
                    EditorTabKind::WorkingDiff => {
                        move_change_selection(1, true);
                    }
                    _ => {}
                }
            });
        }

        // 工具窗口快捷键：⌘0 Commit、⌘1 Project、⌘9 Git
        // 对齐基线 review_toolwindow.go：当面板已打开但焦点在编辑器/Diff 时，按 ⌘0/⌘1 先切焦点到对应树并保留编辑器，仅当焦点已在对应树内再按才折叠。
        on_key_with("mod+0", always, |_| focus_tool_window(LeftTab::Commit, true));
        on_key_with("mod+1", always, |_| focus_tool_window(LeftTab::Project, true));
        on_key_with("mod+9", always, |_| {
            content_search::hide_find_panel();
            app_signal().update(|s| {
                s.show_git_panel = !s.show_git_panel;
            });
        });

        // 树焦点快捷键（Commit 树 / Project 树 / Git 提交文件树）
        on_key_with("mod+d", always, move |_| {
            match zone {
                FocusZone::CommitTree => {
                    let sel = app_signal().with_untracked(|s| s.change_selection.clone());
                    if let Some(it) = sel {
                        open_working_diff(it.file, it.staged, true);
                    } else if let Some(first) = app_signal()
                        .with_untracked(|s| state::visible_change_items(s).into_iter().next())
                    {
                        open_working_diff(first.file, first.staged, true);
                    }
                }
                FocusZone::ProjectTree => {
                    comparison::open_project_selection();
                }
                FocusZone::GitFiles => {
                    let pair = app_signal().with_untracked(|s| {
                        s.selected_commit
                            .clone()
                            .zip(s.selected_commit_file.clone())
                    });
                    if let Some((c, f)) = pair {
                        open_commit_diff(c, f, true);
                    }
                }
                _ => {}
            }
        });

        if matches!(
            zone,
            FocusZone::CommitTree | FocusZone::ProjectTree | FocusZone::GitFiles
        ) {
            on_key_with("mod+c", not_editing, |_| {
                copy_for_current_focus();
            });
            on_key_with("enter", not_editing, move |_| match zone {
                FocusZone::CommitTree => {
                    if let Some(sel) = app_signal().with_untracked(|s| s.change_selection.clone()) {
                        open_working_diff(sel.file, sel.staged, true);
                    }
                }
                FocusZone::ProjectTree => {
                    if let Some(wf) =
                        app_signal().with_untracked(|s| s.project_selected_file.clone())
                    {
                        state::open_code_file(wf, None, true);
                    }
                }
                FocusZone::GitFiles => {
                    let pair = app_signal().with_untracked(|s| {
                        s.selected_commit
                            .clone()
                            .zip(s.selected_commit_file.clone())
                    });
                    if let Some((c, f)) = pair {
                        open_commit_diff(c, f, true);
                    }
                }
                _ => {}
            });
        }

        if zone == FocusZone::CommitTree {
            on_key_with("up", not_editing, |_| {
                move_change_selection(-1, false);
            });
            on_key_with("down", not_editing, |_| {
                move_change_selection(1, false);
            });
            on_key_with("space", not_editing, |_| {
                toggle_selected_change_checkbox();
            });
            on_key_with("s", not_editing, |_| {
                stage_selected_change_sync(true);
            });
            on_key_with("u", not_editing, |_| {
                stage_selected_change_sync(false);
            });
        }

        if zone == FocusZone::ProjectTree {
            on_key_with("up", not_editing, |_| {
                state::move_project_selection(-1);
            });
            on_key_with("down", not_editing, |_| {
                state::move_project_selection(1);
            });
            on_key_with("right", not_editing, |_| {
                state::toggle_project_selected_dir(true);
            });
            on_key_with("left", not_editing, |_| {
                state::toggle_project_selected_dir(false);
            });
        }

        // Escape 仅在存在弹窗或查找条时拦截关闭并恢复焦点；平时透传给 Editor 清除多选区
        if has_dismissable {
            on_key_with("escape", always, |_| {
                state::close_active_modal();
            });
        }
    });
}

/// 监听窗口切回前台与系统生命周期恢复，自动刷新工作区。
fn setup_window_focus_refresh() {
    let was_focused = stored(true);
    effect(move || {
        let focused = window_state().map(|w| w.focused).unwrap_or(true);
        let lc = lifecycle();
        let active = focused && lc == Lifecycle::Active;
        let prev = was_focused.replace(active);
        if active && !prev {
            refresh_workspace_async(None);
        }
    });
}

fn top_header_bar() -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let bar = title_bar();
        let left_pad = bar.left.0.max(76.0);
        let (ws_path, repo_count, branch, is_refreshing) = app_signal().with(|s| {
            let br = s
                .statuses
                .first()
                .map(|st| st.branch.clone())
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| "main".to_string());
            (
                s.cfg.workspace_path.clone(),
                s.repos.len(),
                br,
                s.is_refreshing,
            )
        });
        let ws_name = if ws_path.trim().is_empty() {
            "选择工作区".to_string()
        } else {
            basename(&ws_path)
        };

        view()
            .test_id("top-header-bar")
            .h(px(44.0))
            .w_full()
            .pl(px(left_pad))
            .pr(px(14.0))
            .flex_row()
            .items_center()
            .justify_between()
            .bg(p.rail_bg)
            .border_b_1()
            .border_color(p.border)
            .window_drag(true)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .window_drag(false)
                    .child(
                        view()
                            .h(px(28.0))
                            .px(px(10.0))
                            .gap(px(6.0))
                            .flex_row()
                            .items_center()
                            .rounded(px(6.0))
                            .bg(p.surface2)
                            .child(icons::icon(ICO_FOLDER, 13.0, p.text_control))
                            .child(
                                text(ws_name)
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary)
                                    .number_of_lines(1u32),
                            )
                            .child(
                                text(format!("{repo_count}"))
                                    .text_size(px(11.0))
                                    .text_color(p.text_muted),
                            ),
                    )
                    .child(
                        view()
                            .h(px(28.0))
                            .px(px(10.0))
                            .gap(px(6.0))
                            .flex_row()
                            .items_center()
                            .rounded(px(6.0))
                            .bg(p.surface2)
                            .child(icons::icon(ICO_BRANCH, 13.0, p.accent))
                            .child(
                                text(branch)
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.text_primary)
                                    .number_of_lines(1u32),
                            ),
                    ),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .window_drag(false)
                    .child(
                        view()
                            .test_id("top-refresh-btn")
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(5.0))
                            .items_center()
                            .justify_center()
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .label("刷新工作区 (⌘⌥Y)")
                            .on_press(|| refresh_workspace_async(None))
                            .child(icons::icon(
                                ICO_REFRESH,
                                13.0,
                                if is_refreshing { p.accent } else { p.text_control },
                            )),
                    )
                    .child(
                        view()
                            .test_id("top-search-btn")
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(5.0))
                            .items_center()
                            .justify_center()
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .label("随处搜索")
                            .on_press(|| navigation_picker::open(navigation_picker::Mode::Everywhere))
                            .child(icons::icon(ICO_SEARCH, 13.0, p.text_control)),
                    ),
            )
    })
}

fn left_rail() -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let (show_left, left_tab, show_git, theme_name) = app_signal().with(|s| {
            (
                s.show_left_panel,
                s.left_tab,
                s.show_git_panel,
                s.cfg.theme.clone(),
            )
        });

        let rail_btn = |tid: &'static str,
                        label: &'static str,
                        svg_src: &'static str,
                        active: bool,
                        on_click: fn()| {
            view()
                .test_id(tid)
                .w(px(34.0))
                .h(px(34.0))
                .rounded(px(7.0))
                .bg(if active { p.accent_soft } else { transparent_black() })
                .items_center()
                .justify_center()
                .cursor(Cursor::Pointer)
                .hover(move |s| if active { s } else { s.bg(p.surface_hover) })
                .label(label)
                .on_press(on_click)
                .child(icons::icon(
                    svg_src,
                    16.0,
                    if active { p.accent_soft_text } else { p.text_control },
                ))
        };

        let theme_icon = match theme_name.as_str() {
            "dark" | "darcula" => ICO_MOON,
            "system" => ICO_MONITOR,
            _ => ICO_SUN,
        };

        view()
            .test_id("left-rail")
            .w(px(46.0))
            .flex_none()
            .flex_col()
            .items_center()
            .py(px(8.0))
            .gap(px(6.0))
            .bg(p.rail_bg)
            .border_r_1()
            .border_color(p.border)
            .child(rail_btn(
                "rail-commit-btn",
                "Commit (⌘0)",
                ICO_COMMIT,
                show_left && left_tab == LeftTab::Commit,
                || {
                    app_signal().update(|s| {
                        if s.show_left_panel && s.left_tab == LeftTab::Commit {
                            s.show_left_panel = false;
                        } else {
                            s.show_left_panel = true;
                            s.left_tab = LeftTab::Commit;
                            s.focus_zone = FocusZone::CommitTree;
                        }
                    });
                },
            ))
            .child(rail_btn(
                "rail-project-btn",
                "Project (⌘1)",
                ICO_FOLDER,
                show_left && left_tab == LeftTab::Project,
                || {
                    app_signal().update(|s| {
                        if s.show_left_panel && s.left_tab == LeftTab::Project {
                            s.show_left_panel = false;
                        } else {
                            s.show_left_panel = true;
                            s.left_tab = LeftTab::Project;
                            s.focus_zone = FocusZone::ProjectTree;
                        }
                    });
                },
            ))
            .child(rail_btn(
                "rail-git-btn",
                "Git (⌘9)",
                ICO_BRANCH,
                show_git,
                || {
                    content_search::hide_find_panel();
                    app_signal().update(|s| s.show_git_panel = !s.show_git_panel);
                },
            ))
            .child(content_search::find_panel_available().then(|| {
                rail_btn(
                    "rail-find-btn",
                    "查找结果",
                    ICO_SEARCH,
                    content_search::find_panel_visible(),
                    content_search::toggle_find_panel,
                )
            }))
            .child(view().flex_1())
            .child(rail_btn(
                "rail-theme-btn",
                "切换主题",
                theme_icon,
                false,
                || {
                    let next = app_signal()
                        .with_untracked(|s| theme::next_theme(&s.cfg.theme).to_string());
                    state::set_theme_name(next);
                },
            ))
    })
}

fn status_bar() -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let (ws_display, cursor_text, hint) = app_signal().with(|s| {
            let ws = display_path(&s.cfg.workspace_path);
            let cur = match s.editor_tab {
                EditorTabKind::File => s
                    .code_tabs
                    .get(s.active_code_tab_idx)
                    .map(|t| format!("{}:{}", t.cursor.line + 1, t.cursor.column + 1)),
                EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|w| {
                    let c = w.active_cursor();
                    format!("{}:{}", c.line + 1, c.column + 1)
                }),
                EditorTabKind::CommitDiff => s.commit_diff.as_ref().map(|c| {
                    let pos = c.active_cursor();
                    format!("{}:{}", pos.line + 1, pos.column + 1)
                }),
                EditorTabKind::Welcome | EditorTabKind::Comparison => None,
            };
            let hint = if s.left_tab == LeftTab::Commit {
                "↑↓ 选择 · 空格勾选 · ⌘D 差异 · F4 源码 · ⌘[ 返回"
            } else {
                "⌘0 Commit · ⌘1 Project · ⌘9 Git · ⌘⇧O 转到文件 · ⌘L 转到行"
            };
            (ws, cur, hint)
        });

        view()
            .test_id("status-bar")
            .h(px(25.0))
            .w_full()
            .px(px(12.0))
            .flex_row()
            .items_center()
            .justify_between()
            .bg(p.rail_bg)
            .border_t_1()
            .border_color(p.border)
            .child(
                text(ws_display)
                    .text_size(px(11.0))
                    .text_color(p.text_muted)
                    .number_of_lines(1u32),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .child(cursor_text.map(|pos_txt| {
                        view()
                            .test_id("status-cursor-pos")
                            .px(px(6.0))
                            .py(px(1.5))
                            .rounded(px(4.0))
                            .bg(p.surface2)
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(open_goto_line_modal)
                            .child(text(pos_txt).text_size(px(10.5)).text_color(p.text_control))
                    }))
                    .child(text(hint).text_size(px(10.5)).text_color(p.text_muted)),
            )
    })
}

fn toast_layer() -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let toast = app_signal().with(|s| s.toast.clone());
        let Some(t) = toast else {
            return view().w(px(0.0)).h(px(0.0)).into_element();
        };
        let (ico, col) = match t.kind {
            "ok" => (ICO_CHECK, p.green),
            "warn" => (ICO_ALERT, p.amber),
            "err" => (ICO_ALERT, p.danger),
            _ => (ICO_REFRESH, p.accent),
        };
        view()
            .test_id("toast-notification")
            .absolute()
            .right(px(14.0))
            .bottom(px(33.0))
            .w(px(320.0))
            .px(px(14.0))
            .py(px(10.0))
            .gap(px(8.0))
            .flex_row()
            .items_center()
            .rounded(px(8.0))
            .bg(p.surface)
            .border_1()
            .border_color(p.border_strong)
            .child(icons::icon(ico, 14.0, col))
            .child(
                view().flex_1().child(
                    text(t.message)
                        .text_size(px(11.5))
                        .text_color(p.text_primary)
                        .number_of_lines(2u32),
                ),
            )
            .child(
                view()
                    .cursor(Cursor::Pointer)
                    .on_press(|| app_signal().update(|s| s.toast = None))
                    .child(icons::icon(ICO_CLOSE, 10.0, p.text_muted)),
            )
            .into_element()
    })
}

fn main_editor_content() -> impl IntoElement {
    let tab_memo = computed(|| app_signal().with(|s| s.editor_tab));
    dynamic(move || match tab_memo.get() {
        EditorTabKind::File => code_editor::code_editor_pane().into_element(),
        EditorTabKind::WorkingDiff => diff_view::diff_pane(false).into_element(),
        EditorTabKind::CommitDiff => diff_view::diff_pane(true).into_element(),
        EditorTabKind::Welcome => editor_area::welcome_view().into_element(),
        EditorTabKind::Comparison => comparison::pane().into_element(),
    })
}

/// 将 Nemorous 主题（明亮 / 暗色 / Darcula / 跟随系统）同步到 NSH 原生 Appearance，
/// 确保 `Editor` 语法着色、活动行、行号栏与窗口外观和当前色板保持严格一致。
fn setup_theme_appearance_sync() {
    effect(move || {
        let name = state::theme_signal().with(|t| t.name.clone());
        let appearance = match name.as_str() {
            "dark" | "darcula" => nsh_core::platform::Appearance::Dark,
            "system" => nsh_core::platform::Appearance::System,
            _ => nsh_core::platform::Appearance::Light,
        };
        nsh_core::element::set_color_scheme(appearance);
    });
}

pub fn shell() -> impl IntoElement {
    state::register_root_owner();
    setup_theme_appearance_sync();
    setup_hidden_title_bar();
    setup_application_menu();
    setup_shortcuts();
    setup_window_focus_refresh();
    crate::session_runtime::observe();
    init_workspace();
    let left_tab_memo = computed(|| app_signal().with(|s| s.left_tab));
    let show_git_memo = computed(|| app_signal().with(|s| s.show_git_panel));
    state::setup_project_follow();

    view()
        .test_id("nemorous-root")
        .on_key_down(key_gestures::key_down)
        .on_key_up(key_gestures::key_up)
        .relative()
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .bg(move || current_palette().app_bg)
        .child(top_header_bar())
        .child(
            view()
                .flex_1()
                .flex_row()
                .overflow_hidden()
                .child(left_rail())
                .child(
                    view()
                        .flex_1()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            h_resizable()
                                .flex_1()
                                .child(
                                    resizable_panel()
                                        .visible(move || app_signal().with(|s| s.show_left_panel))
                                        .size(|| px(app_signal().with(|s| match s.left_tab {
                                            LeftTab::Commit => s.cfg.commit_left_width,
                                            LeftTab::Project => s.cfg.code_left_width,
                                        })))
                                        .size_range(px(90.0)..px(640.0))
                                        .on_resize(|sz: &Points| {
                                            app_signal().update(|s| match s.left_tab {
                                                LeftTab::Commit => s.cfg.commit_left_width = sz.0,
                                                LeftTab::Project => s.cfg.code_left_width = sz.0,
                                            });
                                        })
                                        .child(dynamic(move || match left_tab_memo.get() {
                                            LeftTab::Commit => {
                                                commit_pane::commit_pane().into_element()
                                            }
                                            LeftTab::Project => {
                                                project_pane::project_pane().into_element()
                                            }
                                        })),
                                )
                                .child(
                                    resizable_panel().child(
                                        view()
                                            .flex_1()
                                            .flex_col()
                                            .overflow_hidden()
                                            .child(editor_area::editor_tab_bar())
                                            .child(main_editor_content()),
                                    ),
                                ),
                        )
                        .child(when(move || show_git_memo.get(), git_pane::git_pane)),
                ),
        )
        .child(content_search::find_panel())
        .child(status_bar())
        .child(toast_layer())
        .child(content_search::modal_layer())
        .child(editor_tools::modal_layer())
        .child(bookmarks::modal_layer())
        .child(navigation_picker::modal_layer())
        .child(revision_picker::modal_layer())
        .child(modals::modals_layer())
}
