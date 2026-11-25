mod blame_view;
mod diff_blame;
pub(crate) mod branch_matrix;
pub mod bookmarks;
pub mod code_editor;
pub mod commit_pane;
mod commit_form;
pub(crate) mod conflict_workbench;
mod debug_panel;
pub(crate) mod github_panel;
mod mani_panel;
mod language_tools;
pub(crate) mod settings;
mod update_view;
pub(crate) mod run_panel;
mod terminal_panel;
pub(crate) mod workbench;
pub(crate) mod credentials_view;
pub mod content_search;
pub(crate) mod comparison;
mod replacement_flow;
pub(crate) mod key_gestures;
mod local_history_view;
pub(crate) mod keymap_settings;
pub(crate) mod navigation_picker;
mod split_code_view;
pub(crate) mod revision_picker;
pub mod diff_view;
mod editor_find;
pub mod editor_area;
pub mod editor_tools;
mod git_forest;
pub(crate) use git_forest::repository_history_changed;
mod git_graph;
pub mod git_pane;
pub mod icons;
pub mod modals;
pub(crate) mod git_prompt;
pub(crate) mod git_advanced;
pub(crate) mod git_review;
pub(crate) mod index_review;
pub(crate) mod merge_menu;
pub(crate) mod rebase_plan_view;
pub mod project_pane;
mod workspace_menu;
mod workspace_windows;
mod project_menus;
mod left_tool_window;
mod font_zoom;
mod controls;
pub(crate) mod sync_dialog;

use nsh_base::{EditorCommand, h_resizable, resizable_panel, v_resizable};
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
    open_commit_diff, open_find_bar, open_goto_line_modal,
    open_working_diff, refresh_workspace_async, stage_selected_change,
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
    crate::history_runtime::is_open()
        || app_signal().with_untracked(|s| s.focus_zone == FocusZone::Modal)
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

fn document_undo_focus(s: &state::AppState) -> Option<FocusZone> {
    match (s.editor_tab, s.focus_zone) {
        (EditorTabKind::File, FocusZone::CodeEditor) => Some(FocusZone::CodeEditor),
        (EditorTabKind::WorkingDiff, zone)
            if (zone == FocusZone::WorkingDiffNew
                || (zone == FocusZone::WorkingDiffOld && s.cfg.diff_single_column))
                && s.working_diff.as_ref().is_some_and(|diff| diff.editor_editable()) => Some(zone),
        _ => None,
    }
}

fn undo_document(redo: bool) {
    if modal_owns_focus() || state::text_input_focus_signal().with_untracked(|focus| focus.focused) {
        return;
    }
    match app_signal().with_untracked(document_undo_focus) {
        Some(FocusZone::CodeEditor) => crate::code_split_runtime::undo(redo),
        Some(zone) => {
            if let Some(editor) = state::active_editor_api(zone) {
                editor.run(if redo { EditorCommand::Redo } else { EditorCommand::Undo });
            }
        }
        None => {}
    }
}

fn configured_keymap() -> crate::keymap::Keymap {
    app_signal().with(|s| crate::keymap::Keymap::from_config(&s.cfg, apple_keys(), os()))
}

fn keymap_memo() -> Memo<crate::keymap::Keymap> {
    let profile = computed(|| app_signal().with(|s| {
        (s.cfg.keymap_overrides.clone(), s.cfg.extra.get(crate::keymap::LEGACY).cloned())
    }));
    computed(move || profile.with(|(overrides, legacy)| {
        crate::keymap::Keymap::from_values(overrides, legacy.as_ref(), apple_keys(), os())
    }))
}

fn bind_action(
    keymap: &crate::keymap::Keymap,
    action: &str,
    options: KeyOptions,
    handler: impl Fn(&str) + 'static,
) {
    let handler = std::rc::Rc::new(handler);
    for key in keymap.keys(action).iter().filter(|key| key.as_str() != crate::keymap::DOUBLE_SHIFT) {
        #[cfg(target_os = "macos")]
        if action == "switcher" && matches!(key.as_str(), "ctrl+tab" | "ctrl+shift+tab")
            && crate::macos_gestures::is_installed()
        {
            continue;
        }
        let handler = handler.clone();
        bind_application_key(key, options, move |key| handler(key));
    }
}

fn bind_application_key(key: &str, options: KeyOptions, handler: impl Fn(&str) + 'static) {
    on_key_with_priority(key, options, 1, handler);
}

fn menu_binding(item: MenuItem, keymap: &crate::keymap::Keymap, action: &str) -> MenuItem {
    if let Some(key) = keymap.keys(action).first() {
        item.shortcut(key)
    } else {
        item
    }
}

fn build_application_menu(
    text_input_focused: bool, modal_open: bool, document_undo: bool,
    keymap: &crate::keymap::Keymap, project: &project_menus::Context,
) -> Vec<MenuItem> {
    let copy_item = if text_input_focused {
        // 焦点位于原生 <input>（筛选框、查找/替换框、转到行/文件框）时，使用标准 responder copy:，绝不劫持输入框 ⌘C
        MenuItem::role(MenuRole::Copy)
    } else {
        // 焦点位于 Diff 左右侧、源码 Editor 或 Commit/Project/Git 树时，允许点击顶部原生「编辑 > 复制」直达焦点路由复制
        menu_binding(MenuItem::new("复制"), keymap, "copy")
            .on_select(copy_for_current_focus)
    };
    vec![
        MenuItem::submenu(
            "Nemorous",
            [
                MenuItem::role(MenuRole::About),
                menu_binding(MenuItem::new("设置…"), keymap, "settings")
                    .on_select(guard_app_command(settings::open)).disabled(modal_open),
                MenuItem::new("快捷键设置…")
                    .on_select(guard_app_command(keymap_settings::open)).disabled(modal_open),
                MenuItem::separator(),
                MenuItem::role(MenuRole::Hide),
                MenuItem::role(MenuRole::HideOthers),
                MenuItem::role(MenuRole::ShowAll),
                MenuItem::separator(),
                MenuItem::role(MenuRole::Quit),
            ],
        ),
        project_menus::file(project, modal_open, keymap),
        MenuItem::submenu(
            "编辑",
            [
                if document_undo {
                    menu_binding(MenuItem::new("撤销"), keymap, "undo")
                        .on_select(|| undo_document(false))
                } else { MenuItem::role(MenuRole::Undo) },
                if document_undo {
                    menu_binding(MenuItem::new("重做"), keymap, "redo")
                        .on_select(|| undo_document(true))
                } else { MenuItem::role(MenuRole::Redo) },
                MenuItem::separator(),
                MenuItem::role(MenuRole::Cut),
                copy_item,
                MenuItem::role(MenuRole::Paste),
                MenuItem::role(MenuRole::SelectAll),
            ],
        ),
        MenuItem::submenu(
            "导航",
            [
                menu_binding(MenuItem::new("转到文件…"), keymap, "goToFile")
                    .on_select(guard_app_command(|| {
                        app_signal().update(|s| {
                            s.picker_open = Some(PickerKind::GoToFile);
                            s.picker_query.clear();
                            state::enter_modal_focus(s);
                        });
                    }))
                    .disabled(modal_open),
                menu_binding(MenuItem::new("最近文件…"), keymap, "recentFiles")
                    .on_select(guard_app_command(|| navigation_picker::open(navigation_picker::Mode::RecentFiles)))
                    .disabled(modal_open),
                menu_binding(MenuItem::new("查找操作…"), keymap, "actions")
                    .on_select(guard_app_command(|| navigation_picker::open(navigation_picker::Mode::Actions)))
                    .disabled(modal_open),
                menu_binding(MenuItem::new("最近位置…"), keymap, "recentLocations")
                    .on_select(guard_app_command(|| {
                        app_signal().update(|s| {
                            s.picker_open = Some(PickerKind::RecentLocations);
                            s.picker_query.clear();
                            state::enter_modal_focus(s);
                        });
                    }))
                    .disabled(modal_open),
                menu_binding(MenuItem::new("转到行:列…"), keymap, "goToLine")
                    .on_select(guard_app_command(open_goto_line_modal))
                    .disabled(modal_open),
                MenuItem::separator(),
                menu_binding(MenuItem::new("后退"), keymap, "back")
                    .on_select(guard_app_command(navigate_back))
                    .disabled(modal_open),
                menu_binding(MenuItem::new("前进"), keymap, "forward")
                    .on_select(guard_app_command(navigate_forward))
                    .disabled(modal_open),
                menu_binding(MenuItem::new("在文件树中定位"), keymap, "selectIn")
                    .on_select(guard_app_command(locate_current_in_project_tree))
                    .disabled(modal_open),
            ],
        ),
        project_menus::git(project, modal_open, keymap),
        MenuItem::submenu(
            "窗口",
            [
                MenuItem::role(MenuRole::Minimize),
                MenuItem::new("向右拆分")
                    .on_select(guard_app_command(crate::code_split_runtime::split_active))
                    .disabled(modal_open),
                MenuItem::new("取消所有拆分")
                    .on_select(guard_app_command(|| {
                        if app_signal().with_untracked(|s| s.code_split.is_some()) {
                            crate::code_split_runtime::merge();
                        }
                    })).disabled(modal_open),
                menu_binding(MenuItem::new("关闭标签"), keymap, "closeTab")
                    .on_select(guard_app_command(close_active_tab))
                    .disabled(modal_open),
            ],
        ),
    ]
}

fn setup_application_menu() {
    let keymap = keymap_memo();
    let mode = computed(move || {
        let input_focused = state::text_input_focus_signal().with(|f| f.focused);
        app_signal().with(|s| (
            input_focused,
            s.focus_zone == FocusZone::Modal,
            !input_focused && document_undo_focus(s).is_some(),
            keymap.get(),
            project_menus::Context::from_state(s),
        ))
    });
    // The menu handlers belong to this effect. Every rerun must reinstall
    // them, even though unrelated app changes leave the memo unchanged.
    effect(move || {
        mode.with(|(input_focused, modal_open, document_undo, keys, context)| {
            set_application_menu(build_application_menu(*input_focused, *modal_open, *document_undo, keys, context));
        });
    });
}

fn tool_window_focus(tab: LeftTab) -> FocusZone {
    match tab {
        LeftTab::Commit => FocusZone::CommitTree,
        LeftTab::Project => FocusZone::ProjectTree,
    }
}

fn tree_filter_has_focus(s: &state::AppState) -> bool {
    s.show_left_panel
        && s.tree_filter_open == Some(s.left_tab)
        && s.focus_zone == tool_window_focus(s.left_tab)
}

fn hide_tool_window(tab: LeftTab) {
    let refocus = app_signal().with_untracked(|s| s.focus_zone == tool_window_focus(tab));
    app_signal().update(|s| {
        s.show_left_panel = false;
        s.dismiss_tree_filter();
        if refocus {
            s.focus_zone = state::editor_focus_zone(s);
        }
    });
    if refocus {
        state::set_text_input_focused(false);
        state::focus_active_editor_ref();
    }
}

fn focus_tool_window(tab: LeftTab, toggle: bool) {
    let focus = tool_window_focus(tab);
    if toggle && app_signal().with_untracked(|s| {
        s.show_left_panel && s.left_tab == tab && s.focus_zone == focus
    }) {
        hide_tool_window(tab);
        return;
    }
    state::set_text_input_focused(false);
    app_signal().update(|s| {
        s.show_left_panel = true;
        if s.left_tab != tab {
            s.dismiss_tree_filter();
        }
        s.left_tab = tab;
        s.focus_zone = focus;
        if tab == LeftTab::Commit && s.change_selection.is_none() && s.change_selected_dir.is_none() {
            s.change_selection = state::visible_change_items(s).into_iter().next();
        }
    });
}

fn toggle_rail_tool_window(tab: LeftTab) {
    if app_signal().with_untracked(|s| s.show_left_panel && s.left_tab == tab) {
        hide_tool_window(tab);
    } else {
        focus_tool_window(tab, false);
    }
}

/// 绑定全局和各焦点区域的快捷键。
fn setup_shortcuts() {
    let always = KeyOptions {
        when: When::Always,
        ..Default::default()
    };
    let not_editing = KeyOptions {
        when: When::NotEditing,
        ..Default::default()
    };

    let keymap_memo = keymap_memo();
    let zone_memo = computed(|| app_signal().with(|s| s.focus_zone));
    let modal_focus = computed(move || {
        zone_memo.get() == FocusZone::Modal || crate::history_runtime::is_open()
    });
    let text_input_focused = computed(|| state::text_input_focus_signal().with(|f| f.focused));
    effect(move || {
        let zone = zone_memo.get();
        if matches!(zone, FocusZone::ProjectTree | FocusZone::CommitTree | FocusZone::GitFiles) {
            after_answer(move || {
                let editor = app_signal().with_untracked(|s| {
                    (s.focus_zone == zone).then(|| state::editor_focus_zone(s))
                });
                if let Some(api) = editor.and_then(state::active_editor_api) {
                    api.blur();
                }
            });
        }
    });
    let tab_memo = computed(|| app_signal().with(|s| s.editor_tab));
    let workspace_search_open = computed(content_search::is_open);
    let multiline_search = computed(content_search::query_is_multiline);
    let navigation_open = computed(navigation_picker::is_open);
    let revision_open = computed(revision_picker::is_open);
    let switcher_open = computed(navigation_picker::is_switcher);
    let index_review_open = computed(index_review::is_open);
    let conflict_workbench_open = computed(conflict_workbench::is_open);
    let commit_message_focused = computed(commit_form::has_focus);
    let lsp_enabled = computed(language_tools::enabled);
    let dismissable_memo = computed(move || {
        if modal_focus.get() {
            return true;
        }
        app_signal().with(|s| {
            s.save_conflict.is_some()
                || s.dirty_close.is_some()
                || s.goto_line_open
                || s.picker_open.is_some()
                || s.stash_modal_open
                || tree_filter_has_focus(s)
                || s.active_find_open()
        })
    });

    // Keep workspace bindings mounted as focus moves between editors and tool windows.
    effect(move || {
        if modal_focus.get() {
            return;
        }
        let keys = keymap_memo.get();
        bind_action(&keys, "recentFiles", always, |_| navigation_picker::open(navigation_picker::Mode::RecentFiles));
        bind_action(&keys, "actions", always, |_| navigation_picker::open(navigation_picker::Mode::Actions));
        bind_action(&keys, "switcher", always, |key| navigation_picker::start_or_move_switcher(key.contains("shift+")));
        bind_action(&keys, "save", always, |_| {
            crate::format_runtime::save_all();
        });
        bind_action(&keys, "settings", always, |_| settings::open());
        bind_action(&keys, "zoomIn", always, |_| settings::zoom(0.1));
        bind_action(&keys, "zoomOut", always, |_| settings::zoom(-0.1));
        bind_action(&keys, "zoomReset", always, |_| settings::zoom(0.0));
        bind_action(&keys, "gitCommit", not_editing, |_| commit_form::focus());
        bind_action(&keys, "pull", not_editing, |_| sync_dialog::start_all(crate::git_remotes::RemoteAction::Pull));
        bind_action(&keys, "push", not_editing, |_| project_menus::push_all());
        bind_action(&keys, "searchEverywhere", always, |_| navigation_picker::open(navigation_picker::Mode::Everywhere));
        bind_action(&keys, "previousTab", always, |_| navigation_picker::cycle_tabs(-1));
        bind_action(&keys, "nextTab", always, |_| navigation_picker::cycle_tabs(1));
        bind_action(&keys, "closeTab", always, |_| {
            close_active_tab();
        });
        bind_action(&keys, "goToLine", always, |_| {
            open_goto_line_modal();
        });
        bind_action(&keys, "back", always, |_| {
            navigate_back();
        });
        bind_action(&keys, "forward", always, |_| {
            navigate_forward();
        });
        bind_action(&keys, "findInFiles", always, |_| {
            content_search::open_workspace();
        });
        bind_action(&keys, "replaceInFiles", always, |_| {
            content_search::open_replace();
        });
        bind_action(&keys, "structure", always, |_| {
            editor_tools::open_structure();
        });
        bind_action(&keys, "todos", always, |_| {
            editor_tools::open_todos();
        });
        bind_action(&keys, "format", always, |_| {
            crate::format_runtime::format_active();
        });
        bind_action(&keys, "bookmark", always, |_| {
            bookmarks::toggle_current_line();
        });
        bind_action(&keys, "find", always, |_| {
            let tree = app_signal().with_untracked(|s| {
                (s.show_left_panel && s.focus_zone == tool_window_focus(s.left_tab))
                    .then_some(s.left_tab)
            });
            if let Some(tab) = tree {
                left_tool_window::open_tree_filter(tab);
            } else {
                open_find_bar(false);
            }
        });
        bind_action(&keys, "replace", always, |_| open_find_bar(true));
        bind_action(&keys, "findNext", always, |_| {
            state::run_active_find_nav(true);
        });
        bind_action(&keys, "findPrevious", always, |_| {
            state::run_active_find_nav(false);
        });
        bind_action(&keys, "goToFile", always, |_| {
            app_signal().update(|s| {
                s.picker_open = Some(PickerKind::GoToFile);
                s.picker_query.clear();
                state::enter_modal_focus(s);
            });
        });
        bind_action(&keys, "recentLocations", always, |_| {
            app_signal().update(|s| {
                s.picker_open = Some(PickerKind::RecentLocations);
                s.picker_query.clear();
                state::enter_modal_focus(s);
            });
        });
        bind_action(&keys, "refresh", always, |_| {
            refresh_workspace_async(None);
        });
        bind_action(&keys, "copyAbsolute", always, |_| {
            copy_absolute_path_current();
        });
        bind_action(&keys, "copyReference", always, |_| {
            copy_reference_current();
        });
        bind_action(&keys, "selectIn", always, |_| {
            locate_current_in_project_tree();
        });
        for key in keys.keys("jumpToSource").iter().filter(|key| *key != "cmd+down") {
            bind_application_key(key, always, |_| jump_from_diff_to_source());
        }
        bind_action(&keys, "commitWindow", always, |_| focus_tool_window(LeftTab::Commit, true));
        bind_action(&keys, "project", always, |_| focus_tool_window(LeftTab::Project, true));
        bind_action(&keys, "maniWindow", always, |_| mani_panel::toggle());
        bind_action(&keys, "terminalWindow", always, |_| terminal_panel::toggle());
        bind_action(&keys, "gitWindow", always, |_| {
            workbench::hide();
            content_search::hide_find_panel();
            app_signal().update(|s| {
                s.show_git_panel = !s.show_git_panel;
            });
        });

        // 树焦点快捷键（Commit 树 / Project 树 / Git 提交文件树）
        bind_action(&keys, "compareOrDiff", always, |_| {
            let current_zone = app_signal().with_untracked(|s| s.focus_zone);
            match current_zone {
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
    });

    effect(move || {
        if !modal_focus.get() {
            return;
        }
        let keys = keymap_memo.get();
        if index_review_open.get() {
            bind_action(&keys, "save", always, |_| index_review::save_active());
        }
        if conflict_workbench_open.get() {
            use crate::conflict_text::ConflictChoice;
            bind_action(&keys, "save", always, |_| conflict_workbench::save());
            bind_application_key("alt+left", always, |_| conflict_workbench::move_hunk(-1));
            bind_application_key("alt+right", always, |_| conflict_workbench::move_hunk(1));
            bind_application_key("alt+1", always, |_| conflict_workbench::choose(ConflictChoice::Ours));
            bind_application_key("alt+2", always, |_| conflict_workbench::choose(ConflictChoice::Theirs));
            bind_application_key("alt+3", always, |_| conflict_workbench::choose(ConflictChoice::BothOursFirst));
            bind_application_key("mod+enter", always, |_| conflict_workbench::continue_if_ready());
        }
        if revision_open.get() {
            bind_application_key("up", always, |_| revision_picker::move_selection(-1));
            bind_application_key("down", always, |_| revision_picker::move_selection(1));
            bind_application_key("enter", always, |_| revision_picker::accept());
        }
        if navigation_open.get() {
            bind_application_key("up", always, |_| navigation_picker::move_selection(-1));
            bind_application_key("down", always, |_| navigation_picker::move_selection(1));
            if switcher_open.get() {
                bind_application_key("enter", always, |_| navigation_picker::accept());
                bind_action(&keys, "switcher", always, |key| navigation_picker::start_or_move_switcher(key.contains("shift+")));
            }
        }
        if workspace_search_open.get() {
            if multiline_search.get() {
                bind_application_key("mod+enter", always, |_| content_search::activate_selected());
            } else if !content_search::preview_has_focus() {
                bind_application_key("up", always, |_| content_search::move_selection(-1));
                bind_application_key("down", always, |_| content_search::move_selection(1));
            }
        }
    });

    let workspace_non_text_focus = computed(move || !modal_focus.get() && !text_input_focused.get());
    effect(move || {
        if workspace_non_text_focus.get() {
            let keys = keymap_memo.get();
            bind_action(&keys, "branchMatrix", always, |_| branch_matrix::open());
            bind_action(&keys, "rollback", always, |_| git_review::rollback_current());
        }
    });
    let commit_submit = computed(move || !modal_focus.get() && commit_message_focused.get());
    effect(move || {
        if commit_submit.get() {
            bind_application_key("mod+enter", always, |_| commit_form::submit());
        }
    });

    let source_focus = computed(move || zone_memo.get() == FocusZone::CodeEditor);
    effect(move || {
        if source_focus.get() {
            bind_action(&keymap_memo.get(), "breakpoint", always, |_| debug_panel::active_line_breakpoint());
        }
    });
    let source_editing_enabled = computed(move || source_focus.get() && !text_input_focused.get());
    let undo_enabled = computed(move || {
        !modal_focus.get() && !text_input_focused.get()
            && app_signal().with(|s| document_undo_focus(s).is_some())
    });
    effect(move || {
        if undo_enabled.get() {
            let keys = keymap_memo.get();
            let options = KeyOptions { repeat: true, for_focus: true, ..always };
            bind_action(&keys, "undo", options, |_| undo_document(false));
            bind_action(&keys, "redo", options, |_| undo_document(true));
        }
    });
    effect(move || {
        if !source_editing_enabled.get() {
            return;
        }
        let keys = keymap_memo.get();
        bind_action(&keys, "nextOccurrence", always, |_| {
            if let Some(editor) = state::active_editor_api(FocusZone::CodeEditor) {
                editor.run(nsh_base::EditorCommand::SelectNextOccurrence);
            }
        });
        if tab_memo.get() == EditorTabKind::File {
            if lsp_enabled.get() {
                bind_action(&keys, "findReferences", always, |_| language_tools::find_references());
                bind_action(&keys, "quickFixes", always, |_| language_tools::quick_fixes());
            }
            let repeat = KeyOptions {
                repeat: true,
                for_focus: true,
                ..always
            };
            for (key, command) in [
                ("alt+left", EditorCommand::MoveWordLeft),
                ("alt+right", EditorCommand::MoveWordRight),
            ] {
                bind_application_key(key, repeat, move |_| {
                    if let Some(api) = state::active_editor_api(FocusZone::CodeEditor) {
                        api.run(command);
                    }
                });
            }
        }
    });

    let diff_focus = computed(move || matches!(
        zone_memo.get(),
        FocusZone::WorkingDiffOld | FocusZone::WorkingDiffNew | FocusZone::CommitDiffOld | FocusZone::CommitDiffNew
    ));
    effect(move || {
        if diff_focus.get() && keymap_memo.get().keys("jumpToSource").iter().any(|key| key == "cmd+down") {
            bind_application_key("cmd+down", always, |_| jump_from_diff_to_source());
        }
    });

    // Option-arrow follows the active Diff. It continues moving by words in a text input.
    let diff_navigation_enabled = computed(move || {
        workspace_non_text_focus.get()
            && !source_focus.get()
            && matches!(tab_memo.get(), EditorTabKind::WorkingDiff | EditorTabKind::CommitDiff)
    });
    effect(move || {
        if !diff_navigation_enabled.get() {
            return;
        }
        let keys = keymap_memo.get();
        bind_action(&keys, "nextDifference", always, |_| {
            jump_diff_hunk(1);
        });
        bind_action(&keys, "previousDifference", always, |_| {
            jump_diff_hunk(-1);
        });
        bind_action(&keys, "previousDiffFile", always, |_| {
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
        bind_action(&keys, "nextDiffFile", always, |_| {
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
    });

    effect(move || {
        if zone_memo.get() == FocusZone::FindResults
            && content_search::find_panel_visible()
            && !content_search::preview_has_focus()
        {
            let repeat = KeyOptions { repeat: true, ..not_editing };
            bind_application_key("up", repeat, |_| content_search::move_find_selection(-1));
            bind_application_key("down", repeat, |_| content_search::move_find_selection(1));
            bind_application_key("enter", not_editing, |_| content_search::activate_find_selection());
        }
    });

    let tree_focus = computed(move || match zone_memo.get() {
        zone @ (FocusZone::CommitTree | FocusZone::ProjectTree | FocusZone::GitFiles) => Some(zone),
        _ => None,
    });
    effect(move || {
        let Some(zone) = tree_focus.get() else {
            return;
        };
        let keys = keymap_memo.get();
        if zone == FocusZone::GitFiles && !text_input_focused.get() {
            let repeat = KeyOptions { repeat: true, ..not_editing };
            bind_application_key("up", repeat, |_| git_pane::move_selection(-1));
            bind_application_key("down", repeat, |_| git_pane::move_selection(1));
        }
        bind_action(&keys, "copy", not_editing, |_| {
            copy_for_current_focus();
        });
        bind_application_key("enter", not_editing, move |_| match zone {
            FocusZone::CommitTree => {
                if let Some(sel) = app_signal().with_untracked(|s| s.change_selection.clone()) {
                    open_working_diff(sel.file, sel.staged, true);
                }
            }
            FocusZone::ProjectTree => {
                let selected_file = app_signal().with_untracked(|s| s.project_selected_file.clone());
                if let Some(wf) = selected_file {
                    state::open_code_file(wf, None, true);
                } else {
                    app_signal().update(|s| {
                        if let Some((_, rel)) = s.project_selected_dir.clone() {
                            if rel.is_empty() {
                                s.project_workspace_expanded = !s.project_workspace_expanded;
                            } else if !s.expanded_project_dirs.remove(&rel) {
                                s.expanded_project_dirs.insert(rel);
                            }
                        }
                    });
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

        if zone == FocusZone::CommitTree {
            bind_application_key("up", not_editing, |_| {
                move_change_selection(-1, false);
            });
            bind_application_key("down", not_editing, |_| {
                move_change_selection(1, false);
            });
            bind_application_key("space", not_editing, |_| {
                toggle_selected_change_checkbox();
            });
            bind_application_key("s", not_editing, |_| {
                stage_selected_change(true);
            });
            bind_application_key("u", not_editing, |_| {
                stage_selected_change(false);
            });
        }

        if zone == FocusZone::ProjectTree {
            bind_application_key("up", not_editing, |_| {
                state::move_project_selection(-1);
            });
            bind_application_key("down", not_editing, |_| {
                state::move_project_selection(1);
            });
            bind_application_key("right", not_editing, |_| {
                state::toggle_project_selected_dir(true);
            });
            bind_application_key("left", not_editing, |_| {
                state::toggle_project_selected_dir(false);
            });
            bind_application_key("mod+x", not_editing, |_| project_pane::cut_selected());
            bind_application_key("mod+v", not_editing, |_| project_pane::paste_selected());
            bind_application_key("shift+f6", not_editing, |_| project_pane::rename_selected());
            bind_application_key("backspace", not_editing, |_| project_pane::delete_selected());
        }
    });

    effect(move || {
        if dismissable_memo.get() {
            bind_application_key("escape", always, |_| {
                if app_signal().with_untracked(tree_filter_has_focus) {
                    app_signal().update(state::AppState::dismiss_tree_filter);
                    state::set_text_input_focused(false);
                } else {
                    state::close_active_modal();
                }
            });
        }
    });
}

/// 监听窗口切回前台与系统生命周期恢复，自动刷新工作区。
fn setup_window_focus_refresh() {
    let was_focused = stored(true);
    let active = computed(|| {
        let focused = window_state().map(|w| w.focused).unwrap_or(true);
        focused && lifecycle() == Lifecycle::Active
    });
    effect(move || {
        let active = active.get();
        let prev = was_focused.replace(active);
        if active && !prev {
            state::refresh_workspace_on_focus();
        }
    });
}

fn top_header_bar() -> impl IntoElement {
    let workspace = computed(|| app_signal().with(|s| {
        if s.cfg.workspace_path.is_empty() { "选择项目".to_owned() } else { basename(&s.cfg.workspace_path) }
    }));
    let branch = computed(|| app_signal().with(|s| s.statuses.first()
        .map(|status| status.branch.clone()).filter(|branch| !branch.is_empty())));
    let refreshing = computed(|| app_signal().with(|s| s.is_refreshing()));
    dynamic(move || {
        let p = current_palette();
        let bar = title_bar();
        let left_pad = bar.left.0.max(76.0);

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
                    .child(workspace_menu::toolbar(
                        controls::button()
                            .h(px(28.0))
                            .px(px(10.0))
                            .gap(px(6.0))
                            .rounded(px(6.0))
                            .bg(p.surface2)
                            .test_id("top-workspace-button")
                            .label("切换项目")
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .expanded(move |s| s.bg(p.accent_soft))
                            .child(icons::icon(ICO_FOLDER, 13.0, p.text_control))
                            .child(
                                text(move || workspace.get())
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary)
                                    .number_of_lines(1u32),
                            )
                            .child(icons::icon(ICO_CHEVRON_DOWN, 10.0, p.text_muted)),
                        p,
                    ))
                    .child(dynamic(move || branch.get().map(|branch| {
                        controls::button()
                            .h(px(28.0))
                            .px(px(10.0))
                            .gap(px(6.0))
                            .rounded(px(6.0))
                            .bg(p.surface2)
                            .test_id("top-branch-button")
                            .label("分支矩阵")
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(branch_matrix::open)
                            .child(icons::icon(ICO_BRANCH, 13.0, p.accent))
                            .child(
                                text(branch)
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.text_primary)
                                    .number_of_lines(1u32),
                            )
                    }))),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .window_drag(false)
                    .child(
                        controls::button()
                            .test_id("top-git-advanced-btn")
                            .label("Git 高级操作")
                            .px(px(6.0)).h(px(26.0)).rounded(px(5.0)).cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(git_advanced::open)
                            .child(icons::icon(ICO_SETTINGS, 13.0, p.text_control)),
                    )
                    .child(
                        controls::button()
                            .test_id("top-git-sync-btn")
                            .label("获取、拉取或推送")
                            .px(px(6.0)).h(px(26.0)).rounded(px(5.0)).cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(sync_dialog::open_menu)
                            .child(text("同步").text_size(px(11.0)).text_color(p.text_control)),
                    )
                    .child(
                        controls::button()
                            .test_id("top-refresh-btn")
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(5.0))
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .label("刷新工作区 (⌘⌥Y)")
                            .on_press(|| refresh_workspace_async(None))
                            .child(dynamic(move || icons::icon(
                                ICO_REFRESH,
                                13.0,
                                if refreshing.get() { p.accent } else { p.text_control },
                            ))),
                    )
                    .child(
                        controls::button()
                            .test_id("top-search-btn")
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(5.0))
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

        let captions = app_signal().with(|s| s.cfg.extra_bool("nav_show_labels",false));
        let rail_btn = |tid: &'static str,
                        label: &'static str,
                        svg_src: &'static str,
                        active: bool,
                        on_click: fn()| {
            view()
                .test_id(tid)
                .w(px(if captions { 58.0 } else { 34.0 }))
                .h(px(if captions { 42.0 } else { 34.0 }))
                .flex_col()
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
                .child(captions.then(|| text(label.split(" (").next().unwrap_or(label))
                    .text_color(p.text_muted).text_size(px(8.5)).number_of_lines(1u32)))
        };

        let theme_icon = match theme_name.as_str() {
            "dark" | "darcula" => ICO_MOON,
            "system" => ICO_MONITOR,
            _ => ICO_SUN,
        };

        view()
            .test_id("left-rail")
            .w(px(if captions { 68.0 } else { 46.0 }))
            .flex_none()
            .flex_col()
            .items_center()
            .py(px(8.0))
            .gap(px(6.0))
            .bg(p.rail_bg)
            .border_r_1()
            .border_color(p.border)
            .child(rail_btn(
                "rail-project-btn",
                "Project (⌘1)",
                ICO_FOLDER,
                show_left && left_tab == LeftTab::Project,
                || toggle_rail_tool_window(LeftTab::Project),
            ))
            .child(rail_btn(
                "rail-commit-btn",
                "Commit (⌘0)",
                ICO_COMMIT,
                show_left && left_tab == LeftTab::Commit,
                || toggle_rail_tool_window(LeftTab::Commit),
            ))
            .child(rail_btn(
                "rail-git-btn",
                "Git (⌘9)",
                ICO_BRANCH,
                show_git,
                || {
                    workbench::hide();
                    content_search::hide_find_panel();
                    app_signal().update(|s| s.show_git_panel = !s.show_git_panel);
                },
            ))
            .child(view().flex_1())
            .child(rail_btn("rail-run-btn", "运行 / 调试 / 测试", ICO_PLAY,
                workbench::active() == Some(workbench::Panel::Run), || workbench::toggle(workbench::Panel::Run)))
            .child(rail_btn("rail-terminal-btn", "终端", ICO_TERMINAL,
                workbench::active() == Some(workbench::Panel::Terminal), terminal_panel::toggle))
            .child(mani_panel::enabled().then(|| rail_btn("rail-mani-btn", "Mani", ICO_LIST,
                workbench::active() == Some(workbench::Panel::Mani), mani_panel::toggle)))
            .child(rail_btn("rail-logs-btn", "日志", ICO_LOGS,
                workbench::active() == Some(workbench::Panel::Logs), || workbench::toggle(workbench::Panel::Logs)))
            .child(content_search::find_panel_available().then(|| {
                rail_btn(
                    "rail-find-btn",
                    "查找结果",
                    ICO_SEARCH,
                    content_search::find_panel_visible(),
                    content_search::toggle_find_panel,
                )
            }))
            .child(rail_btn("rail-settings-btn","设置",ICO_SETTINGS,false,settings::open))
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

fn reveal_selected_row(row: ElementRef) {
    after_answer(move || {
        row.reveal(nsh_core::RevealOptions::default()
            .vertical(nsh_core::ScrollAlignment::Nearest)
            .animated(false));
    });
}

fn tooltip_label(label: String, p: theme::Palette) -> nsh_core::element::View {
    view()
        .test_id("toolbar-tooltip")
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(p.border)
        .bg(p.surface)
        .child(text(label).text_size(px(11.0)).text_color(p.text_primary))
}

fn status_bar() -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let loading_diff = state::diff_read_pending();
        let (ws_display, cursor_text) = app_signal().with(|s| {
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
            (ws, cur)
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
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(p.text_muted)
                    .number_of_lines(1u32),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_none()
                    .when(loading_diff, |row| {
                        row.child(text("正在读取审阅内容…").test_id("diff-read-status")
                            .px(px(6.0)).text_size(px(11.0)).text_color(p.text_muted))
                    })
                    .child(cursor_text.map(|pos_txt| {
                        controls::button()
                            .test_id("status-cursor-pos")
                            .px(px(6.0))
                            .py(px(1.5))
                            .rounded(px(4.0))
                            .bg(p.surface2)
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(open_goto_line_modal)
                            .child(text(pos_txt).text_size(px(10.5)).text_color(p.text_control))
                    })),
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
                controls::button()
                    .label("关闭提示")
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

/// 同步原生窗口明暗模式和编辑器语法配色。
fn setup_theme_appearance_sync() {
    let default_tokens = nsh_core::element::untracked(nsh_core::element::tokens);
    let darcula_tokens = theme::darcula_editor_tokens(&default_tokens);
    effect(move || {
        let name = state::theme_signal().with(|t| t.name.clone());
        let appearance = match name.as_str() {
            "dark" | "darcula" => nsh_core::platform::Appearance::Dark,
            "system" => nsh_core::platform::Appearance::System,
            _ => nsh_core::platform::Appearance::Light,
        };
        nsh_core::element::set_color_scheme(appearance);
        let tokens = if name == "darcula" {
            &darcula_tokens
        } else {
            default_tokens.as_ref()
        };
        nsh_core::element::set_tokens(tokens.clone());
    });
}

fn review_editor_styles() -> nsh_base::EditorStyles {
    use nsh_core::{PropertyValue, StyleKey, element::Style};

    let mut styles = nsh_base::EditorStyles::default();
    if !state::theme_signal().with(|theme| theme.name == "darcula") {
        return styles;
    }

    let color = |key, value| {
        Style::default()
            .with(key, PropertyValue::Color(Some(rgb(value).into())))
            .into()
    };
    let foreground = |value| color(StyleKey::Color, value);
    let background = |value| color(StyleKey::BackgroundColor, value);
    styles.content = Some(foreground(0xA9B7C6));
    styles.gutter = Some(background(0x313335));
    styles.line_number = Some(foreground(0x888888));
    styles.active_line_number = Some(foreground(0xBBBBBB));
    styles.active_line = Some(background(0x323232));
    styles.selection = Some(background(0x214283));
    styles.caret = Some(background(0xBBBBBB));
    styles.indent_guide = Some(background(0x373737));
    styles.whitespace = Some(foreground(0x666666));
    styles
}

pub fn shell() -> impl IntoElement {
    state::register_root_owner();
    setup_theme_appearance_sync();
    setup_hidden_title_bar();
    setup_application_menu();
    workspace_windows::observe();
    setup_shortcuts();
    #[cfg(target_os = "macos")]
    key_gestures::observe_macos();
    setup_window_focus_refresh();
    crate::session_runtime::observe();
    init_workspace();
    let left_tab_memo = computed(|| app_signal().with(|s| s.left_tab));
    let show_git_memo = computed(|| app_signal().with(|s| s.show_git_panel));
    state::setup_project_follow();
    git_forest::setup();
    workbench::setup();
    terminal_panel::setup();
    debug_panel::setup();
    mani_panel::setup();
    language_tools::setup();
    update_view::setup();
    #[cfg(target_os="macos")]
    settings::setup_icon();
    let zoom = computed(|| app_signal().with(|state| state.cfg.zoom()));
    use nsh_core::geometry::relative;

    let content = view()
        .test_id("nemorous-root")
        .w(move || relative(1.0 / zoom.get()))
        .h(move || relative(1.0 / zoom.get()))
        .flex_none()
        .scale(move || zoom.get())
        .transform_origin(nsh_core::TransformOrigin { x: relative(0.0), y: relative(0.0), z: px(0.0) })
        .on_key_down(key_gestures::key_down)
        .on_key_up(key_gestures::key_up)
        .relative()
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
                            v_resizable()
                                .flex_1()
                                .child(resizable_panel().child(
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
                                ))
                                .child(resizable_panel()
                                    .visible(|| workbench::active().is_some())
                                    .size(workbench::height)
                                    .size_range(px(130.0)..px(600.0))
                                    .on_resize(workbench::resize)
                                    .child(workbench::layer())),
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
        .child(keymap_settings::modal_layer())
        .child(local_history_view::modal_layer())
        .child(branch_matrix::layer())
        .child(sync_dialog::layer())
        .child(index_review::layer())
        .child(git_advanced::layer())
        .child(credentials_view::layer())
        .child(merge_menu::layer())
        .child(rebase_plan_view::layer())
        .child(conflict_workbench::layer())
        .child(git_prompt::layer())
        .child(run_panel::layer())
        .child(github_panel::layer())
        .child(settings::layer())
        .child(modals::modals_layer());
    view().flex_1().w_full().h_full().overflow_hidden().child(content)
}
