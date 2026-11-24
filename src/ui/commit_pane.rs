use std::collections::{BTreeMap, HashSet};

use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::{ChangedFile, RepoStatus, WorkspaceFile};
use crate::paths::{basename, display_path};
use crate::state::{
    self, FocusZone, LeftTab, WorkspaceData, WorkspaceNotice, app_signal, click_change_row, current_palette, open_code_file,
    open_working_diff, set_group_checked, stage_files_action,
    tri_state_for_keys,
};
use crate::theme::{Palette, repo_badge_colors, status_text_color};
use crate::ui::icons::{self, *};

use super::left_tool_window::{ToolWindow, button, tree_scroll, workspace_notice};

#[derive(Default)]
struct ChangeDirNode {
    name: String,
    rel_path: String,
    subdirs: BTreeMap<String, ChangeDirNode>,
    files: Vec<ChangedFile>,
}

fn build_change_tree(files: &[ChangedFile]) -> ChangeDirNode {
    let mut root = ChangeDirNode::default();
    for f in files {
        let parts: Vec<&str> = f.path.split('/').collect();
        if parts.len() <= 1 {
            root.files.push(f.clone());
            continue;
        }
        let mut cur = &mut root;
        let mut acc = String::new();
        for seg in &parts[..parts.len() - 1] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            cur = cur
                .subdirs
                .entry((*seg).to_string())
                .or_insert_with(|| ChangeDirNode {
                    name: (*seg).to_string(),
                    rel_path: acc.clone(),
                    subdirs: BTreeMap::new(),
                    files: Vec::new(),
                });
        }
        cur.files.push(f.clone());
    }
    sort_tree(&mut root);
    root
}

fn sort_tree(node: &mut ChangeDirNode) {
    node.files.sort_by(|a, b| a.path.cmp(&b.path));
    for sub in node.subdirs.values_mut() {
        sort_tree(sub);
    }
}

fn collect_tree_keys(node: &ChangeDirNode, out: &mut Vec<String>) {
    for sub in node.subdirs.values() {
        collect_tree_keys(sub, out);
    }
    for f in &node.files {
        out.push(f.key());
    }
}

pub fn build_location_menu_items(
    repo_id: String,
    repo_path: String,
    rel_path: String,
    abs_path: String,
    line_1based: u32,
    is_dir: bool,
) -> Vec<MenuItem> {
    let name_label = if is_dir { "目录名" } else { "文件名" };
    let r1 = repo_path.clone();
    let p1 = rel_path.clone();
    let a1 = abs_path.clone();
    let r2 = repo_path.clone();
    let p2 = rel_path.clone();
    let a2 = abs_path.clone();
    let r3 = repo_path.clone();
    let p3 = rel_path.clone();
    let a3 = abs_path.clone();
    let r4 = repo_path.clone();
    let p4 = rel_path.clone();
    let a4 = abs_path.clone();
    let r5 = repo_path.clone();
    let p5 = rel_path.clone();
    let a5 = abs_path.clone();
    let r6 = repo_path;
    let p6 = rel_path.clone();
    let a6 = abs_path.clone();

    let mut copy_sub = vec![
        MenuItem::new(name_label).on_select(move || {
            state::copy_path_variant(&r1, &p1, &a1, line_1based, "name");
        }),
        MenuItem::new("绝对路径").on_select(move || {
            state::copy_path_variant(&r2, &p2, &a2, line_1based, "abs");
        }),
        MenuItem::new("相对仓库路径").on_select(move || {
            state::copy_path_variant(&r3, &p3, &a3, line_1based, "repo_rel");
        }),
        MenuItem::new("相对工作区路径").on_select(move || {
            state::copy_path_variant(&r4, &p4, &a4, line_1based, "ws_rel");
        }),
        MenuItem::new("所在目录").on_select(move || {
            state::copy_path_variant(&r5, &p5, &a5, line_1based, "dir");
        }),
    ];
    if !is_dir {
        copy_sub.push(MenuItem::new("文件与行号").on_select(move || {
            state::copy_path_variant(&r6, &p6, &a6, line_1based, "ref");
        }));
    }

    let loc_repo = repo_id;
    let loc_rel = rel_path;
    let rev_abs = abs_path.clone();
    let open_abs = abs_path;

    let mut items = vec![
        MenuItem::submenu("复制路径／引用", copy_sub),
        MenuItem::new("在文件树中定位").on_select(move || {
            state::locate_in_project_tree(&loc_repo, &loc_rel);
        }),
        MenuItem::new("在文件管理器中显示").on_select(move || {
            state::reveal_in_system_finder(&rev_abs);
        }),
    ];
    if !is_dir {
        items.push(MenuItem::new("用默认应用打开").on_select(move || {
            state::open_with_default_app(&open_abs);
        }));
    }
    items
}

fn commit_tri_state(keys: &[String], checked: &HashSet<String>) -> Option<bool> {
    let tri = tri_state_for_keys(keys.iter().map(String::as_str), checked);
    if tri == Some(true) && app_signal().with(|s| keys.iter().any(|key| s.included_hunks.contains_key(key))) {
        Some(false)
    } else {
        tri
    }
}

fn tri_checkbox(state_val: Option<bool>, p: Palette, on_toggle: impl Fn(bool) + 'static) -> impl IntoElement {
    let checked = state_val == Some(true);
    let partial = state_val == Some(false);
    let bg = if checked || partial { p.accent } else { p.surface };
    let border_col = if checked || partial { p.accent } else { p.border_strong };

    view()
        .w(px(14.0))
        .h(px(14.0))
        .flex_none()
        .rounded(px(3.0))
        .bg(bg)
        .border_1()
        .border_color(border_col)
        .items_center()
        .justify_center()
        .cursor(Cursor::Pointer)
        .on_press(move |e: &PressEvent| {
            on_toggle(e.modifiers.shift);
        })
        .child(if checked {
            Some(icons::icon(ICO_CHECK, 10.0, p.accent_text).into_element())
        } else if partial {
            Some(icons::icon(ICO_MINUS, 10.0, p.accent_text).into_element())
        } else {
            None
        })
}

fn view_option_items() -> Vec<MenuItem> {
    let (preview, flat) = app_signal().with_untracked(|s| (s.cfg.change_diff_preview, s.cfg.changes_flat));
    vec![
        MenuItem::new("按目录分组").checked(!flat).on_select(|| {
            app_signal().update(|s| s.cfg.changes_flat = !s.cfg.changes_flat);
            state::save_config_quiet();
        }),
        MenuItem::new("单击时预览差异").checked(preview).on_select(|| {
            app_signal().update(|s| s.cfg.change_diff_preview = !s.cfg.change_diff_preview);
            state::save_config_quiet();
        }),
    ]
}

fn stash_menu_items() -> Vec<MenuItem> {
    let (count, notice) = app_signal().with_untracked(|s| (s.stashes.len(), s.workspace_notice(WorkspaceData::Stashes)));
    let mut items = super::git_advanced::stash_workspace_items().into_iter().collect::<Vec<_>>();
    if let Some(notice) = notice {
        let message = match notice {
            WorkspaceNotice::Loading(message) => message.to_owned(),
            WorkspaceNotice::Failed(message) => message,
        };
        items.push(MenuItem::new(message).disabled(true));
    }
    items.push(MenuItem::new(format!("管理 Stash… ({count})")).on_select(|| {
        app_signal().update(|s| {
            s.stash_modal_open = true;
            state::enter_modal_focus(s);
        });
    }));
    items.push(MenuItem::new("刷新 Stash 列表").on_select(|| {
        state::refresh_workspace_async(None);
    }));
    items
}

fn open_stash_menu() {
    state::show_popup_menu(stash_menu_items());
}

fn set_change_list_filter(name: &str) {
    app_signal().update(|s| s.cfg.active_change_list = name.to_owned());
    state::save_config_quiet();
}

fn edit_change_list(existing: Option<String>) {
    let rename = existing.clone();
    super::git_prompt::open(
        if rename.is_some() { "重命名变更列表" } else { "新建变更列表" },
        existing.unwrap_or_default(),
        move |name| {
            let mut result = Ok(());
            app_signal().update(|s| {
                result = match &rename {
                    Some(old) => s.cfg.rename_change_list(old, name),
                    None => s.cfg.create_change_list(name),
                };
                if result.is_ok() {
                    s.cfg.active_change_list = name.to_owned();
                }
            });
            result?;
            state::save_config_quiet();
            Ok(())
        },
    );
}

fn changelist_menu_items() -> Vec<MenuItem> {
    let (names, counts, active) = app_signal().with_untracked(|s| {
        let keys: HashSet<String> = s.statuses.iter()
            .flat_map(|st| st.files.iter().map(ChangedFile::key))
            .collect();
        let mut counts = BTreeMap::<String, usize>::new();
        for key in keys {
            *counts.entry(s.cfg.change_list_for(&key).to_owned()).or_default() += 1;
        }
        let names = s.cfg.change_lists.iter().map(|list| list.name.clone()).collect::<Vec<_>>();
        (names, counts, s.cfg.active_change_list.clone())
    });
    let mut items = vec![
        MenuItem::new("全部变更").checked(active.is_empty()).on_select(|| set_change_list_filter("")),
        MenuItem::new(format!("默认变更列表 ({} 个文件)", counts.get("").copied().unwrap_or(0)))
            .checked(active == DEFAULT_CHANGE_LIST_FILTER)
            .on_select(|| set_change_list_filter(DEFAULT_CHANGE_LIST_FILTER)),
    ];
    for name in names {
        items.push(
            MenuItem::new(format!("{} ({} 个文件)", name, counts.get(&name).copied().unwrap_or(0)))
                .checked(active == name)
                .on_select(move || set_change_list_filter(&name)),
        );
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::new("新建变更列表…").on_select(|| edit_change_list(None)));
    if !active.is_empty() && active != DEFAULT_CHANGE_LIST_FILTER {
        let rename = active.clone();
        items.push(MenuItem::new("重命名当前列表…").on_select(move || edit_change_list(Some(rename.clone()))));
        items.push(MenuItem::new("删除当前列表").on_select(move || {
            app_signal().update(|s| s.cfg.delete_change_list(&active));
            state::save_config_quiet();
        }));
    }
    items
}

const DEFAULT_CHANGE_LIST_FILTER: &str = "__default__";

fn matches_change_list(cfg: &crate::config::Config, file: &ChangedFile) -> bool {
    cfg.matches_change_list(&file.key())
}

fn assign_to_list_menu(file: &ChangedFile) -> MenuItem {
    let key = file.key();
    let (names, active) = app_signal().with_untracked(|s| {
        let names = s.cfg.change_lists.iter().map(|list| list.name.clone()).collect::<Vec<_>>();
        (names, s.cfg.change_list_for(&key).to_owned())
    });
    let items = std::iter::once(String::new()).chain(names).map(|name| {
        let key = key.clone();
        MenuItem::new(if name.is_empty() { "默认变更列表".to_owned() } else { name.clone() })
            .checked(active == name)
            .on_select(move || {
                app_signal().update(|s| s.cfg.assign_change_list(&name, std::slice::from_ref(&key)));
                state::save_config_quiet();
            })
    });
    MenuItem::submenu("移到变更列表", items)
}

fn refresh_changes() {
    state::refresh_workspace_async(None);
}

fn rollback_selection() {
    if let Some(selection) = app_signal().with_untracked(|s| s.change_selection.clone()) {
        super::git_review::rollback_file(selection.file);
    }
}

fn commit_panel_options() -> Vec<MenuItem> {
    vec![
        MenuItem::submenu("变更视图选项", view_option_items()),
        MenuItem::submenu("变更列表", changelist_menu_items()),
        MenuItem::submenu("Stash", stash_menu_items()),
    ]
}

fn open_narrow_overflow_menu() {
    let has_selection = app_signal().with_untracked(|s| s.change_selection.is_some());
    let mut items = vec![
        MenuItem::new("回滚所选变更…").disabled(!has_selection).on_select(rollback_selection),
        MenuItem::new("全部展开").on_select(|| state::set_change_tree_expanded(true)),
        MenuItem::new("全部折叠").on_select(|| state::set_change_tree_expanded(false)),
        MenuItem::separator(),
    ];
    items.extend(commit_panel_options());
    state::show_popup_menu(items);
}

pub fn commit_pane() -> impl IntoElement {
    let tool = ToolWindow::new(LeftTab::Commit);
    let selected_row = element_ref();
    let selected_file = computed(|| app_signal().with(|s| {
        s.change_selection.as_ref().map(|item| (item.file.key(), item.staged))
    }));
    effect(move || {
        if selected_file.get().is_some() {
            super::reveal_selected_row(selected_row);
        }
    });
    let selected_keys = computed(|| app_signal().with(|s| {
        s.statuses.iter()
            .flat_map(|st| st.files.iter().filter(|f| matches_change_list(&s.cfg, f)).map(ChangedFile::key))
            .collect::<Vec<_>>()
    }));
    let title = view().flex_row().items_center().gap(px(6.0))
        .child(text(move || if tool.width.get() < 122.0 { "变更" } else { "本地变更" })
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(move || current_palette().text_primary)
            .number_of_lines(1u32))
        .child(when(move || tool.width.get() >= 174.0, move || {
            text(move || format!("{} 个文件", selected_keys.with(Vec::len)))
                .test_id("changes-file-count")
                .text_size(px(11.0))
                .text_color(move || current_palette().text_muted)
                .number_of_lines(1u32)
        }));

    tool.panel("commit-pane", view()
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .child(tool.header(title, || None, commit_panel_options))
        .child(workspace_notice(WorkspaceData::Changes))
        .child(dynamic(move || {
            let p = current_palette();
            let keys = selected_keys.get();
            let (root_tri, refreshing, can_rollback, compact) = (
                app_signal().with(|s| commit_tri_state(&keys, &s.checked_changes)),
                app_signal().with(|s| s.is_refreshing()),
                app_signal().with(|s| s.change_selection.is_some()),
                tool.width.get() < 208.0,
            );
            view()
                .test_id("changes-header-toolbar-row")
                .h(px(30.0))
                .px(px(8.0))
                .flex_none()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .border_b_1()
                .border_color(p.border)
                .child(tri_checkbox(root_tri, p, move |_| {
                    set_group_checked(keys.clone(), root_tri != Some(true));
                }))
                .child(button("changes-refresh-btn", "刷新变更", ICO_REFRESH, p, refreshing, true, refresh_changes))
                .child(if compact {
                    button("changes-toolbar-more", "更多变更操作", ICO_MORE, p, false, true, open_narrow_overflow_menu)
                        .into_element()
                } else {
                    view().flex_row().gap(px(2.0))
                        .child(button("changes-rollback-btn", "回滚所选变更…", ICO_UNDO, p, false, can_rollback, rollback_selection))
                        .child(button("changes-expand-all", "全部展开", ICO_EXPAND, p, false, true, || {
                            state::set_change_tree_expanded(true);
                        }))
                        .child(button("changes-collapse-all", "全部折叠", ICO_COLLAPSE, p, false, true, || {
                            state::set_change_tree_expanded(false);
                        }))
                        .child(button("changes-view-options-btn", "变更视图选项", ICO_EYE, p, false, true, || {
                            state::show_popup_menu(view_option_items());
                        }))
                        .child(button("changes-list-btn", "变更列表", ICO_FOLDER, p, false, true, || {
                            state::show_popup_menu(changelist_menu_items());
                        }))
                        .child(button("changes-stash-btn", "Stash", ICO_ARCHIVE, p, false, true, open_stash_menu))
                        .into_element()
                })
                .into_element()
        }))
        .child(super::left_tool_window::tree_filter(LeftTab::Commit, "changes-filter-input", "筛选变更文件…"))
        // 双分区变更列表
        .child(
            tree_scroll(
                "changes-tree-scroll",
                "changes-tree-rows",
                dynamic(move || {
                    let p = current_palette();
                    let font_size = app_signal().with(|s| s.cfg.tree_font_size);
                    let snapshot = app_signal().with(|s| {
                        (
                            s.statuses.iter().cloned().map(|mut status| {
                                status.files.retain(|f| matches_change_list(&s.cfg, f));
                                status
                            }).collect::<Vec<_>>(),
                            s.changes_filter.trim().to_ascii_lowercase(),
                            s.cfg.changes_flat,
                            s.change_selection.clone(),
                            s.change_selected_dir.clone(),
                            s.checked_changes.clone(),
                            s.collapsed_staged_section,
                            s.collapsed_unstaged_section,
                            s.collapsed_change_repos.clone(),
                            s.collapsed_change_dirs.clone(),
                            s.focus_zone == FocusZone::CommitTree,
                            s.workspace_notice(WorkspaceData::Changes).is_some(),
                        )
                    });
                    let (
                        statuses,
                        filter_q,
                        flat,
                        selection,
                        _selected_dir,
                        checked_set,
                        collapsed_staged,
                        collapsed_unstaged,
                        collapsed_repos,
                        collapsed_dirs,
                        tree_focused,
                        has_notice,
                    ) = snapshot;

                    let mut staged_repos: Vec<(RepoStatus, Vec<ChangedFile>)> = Vec::new();
                    let mut unstaged_repos: Vec<(RepoStatus, Vec<ChangedFile>)> = Vec::new();
                    for st in statuses {
                        let staged: Vec<ChangedFile> = st
                            .files
                            .iter()
                            .filter(|f| {
                                state::change_in_section(f, true)
                                    && (filter_q.is_empty()
                                        || f.path.to_ascii_lowercase().contains(&filter_q)
                                        || f.repo_name.to_ascii_lowercase().contains(&filter_q))
                            })
                            .cloned()
                            .collect();
                        let unstaged: Vec<ChangedFile> = st
                            .files
                            .iter()
                            .filter(|f| {
                                state::change_in_section(f, false)
                                    && (filter_q.is_empty()
                                        || f.path.to_ascii_lowercase().contains(&filter_q)
                                        || f.repo_name.to_ascii_lowercase().contains(&filter_q))
                            })
                            .cloned()
                            .collect();
                        if !staged.is_empty() {
                            staged_repos.push((st.clone(), staged));
                        }
                        if !unstaged.is_empty() {
                            unstaged_repos.push((st, unstaged));
                        }
                    }

                    if staged_repos.is_empty() && unstaged_repos.is_empty() {
                        if has_notice {
                            return view().into_element();
                        }
                        let filtered = !filter_q.is_empty() || app_signal().with_untracked(|s| {
                            !s.cfg.active_change_list.is_empty()
                        });
                        return view()
                            .py(px(32.0))
                            .px(px(14.0))
                            .items_center()
                            .justify_center()
                            .child(
                                text(if filtered { "没有匹配的本地变更" } else { "工作区干净，没有本地变更" })
                                    .text_size(px(12.0))
                                    .text_color(p.text_muted),
                            )
                            .into_element();
                    }

                    let mut sections: Vec<AnyElement> = Vec::new();
                    if !staged_repos.is_empty() {
                        sections.push(
                            render_section(
                                "已暂存的更改",
                                true,
                                collapsed_staged,
                                &staged_repos,
                                flat,
                                &selection,
                                &checked_set,
                                &collapsed_repos,
                                &collapsed_dirs,
                                tree_focused,
                                selected_row,
                                p,
                                font_size,
                            )
                            .into_element(),
                        );
                    }
                    if !unstaged_repos.is_empty() {
                        sections.push(
                            render_section(
                                "变更",
                                false,
                                collapsed_unstaged,
                                &unstaged_repos,
                                flat,
                                &selection,
                                &checked_set,
                                &collapsed_repos,
                                &collapsed_dirs,
                                tree_focused,
                                selected_row,
                                p,
                                font_size,
                            )
                            .into_element(),
                        );
                    }

                    view().flex_col().py(px(4.0)).children(sections).into_element()
                }),
            ),
        )
        .child(super::commit_form::form())
    )
}

#[allow(clippy::too_many_arguments)]
fn render_section(
    title: &'static str,
    is_staged: bool,
    collapsed: bool,
    repo_groups: &[(RepoStatus, Vec<ChangedFile>)],
    flat: bool,
    selection: &Option<state::ChangeSelection>,
    checked_set: &std::collections::HashSet<String>,
    collapsed_repos: &std::collections::HashSet<String>,
    collapsed_dirs: &std::collections::HashSet<String>,
    tree_focused: bool,
    selected_row: ElementRef,
    p: Palette,
    font_size: f32,
) -> impl IntoElement {
    let section_keys: Vec<String> = repo_groups
        .iter()
        .flat_map(|(_, fs)| fs.iter().map(|f| f.key()))
        .collect();
    let count = section_keys.len();
    let tri = commit_tri_state(&section_keys, checked_set);
    let keys_for_toggle = section_keys.clone();
    let multi_repo = repo_groups.len() > 1;
    let section_prefix = if is_staged { "staged" } else { "unstaged" };

    let mut children: Vec<AnyElement> = Vec::new();
    children.push(
        view()
            .h(px(font_size + 14.5))
            .px(px(8.0))
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .cursor(Cursor::Pointer)
            .hover(move |s| s.bg(p.surface_hover))
            .on_press(move || {
                app_signal().update(|s| {
                    if is_staged {
                        s.collapsed_staged_section = !s.collapsed_staged_section;
                    } else {
                        s.collapsed_unstaged_section = !s.collapsed_unstaged_section;
                    }
                });
            })
            .child(icons::icon(
                if collapsed {
                    ICO_CHEVRON_RIGHT
                } else {
                    ICO_CHEVRON_DOWN
                },
                10.0,
                p.text_muted,
            ))
            .child(tri_checkbox(tri, p, move |_| {
                let target = tri != Some(true);
                set_group_checked(keys_for_toggle.clone(), target);
            }))
            .child(
                text(title)
                    .text_size(px(font_size))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text_primary)
                    .number_of_lines(1u32),
            )
            .child(
                text(format!("{count}"))
                    .text_size(px(10.5))
                    .text_color(p.text_muted),
            )
            .into_element(),
    );

    if !collapsed {
        for (st, files) in repo_groups {
            let repo_key = format!("{section_prefix}::{}", st.repo.id);
            let repo_collapsed = multi_repo && collapsed_repos.contains(&repo_key);
            let repo_keys: Vec<String> = files.iter().map(|f| f.key()).collect();
            let repo_tri = commit_tri_state(&repo_keys, checked_set);
            let badge = repo_badge_colors(st.repo.color_idx, p.is_dark);

            if multi_repo {
                let rk = repo_key.clone();
                let rkeys = repo_keys.clone();
                children.push(
                    view()
                        .h(px(font_size + 12.5))
                        .pl(px(18.0))
                        .pr(px(8.0))
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .cursor(Cursor::Pointer)
                        .hover(move |s| s.bg(p.surface_hover))
                        .on_press(move || {
                            app_signal().update(|s| {
                                if !s.collapsed_change_repos.remove(&rk) {
                                    s.collapsed_change_repos.insert(rk.clone());
                                }
                            });
                        })
                        .child(icons::icon(
                            if repo_collapsed {
                                ICO_CHEVRON_RIGHT
                            } else {
                                ICO_CHEVRON_DOWN
                            },
                            10.0,
                            p.text_muted,
                        ))
                        .child(tri_checkbox(repo_tri, p, move |_| {
                            let target = repo_tri != Some(true);
                            set_group_checked(rkeys.clone(), target);
                        }))
                        .child(
                            view()
                                .w(px(15.0))
                                .h(px(15.0))
                                .rounded(px(3.0))
                                .bg(badge.bg)
                                .items_center()
                                .justify_center()
                                .child(
                                    text(st.repo.monogram.clone())
                                        .text_size(px(8.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(badge.text),
                                ),
                        )
                        .child(
                            text(st.repo.label().to_owned())
                                .flex_none()
                                .text_size(px(font_size))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.text_primary)
                                .number_of_lines(1u32),
                        )
                        .into_element(),
                );
            }

            if !repo_collapsed {
                let base_depth = if multi_repo { 2 } else { 1 };
                if flat {
                    let mut sorted = files.clone();
                    sorted.sort_by(|a, b| a.path.cmp(&b.path));
                    for cf in sorted {
                        children.push(
                            render_change_file_row(
                                cf,
                                is_staged,
                                true,
                                base_depth,
                                selection,
                                checked_set,
                                tree_focused,
                                selected_row,
                                p,
                                font_size,
                            )
                            .into_element(),
                        );
                    }
                } else {
                    let tree = build_change_tree(files);
                    render_change_dir_children(
                        &tree,
                        &format!("{section_prefix}::{}", st.repo.id),
                        &st.repo.id,
                        &st.repo.path,
                        is_staged,
                        base_depth,
                        selection,
                        checked_set,
                        collapsed_dirs,
                        tree_focused,
                        selected_row,
                        p,
                        font_size,
                        &mut children,
                    );
                }
            }
        }
    }

    view().flex_col().children(children)
}

#[allow(clippy::too_many_arguments)]
fn render_change_dir_children(
    node: &ChangeDirNode,
    prefix: &str,
    repo_id: &str,
    repo_path: &str,
    is_staged: bool,
    depth: usize,
    selection: &Option<state::ChangeSelection>,
    checked_set: &std::collections::HashSet<String>,
    collapsed_dirs: &std::collections::HashSet<String>,
    tree_focused: bool,
    selected_row: ElementRef,
    p: Palette,
    font_size: f32,
    out: &mut Vec<AnyElement>,
) {
    let indent = 10.0 + (depth as f32) * 14.0;
    for sub in node.subdirs.values() {
        let dir_key = format!("{prefix}::{}", sub.rel_path);
        let dir_collapsed = collapsed_dirs.contains(&dir_key);
        let mut keys = Vec::new();
        collect_tree_keys(sub, &mut keys);
        let tri = commit_tri_state(&keys, checked_set);
        let dk = dir_key.clone();
        let dk_chev = dir_key.clone();
        let abs_dir = std::path::Path::new(repo_path)
            .join(&sub.rel_path)
            .to_string_lossy()
            .to_string();
        let cm_repo_id = repo_id.to_string();
        let cm_repo_path = repo_path.to_string();
        let cm_rel = sub.rel_path.clone();
        let cm_abs = abs_dir.clone();
        let sel_rid = repo_id.to_string();
        let sel_rel = sub.rel_path.clone();
        let sel_abs = abs_dir.clone();
        let is_dir_selected = app_signal().with_untracked(|s| {
            s.change_selection.is_none()
                && s.change_selected_dir
                    .as_ref()
                    .is_some_and(|(r, p_rel, _)| r == repo_id && p_rel == &sub.rel_path)
        });
        let dir_row_bg = if is_dir_selected {
            if tree_focused {
                p.active_row_bg
            } else {
                p.surface2
            }
        } else {
            transparent_black()
        };
        let dir_test_id = format!("change-dir-{}-{}", if is_staged { "staged" } else { "unstaged" }, sub.rel_path);

        out.push(
            view()
                .test_id(&dir_test_id)
                .h(px(font_size + 12.5))
                .pl(px(indent))
                .pr(px(8.0))
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .bg(dir_row_bg)
                .cursor(Cursor::Pointer)
                .hover(move |s| if is_dir_selected { s } else { s.bg(p.surface_hover) })
                .on_press(move |e: &PressEvent| {
                    app_signal().update(|s| {
                        s.change_selection = None;
                        s.change_selected_dir = Some((sel_rid.clone(), sel_rel.clone(), sel_abs.clone()));
                        s.focus_zone = FocusZone::CommitTree;
                        if e.clicks >= 2 {
                            if !s.collapsed_change_dirs.remove(&dk) {
                                s.collapsed_change_dirs.insert(dk.clone());
                            }
                        }
                    });
                })
                .on_context_menu(move |e: &ContextMenuEvent| {
                    let mut items = vec![
                        MenuItem::new("复制").on_select({
                            let abs = cm_abs.clone();
                            move || set_clipboard(display_path(&abs))
                        }),
                        MenuItem::separator(),
                    ];
                    items.extend(build_location_menu_items(
                        cm_repo_id.clone(),
                        cm_repo_path.clone(),
                        cm_rel.clone(),
                        cm_abs.clone(),
                        1,
                        true,
                    ));
                    crate::state::show_popup_menu_at(items, e.page_position);
                })
                .child(
                    view()
                        .w(px(14.0))
                        .h(px(16.0))
                        .items_center()
                        .justify_center()
                        .on_press(move || {
                            app_signal().update(|s| {
                                if !s.collapsed_change_dirs.remove(&dk_chev) {
                                    s.collapsed_change_dirs.insert(dk_chev.clone());
                                }
                            });
                        })
                        .child(icons::icon(
                            if dir_collapsed {
                                ICO_CHEVRON_RIGHT
                            } else {
                                ICO_CHEVRON_DOWN
                            },
                            10.0,
                            p.text_muted,
                        )),
                )
                .child(tri_checkbox(tri, p, move |_| {
                    let target = tri != Some(true);
                    set_group_checked(keys.clone(), target);
                }))
                .child(icons::icon(ICO_FOLDER, 12.0, p.text_control))
                .child(
                    text(sub.name.clone())
                        .flex_none()
                        .text_size(px(font_size))
                        .text_color(p.text_primary)
                        .number_of_lines(1u32),
                )
                .into_element(),
        );

        if !dir_collapsed {
            render_change_dir_children(
                sub,
                prefix,
                repo_id,
                repo_path,
                is_staged,
                depth + 1,
                selection,
                checked_set,
                collapsed_dirs,
                tree_focused,
                selected_row,
                p,
                font_size,
                out,
            );
        }
    }

    for cf in &node.files {
        out.push(
            render_change_file_row(
                cf.clone(),
                is_staged,
                false,
                depth,
                selection,
                checked_set,
                tree_focused,
                selected_row,
                p,
                font_size,
            )
            .into_element(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_change_file_row(
    cf: ChangedFile,
    is_staged: bool,
    show_parent_dir: bool,
    depth: usize,
    selection: &Option<state::ChangeSelection>,
    checked_set: &std::collections::HashSet<String>,
    tree_focused: bool,
    selected_row: ElementRef,
    p: Palette,
    font_size: f32,
) -> impl IntoElement {
    let key = cf.key();
    let is_selected = selection
        .as_ref()
        .is_some_and(|sel| sel.staged == is_staged && sel.file.key() == key);
    let is_checked = checked_set.contains(&key);
    let partial = app_signal().with(|s| s.included_hunks.contains_key(&key));
    let indent = 14.0 + (depth as f32) * 14.0;
    let status_col = status_text_color(&cf.kind, p);
    let (parent_dir, filename) = cf.path.rsplit_once('/').unwrap_or(("", cf.path.as_str()));
    let is_deleted = cf.kind == "D";
    let row_bg = if is_selected {
        if tree_focused {
            p.active_row_bg
        } else {
            p.surface2
        }
    } else {
        transparent_black()
    };

    let test_id_val = format!(
        "change-row-{}-{}",
        if is_staged { "staged" } else { "unstaged" },
        cf.path
    );
    let cf_click = cf.clone();
    let cf_cm = cf.clone();
    let key_chk = key.clone();

    let name_el = text(filename.to_owned())
        .flex_none()
        .text_size(px(font_size))
        .text_color(status_col)
        .number_of_lines(1u32);
    let name_el = if is_deleted {
        name_el.line_through()
    } else {
        name_el
    };

    view()
        .test_id(&test_id_val)
        .when(is_selected, |row| row.element_ref(&selected_row))
        .h(px(font_size + 12.5))
        .pl(px(indent))
        .pr(px(8.0))
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .bg(row_bg)
        .cursor(Cursor::Pointer)
        .hover(move |s| {
            if is_selected {
                s
            } else {
                s.bg(p.surface_hover)
            }
        })
        .on_press(move |e: &PressEvent| {
            click_change_row(cf_click.clone(), is_staged, e.clicks);
        })
        .on_context_menu(move |e: &ContextMenuEvent| {
            let f = cf_cm.clone();
            app_signal().update(|s| {
                s.change_selection = Some(state::ChangeSelection {
                    file: f.clone(),
                    staged: is_staged,
                });
                s.focus_zone = FocusZone::CommitTree;
            });
            let copy_val = if f.untracked {
                display_path(&f.abs_path())
            } else {
                basename(&f.path)
            };
            let f_diff = f.clone();
            let f_open = f.clone();
            let f_stage = f.clone();
            let mut items = vec![
                MenuItem::new("查看差异")
                    .shortcut("mod+d")
                    .on_select(move || open_working_diff(f_diff.clone(), is_staged, true)),
                MenuItem::new("在文件视图打开").on_select(move || {
                    open_code_file(
                        WorkspaceFile {
                            repo_id: f_open.repo_id.clone(),
                            repo_name: f_open.repo_name.clone(),
                            repo_path: f_open.repo_path.clone(),
                            rel_path: f_open.path.clone(),
                            abs_path: f_open.abs_path(),
                            color_idx: 0,
                        },
                        None,
                        true,
                    );
                }),
                MenuItem::new(if is_staged { "取消暂存" } else { "暂存" }).on_select(move || {
                    stage_files_action(vec![f_stage.clone()], !is_staged);
                }),
                {
                    let file = f.clone();
                    MenuItem::new("HEAD、暂存区与工作区对比").on_select(move || super::index_review::open(file.clone()))
                },
                {
                    let file = f.clone();
                    MenuItem::new("回滚文件…").on_select(move || super::git_review::rollback_file(file.clone()))
                },
                assign_to_list_menu(&f),
                MenuItem::separator(),
                MenuItem::new("复制").on_select(move || set_clipboard(copy_val.clone())),
            ];
            if f.conflict {
                let repo = std::path::PathBuf::from(&f.repo_path);
                let path = f.path.clone();
                items.insert(0, MenuItem::new("解决冲突…").on_select(move || {
                    super::conflict_workbench::open_file(repo.clone(), path.clone());
                }));
            }
            items.extend(build_location_menu_items(
                f.repo_id.clone(),
                f.repo_path.clone(),
                f.path.clone(),
                f.abs_path(),
                1,
                false,
            ));
            crate::state::show_popup_menu_at(items, e.page_position);
        })
        .child(tri_checkbox(
            if partial { Some(false) } else if is_checked { Some(true) } else { None },
            p,
            move |shift| state::toggle_change_checked_in_section(&key_chk, is_staged, shift),
        ))
        .child(icons::icon(ICO_FILE, 11.5, status_col))
        .child(name_el)
        .child((show_parent_dir && !parent_dir.is_empty()).then(|| {
            text(parent_dir.to_owned())
                .flex_none()
                .text_size(px(10.0))
                .text_color(p.text_muted)
                .number_of_lines(1u32)
        }))
        .child(view().flex_1())
        .child((!cf.untracked).then(|| {
            text(cf.kind.clone())
                .flex_none()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(status_col)
        }))
}
