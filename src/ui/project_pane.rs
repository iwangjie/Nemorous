use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::WorkspaceFile;
use crate::paths::basename;
use crate::state::{
    self, FocusZone, LeftTab, WorkspaceData, app_signal, current_palette, open_code_file, open_code_file_preview,
};
use crate::theme::{Palette, repo_badge_colors, status_text_color};
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

use super::left_tool_window::{ToolWindow, button, tree_scroll, workspace_notice};

#[derive(Default, PartialEq, Eq)]
struct ProjectDir {
    name: String,
    rel_path: String,
    subdirs: BTreeMap<String, ProjectDir>,
    files: Vec<WorkspaceFile>,
}

fn directory_node<'a>(mut root: &'a mut ProjectDir, path: &str) -> &'a mut ProjectDir {
    let mut relative = String::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if !relative.is_empty() { relative.push('/'); }
        relative.push_str(segment);
        root = root.subdirs.entry(segment.to_owned()).or_insert_with(|| ProjectDir {
            name: segment.to_owned(), rel_path: relative.clone(), ..Default::default()
        });
    }
    root
}

fn build_project_tree<'a>(files: impl IntoIterator<Item = &'a WorkspaceFile>) -> ProjectDir {
    let mut root = ProjectDir::default();
    for wf in files {
        let parent = wf.rel_path.rsplit_once('/').map_or("", |(parent, _)| parent);
        directory_node(&mut root, parent).files.push(wf.clone());
    }
    sort_dir(&mut root);
    root
}

fn sort_dir(node: &mut ProjectDir) {
    node.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    for sub in node.subdirs.values_mut() {
        sort_dir(sub);
    }
}

const FULL_PROJECT_HEADER: f32 = 172.0;

fn project_options(width: f32) -> Vec<MenuItem> {
    let (follow_editor, has_directory) = app_signal().with_untracked(|s| {
        (s.cfg.project_follow_editor, s.project_selected_dir.is_some())
    });
    let mut items = vec![MenuItem::submenu("行为", [
        MenuItem::new("始终选中打开的文件")
            .checked(follow_editor)
            .on_select(|| {
                app_signal().update(|s| {
                    s.cfg.project_follow_editor = !s.cfg.project_follow_editor;
                });
                state::save_config_quiet();
            }),
    ])];
    if width < FULL_PROJECT_HEADER {
        items.extend([
            MenuItem::separator(),
            MenuItem::new("定位当前编辑文件").on_select(state::locate_current_in_project_tree),
            MenuItem::new("展开选中目录").disabled(!has_directory).on_select(expand_selected),
            MenuItem::new("全部折叠").on_select(collapse_all),
        ]);
    }
    items.push(MenuItem::submenu("仓库管理", super::workspace_menu::repository_items()));
    items
}

fn expand_selected() {
    app_signal().update(|s| {
        let Some((repo_id, directory)) = &s.project_selected_dir else {
            return;
        };
        let directory = Path::new(directory);
        s.project_workspace_expanded = true;
        s.expanded_project_repos.insert(repo_id.clone());
        for parent in directory.ancestors().take_while(|path| !path.as_os_str().is_empty()) {
            let parent = crate::paths::normalize_slashes(&parent.to_string_lossy());
            s.expanded_project_dirs.insert(format!("{repo_id}::{parent}"));
        }
        for file in s.workspace_files.iter() {
            let path = Path::new(&file.rel_path);
            if file.repo_id == *repo_id && path.starts_with(directory) {
                for parent in path.ancestors().skip(1).take_while(|parent| parent.starts_with(directory)) {
                    let parent = crate::paths::normalize_slashes(&parent.to_string_lossy());
                    s.expanded_project_dirs.insert(format!("{repo_id}::{parent}"));
                }
            }
        }
    });
}

fn collapse_all() {
    app_signal().update(|s| {
        s.expanded_project_repos.clear();
        s.expanded_project_dirs.clear();
        s.project_workspace_expanded = false;
        s.focus_zone = FocusZone::ProjectTree;
    });
}

pub fn project_pane() -> impl IntoElement {
    let tool = ToolWindow::new(LeftTab::Project);
    let located_row = element_ref();
    let reveal_request = computed(|| app_signal().with(|s| s.project_reveal_request));
    let files = computed(|| app_signal().with(|s| s.workspace_files.clone()));
    let directories = computed(|| app_signal().with(|s| s.project_directories()));
    let filter = computed(|| app_signal().with(|s| s.project_filter.trim().to_ascii_lowercase()));
    let trees = computed(move || {
        let files = files.get();
        let filter = filter.get();
        let mut by_repo: HashMap<&str, Vec<&WorkspaceFile>> = HashMap::new();
        for file in files.iter().filter(|file| {
            filter.is_empty() || file.rel_path.to_ascii_lowercase().contains(&filter)
        }) {
            by_repo.entry(&file.repo_id).or_default().push(file);
        }
        let mut trees = by_repo.into_iter()
            .map(|(id, files)| (id.to_owned(), build_project_tree(files)))
            .collect::<HashMap<_, _>>();
        for (repo, directory) in directories.get().iter() {
            if filter.is_empty() || directory.to_ascii_lowercase().contains(&filter) {
                directory_node(trees.entry(repo.clone()).or_default(), directory);
            }
        }
        Rc::new(trees)
    });
    effect(move || {
        if reveal_request.get() != 0 {
            super::reveal_selected_row(located_row);
        }
    });
    let title = text(move || if tool.width.get() < 116.0 { "文件" } else { "文件树" })
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(move || current_palette().text_primary)
        .number_of_lines(1u32);
    let header = tool.header(title, move || {
        if tool.width.get() < FULL_PROJECT_HEADER {
            return None;
        }
        let p = current_palette();
        let selected = app_signal().with(|s| s.project_selected_dir.is_some());
        Some(view().flex_row().gap(px(2.0))
            .child(button("project-locate-current", "定位当前编辑文件", ICO_LOCATE, p,
                          false, true, state::locate_current_in_project_tree))
            .child(button("project-expand-selected", "展开选中目录", ICO_EXPAND, p,
                          false, selected, expand_selected))
            .child(button("project-collapse-all", "全部折叠", ICO_COLLAPSE, p,
                          false, true, collapse_all))
            .into_element())
    }, move || project_options(tool.width.get_untracked()));
    tool.panel("project-pane", view()
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .child(header)
        .child(workspace_notice(WorkspaceData::Files))
        .child(super::left_tool_window::tree_filter(LeftTab::Project, "project-filter-input", "过滤文件…"))
        .child(
            tree_scroll(
                "project-tree-scroll",
                "project-tree-rows",
                dynamic(move || {
                    let p = current_palette();
                    let font_size = app_signal().with(|s| s.cfg.tree_font_size);
                    let snap = app_signal().with(|s| {
                        let mut status_map: HashMap<String, String> = HashMap::new();
                        let mut dirty_dirs: HashSet<String> = HashSet::new();
                        for st in &s.statuses {
                            for cf in &st.files {
                                status_map.insert(cf.key(), cf.kind.clone());
                                let parts: Vec<&str> = cf.path.split('/').collect();
                                let mut acc = String::new();
                                for seg in parts.iter().take(parts.len().saturating_sub(1)) {
                                    if !acc.is_empty() {
                                        acc.push('/');
                                    }
                                    acc.push_str(seg);
                                    dirty_dirs.insert(format!("{}::{acc}", cf.repo_id));
                                }
                            }
                        }
                        let folder = s.plain_project();
                        let is_folder = folder.is_some();
                        let mut repos = s.repos.clone();
                        repos.extend(folder);
                        (
                            repos,
                            is_folder,
                            s.workspace_notice(WorkspaceData::Files).is_some(),
                            s.expanded_project_repos.clone(),
                            s.expanded_project_dirs.clone(),
                            (s.project_located_key.clone(), s.project_compare_keys.clone()),
                            s.project_workspace_expanded,
                            basename(&s.cfg.workspace_path),
                            status_map,
                            dirty_dirs,
                        )
                    });
                    let (
                        repos,
                        is_folder,
                        has_notice,
                        expanded_repos,
                        expanded_dirs,
                        located_key,
                        workspace_expanded_state,
                        workspace_name,
                        status_map,
                        dirty_dirs,
                    ) = snap;

                    if repos.is_empty() {
                        return view()
                            .py(px(if has_notice { 0.0 } else { 32.0 }))
                            .items_center()
                            .child((!has_notice).then(|| text("未发现仓库").text_size(px(12.0)).text_color(p.text_muted)))
                            .into_element();
                    }

                    let trees = trees.get();
                    let force_expand = !filter.get().is_empty();
                    let mut rows: Vec<AnyElement> = Vec::new();
                    let workspace_expanded = is_folder || force_expand || workspace_expanded_state;
                    if !is_folder { rows.push(
                        view()
                            .test_id("project-workspace-root")
                            .h(px(font_size + 14.5))
                            .px(px(8.0))
                            .flex_row()
                            .items_center()
                            .gap(px(5.0))
                            .cursor(Cursor::Pointer)
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_press(move || {
                                app_signal().update(|s| {
                                    if !force_expand {
                                        s.project_workspace_expanded = !s.project_workspace_expanded;
                                    }
                                    s.focus_zone = FocusZone::ProjectTree;
                                });
                            })
                            .child(icons::icon(
                                if workspace_expanded {
                                    ICO_CHEVRON_DOWN
                                } else {
                                    ICO_CHEVRON_RIGHT
                                },
                                10.0,
                                p.text_muted,
                            ))
                            .child(
                                text(if workspace_name.is_empty() { "工作区".into() } else { workspace_name })
                                    .flex_none()
                                    .text_size(px(font_size))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary),
                            )
                            .into_element(),
                    ); }
                    let repos = if workspace_expanded { repos } else { Vec::new() };
                    for repo in repos {
                        let tree = trees.get(&repo.id);
                        if force_expand && tree.is_none() {
                            continue;
                        }
                        let expanded = force_expand || expanded_repos.contains(&repo.id);
                        let badge = repo_badge_colors(repo.color_idx, p.is_dark);
                        let rid = repo.id.clone();
                        let sync_repo = repo.clone();

                        rows.push(
                            view()
                                .test_id(&if is_folder { "project-folder-root".to_owned() } else { format!("project-repo-{}", repo.id) })
                                .h(px(font_size + 14.5))
                                .px(px(8.0))
                                .flex_row()
                                .items_center()
                                .gap(px(5.0))
                                .cursor(Cursor::Pointer)
                                .hover(move |s| s.bg(p.surface_hover))
                                .on_press(move || {
                                    app_signal().update(|s| {
                                        if !s.expanded_project_repos.remove(&rid) {
                                            s.expanded_project_repos.insert(rid.clone());
                                        }
                                        s.focus_zone = FocusZone::ProjectTree;
                                    });
                                })
                                .on_context_menu(move |event: &ContextMenuEvent| {
                                    app_signal().update(|s| {
                                        s.project_selected_dir = Some((sync_repo.id.clone(), String::new()));
                                        s.project_selected_file = None;
                                        s.focus_zone = FocusZone::ProjectTree;
                                    });
                                    let mut items = vec![super::workspace_menu::new_item_menu()];
                                    if !is_folder {
                                        items.extend(super::sync_dialog::menu(Some(sync_repo.clone())));
                                        items.push(MenuItem::submenu("Git 高级", super::git_advanced::repo_items(sync_repo.clone())));
                                        if super::mani_panel::enabled() {
                                            let path = std::path::PathBuf::from(&sync_repo.path);
                                            items.push(MenuItem::new("运行 Mani 任务…").on_select(move || super::mani_panel::open_project(path.clone())));
                                        }
                                    }
                                    crate::state::show_popup_menu_at(items, event.page_position);
                                })
                                .child(icons::icon(
                                    if expanded {
                                        ICO_CHEVRON_DOWN
                                    } else {
                                        ICO_CHEVRON_RIGHT
                                    },
                                    10.0,
                                    p.text_muted,
                                ))
                                .child(
                                    view()
                                        .w(px(15.0))
                                        .h(px(15.0))
                                        .rounded(px(3.0))
                                        .bg(badge.bg)
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            text(repo.monogram.clone())
                                                .text_size(px(8.5))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(badge.text),
                                        ),
                                )
                                .child(
                                    text(repo.label().to_owned())
                                        .flex_none()
                                        .text_size(px(font_size))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_primary)
                                        .number_of_lines(1u32),
                                )
                                .into_element(),
                        );

                        if let Some(tree) = tree.filter(|_| expanded) {
                            render_project_dir(
                                tree,
                                &repo.id,
                                &repo.path,
                                1,
                                force_expand,
                                &expanded_dirs,
                                &located_key,
                                located_row,
                                &status_map,
                                &dirty_dirs,
                                p,
                                font_size,
                                &mut rows,
                            );
                        }
                    }

                    view().flex_col().py(px(4.0)).children(rows).into_element()
                }),
            ),
        ))
}

#[allow(clippy::too_many_arguments)]
fn render_project_dir(
    node: &ProjectDir,
    repo_id: &str,
    repo_path: &str,
    depth: usize,
    force_expand: bool,
    expanded_dirs: &HashSet<String>,
    located_key: &(Option<String>, Vec<String>),
    located_row: nsh_core::element::ElementRef,
    status_map: &HashMap<String, String>,
    dirty_dirs: &HashSet<String>,
    p: Palette,
    font_size: f32,
    out: &mut Vec<AnyElement>,
) {
    let indent = 12.0 + (depth as f32) * 14.0;
    for sub in node.subdirs.values() {
        let dir_key = format!("{repo_id}::{}", sub.rel_path);
        let expanded = force_expand || expanded_dirs.contains(&dir_key);
        let selected = located_key.0.as_deref() == Some(dir_key.as_str());
        let folder_col = if dirty_dirs.contains(&dir_key) {
            p.blue
        } else {
            p.text_control
        };
        let abs_dir = Path::new(repo_path)
            .join(&sub.rel_path)
            .to_string_lossy()
            .to_string();

        let dk_click = dir_key.clone();
        let rid_click = repo_id.to_string();
        let rel_click = sub.rel_path.clone();
        let dk_chev = dir_key.clone();
        let cm_rid = repo_id.to_string();
        let cm_rpath = repo_path.to_string();
        let cm_rel = sub.rel_path.clone();
        let cm_abs = abs_dir;

        out.push(
            view()
                .test_id(&format!("project-dir-{}", sub.rel_path))
                .when(selected, |row| row.element_ref(&located_row))
                .h(px(font_size + 12.5))
                .pl(px(indent))
                .pr(px(8.0))
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .bg(if selected {
                    p.active_row_bg
                } else {
                    transparent_black()
                })
                .cursor(Cursor::Pointer)
                .hover(move |s| {
                    if selected {
                        s
                    } else {
                        s.bg(p.surface_hover)
                    }
                })
                .on_press(move |e: &PressEvent| {
                    app_signal().update(|s| {
                        s.project_located_key = Some(dk_click.clone());
                        s.project_compare_keys.clear();
                        s.project_selected_file = None;
                        s.project_selected_dir = Some((rid_click.clone(), rel_click.clone()));
                        s.focus_zone = FocusZone::ProjectTree;
                        if e.clicks >= 2 {
                            if !s.expanded_project_dirs.remove(&dk_click) {
                                s.expanded_project_dirs.insert(dk_click.clone());
                            }
                        }
                    });
                })
                .on_context_menu(move |e: &ContextMenuEvent| {
                    app_signal().update(|s| {
                        s.project_selected_dir = Some((cm_rid.clone(), cm_rel.clone()));
                        s.project_selected_file = None;
                        s.focus_zone = FocusZone::ProjectTree;
                    });
                    let mut items = build_location_menu_items(
                        cm_rid.clone(),
                        cm_rpath.clone(),
                        cm_rel.clone(),
                        cm_abs.clone(),
                        1,
                        true,
                    );
                    items.insert(0, super::workspace_menu::new_item_menu());
                    let search_dir = cm_abs.clone();
                    items.push(
                        MenuItem::new("在目录中查找").on_select(move || {
                            crate::ui::content_search::open_for_directory(search_dir.clone());
                        }),
                    );
                    crate::state::show_popup_menu_at(items, e.page_position);
                })
                .child(
                    super::controls::button()
                        .test_id(&format!("project-dir-toggle-{}", sub.rel_path))
                        .w(px(14.0))
                        .h(px(16.0))
                        .on_press(move || {
                            app_signal().update(|s| {
                                if !s.expanded_project_dirs.remove(&dk_chev) {
                                    s.expanded_project_dirs.insert(dk_chev.clone());
                                }
                            });
                        })
                        .child(icons::icon(
                            if expanded {
                                ICO_CHEVRON_DOWN
                            } else {
                                ICO_CHEVRON_RIGHT
                            },
                            10.0,
                            p.text_muted,
                        )),
                )
                .child(icons::icon(ICO_FOLDER, 12.0, folder_col))
                .child(
                    text(sub.name.clone())
                        .flex_none()
                        .text_size(px(font_size))
                        .text_color(p.text_primary)
                        .number_of_lines(1u32),
                )
                .into_element(),
        );

        if expanded {
            render_project_dir(
                sub,
                repo_id,
                repo_path,
                depth + 1,
                force_expand,
                expanded_dirs,
                located_key,
                located_row,
                status_map,
                dirty_dirs,
                p,
                font_size,
                out,
            );
        }
    }

    for wf in &node.files {
        let key = wf.key();
        let selected = located_key.0.as_deref() == Some(key.as_str()) || located_key.1.contains(&key);
        let text_col = status_map
            .get(&key)
            .map(|k| status_text_color(k, p))
            .unwrap_or(if selected {
                p.text_primary
            } else {
                p.text_secondary
            });
        let base = basename(&wf.rel_path);
        let wf_click = wf.clone();
        let wf_cm = wf.clone();

        out.push(
            view()
                .test_id(&format!("project-file-{}", wf.rel_path))
                .when(located_key.0.as_deref() == Some(key.as_str()), |row| row.element_ref(&located_row))
                .h(px(font_size + 12.5))
                .pl(px(indent + 16.0))
                .pr(px(8.0))
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .bg(if selected {
                    p.active_row_bg
                } else {
                    transparent_black()
                })
                .cursor(Cursor::Pointer)
                .hover(move |s| {
                    if selected {
                        s
                    } else {
                        s.bg(p.surface_hover)
                    }
                })
                .on_press(move |e: &PressEvent| {
                    let f = wf_click.clone();
                    let extended = if apple_keys() { e.modifiers.cmd } else { e.modifiers.ctrl };
                    app_signal().update(|s| {
                        let key = f.key();
                        if extended {
                            if let Some(index) = s.project_compare_keys.iter().position(|existing| existing == &key) {
                                s.project_compare_keys.remove(index);
                                s.project_selected_file = s.project_compare_keys.last().and_then(|active| {
                                    s.workspace_files.iter().find(|file| file.key() == *active).cloned()
                                });
                            } else {
                                s.project_compare_keys.push(key.clone());
                                s.project_selected_file = Some(f.clone());
                            }
                            s.project_located_key = s.project_compare_keys.last().cloned();
                        } else {
                            s.project_compare_keys = vec![key.clone()];
                            s.project_located_key = Some(key);
                            s.project_selected_file = Some(f.clone());
                        }
                        s.project_selected_dir = None;
                        s.focus_zone = FocusZone::ProjectTree;
                    });
                    if !extended && e.clicks >= 2 {
                        open_code_file(f, None, true);
                    }
                })
                .on_context_menu(move |e: &ContextMenuEvent| {
                    let f = wf_cm.clone();
                    app_signal().update(|s| {
                        let key = f.key();
                        if !s.project_compare_keys.contains(&key) {
                            s.project_compare_keys = vec![key.clone()];
                        }
                        s.project_located_key = Some(key);
                        s.project_selected_file = Some(f.clone());
                        s.project_selected_dir = None;
                        s.focus_zone = FocusZone::ProjectTree;
                    });
                    let f_open = f.clone();
                    let f_preview = f.clone();
                    let mut items = vec![
                        MenuItem::new("打开源码").on_select(move || {
                            open_code_file(f_open.clone(), None, true);
                        }),
                        MenuItem::new("预览").on_select(move || {
                            open_code_file_preview(f_preview.clone());
                        }),
                        MenuItem::new("复制").on_select({
                            let name = basename(&f.rel_path);
                            move || set_clipboard(name.clone())
                        }),
                        MenuItem::separator(),
                    ];
                    if app_signal().with_untracked(|s| s.project_compare_keys.len() == 2 && s.project_compare_keys.contains(&f.key())) {
                        items.push(MenuItem::new("比较选中的文件").on_select(|| {
                            super::comparison::open_project_selection();
                        }));
                    }
                    let history = f.clone();
                    let revision = f.clone();
                    if app_signal().with_untracked(|s| s.repos.iter().any(|repo| repo.id == f.repo_id)) {
                        items.push(MenuItem::submenu("Git", [
                            MenuItem::new("查看文件历史").on_select(move || super::revision_picker::show_history(history.clone())),
                            MenuItem::new("与修订版本比较…").on_select(move || super::revision_picker::open(revision.clone())),
                        ]));
                    }
                    items.extend(build_location_menu_items(
                        f.repo_id.clone(),
                        f.repo_path.clone(),
                        f.rel_path.clone(),
                        f.abs_path.clone(),
                        1,
                        false,
                    ));
                    crate::state::show_popup_menu_at(items, e.page_position);
                })
                .child(icons::file_icon(&wf.rel_path, 12.0, p))
                .child(
                    text(base)
                        .flex_none()
                        .text_size(px(font_size))
                        .text_color(text_col)
                        .number_of_lines(1u32),
                )
                .into_element(),
        );
    }
}
