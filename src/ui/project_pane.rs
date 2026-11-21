use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use nsh_base::input;
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::WorkspaceFile;
use crate::paths::basename;
use crate::state::{FocusZone, app_signal, current_palette, open_code_file};
use crate::theme::{Palette, repo_badge_colors, status_text_color};
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

#[derive(Default)]
struct ProjectDir {
    name: String,
    rel_path: String,
    subdirs: BTreeMap<String, ProjectDir>,
    files: Vec<WorkspaceFile>,
}

fn build_project_tree(files: &[WorkspaceFile]) -> ProjectDir {
    let mut root = ProjectDir::default();
    for wf in files {
        let parts: Vec<&str> = wf.rel_path.split('/').collect();
        if parts.len() <= 1 {
            root.files.push(wf.clone());
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
                .or_insert_with(|| ProjectDir {
                    name: (*seg).to_string(),
                    rel_path: acc.clone(),
                    subdirs: BTreeMap::new(),
                    files: Vec::new(),
                });
        }
        cur.files.push(wf.clone());
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

pub fn project_pane() -> impl IntoElement {
    view()
        .test_id("project-pane")
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .bg(move || current_palette().rail_bg)
        .child(
            view()
                .h(px(34.0))
                .px(px(10.0))
                .flex_row()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(move || current_palette().border)
                .child(
                    text("文件树")
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || current_palette().text_primary),
                )
                .child(
                    text(move || {
                        let cnt = app_signal().with(|s| s.workspace_files.len());
                        format!("{cnt} 个文件")
                    })
                    .text_size(px(10.5))
                    .text_color(move || current_palette().text_muted),
                ),
        )
        .child(
            view()
                .px(px(8.0))
                .py(px(6.0))
                .border_b_1()
                .border_color(move || current_palette().border)
                .child(
                    input()
                        .test_id("project-filter-input")
                        .h(px(26.0))
                        .px(px(8.0))
                        .rounded(px(5.0))
                        .bg(move || current_palette().surface)
                        .border_1()
                        .border_color(move || current_palette().border)
                        .text_size(px(11.5))
                        .text_color(move || current_palette().text_primary)
                        .placeholder("过滤文件…")
                        .value(move || app_signal().with(|s| s.project_filter.clone()))
                        .on_focus(|| crate::state::set_text_input_focused(true))
                        .on_blur(|| crate::state::set_text_input_focused(false))
                        .on_change(|v: &str| {
                            app_signal().update(|s| s.project_filter = v.to_string());
                        }),
                ),
        )
        .child(
            scroll()
                .flex_1()
                .child(dynamic(move || {
                    let p = current_palette();
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
                        (
                            s.repos.clone(),
                            s.workspace_files.clone(),
                            s.project_filter.trim().to_ascii_lowercase(),
                            s.expanded_project_repos.clone(),
                            s.expanded_project_dirs.clone(),
                            s.project_located_key.clone(),
                            status_map,
                            dirty_dirs,
                        )
                    });
                    let (
                        repos,
                        files,
                        filter_q,
                        expanded_repos,
                        expanded_dirs,
                        located_key,
                        status_map,
                        dirty_dirs,
                    ) = snap;

                    if repos.is_empty() {
                        return view()
                            .py(px(32.0))
                            .items_center()
                            .child(text("未发现仓库").text_size(px(12.0)).text_color(p.text_muted))
                            .into_element();
                    }

                    let force_expand = !filter_q.is_empty();
                    let mut rows: Vec<AnyElement> = Vec::new();
                    for repo in repos {
                        let repo_files: Vec<WorkspaceFile> = files
                            .iter()
                            .filter(|f| {
                                f.repo_id == repo.id
                                    && (filter_q.is_empty()
                                        || f.rel_path.to_ascii_lowercase().contains(&filter_q))
                            })
                            .cloned()
                            .collect();
                        if force_expand && repo_files.is_empty() {
                            continue;
                        }
                        let expanded = force_expand || expanded_repos.contains(&repo.id);
                        let badge = repo_badge_colors(repo.color_idx, p.is_dark);
                        let rid = repo.id.clone();

                        rows.push(
                            view()
                                .h(px(26.0))
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
                                    text(repo.name.clone())
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_primary)
                                        .number_of_lines(1u32),
                                )
                                .into_element(),
                        );

                        if expanded {
                            let tree = build_project_tree(&repo_files);
                            render_project_dir(
                                &tree,
                                &repo.id,
                                &repo.path,
                                1,
                                force_expand,
                                &expanded_dirs,
                                &located_key,
                                &status_map,
                                &dirty_dirs,
                                p,
                                &mut rows,
                            );
                        }
                    }

                    view().flex_col().py(px(4.0)).children(rows).into_element()
                })),
        )
}

#[allow(clippy::too_many_arguments)]
fn render_project_dir(
    node: &ProjectDir,
    repo_id: &str,
    repo_path: &str,
    depth: usize,
    force_expand: bool,
    expanded_dirs: &HashSet<String>,
    located_key: &Option<String>,
    status_map: &HashMap<String, String>,
    dirty_dirs: &HashSet<String>,
    p: Palette,
    out: &mut Vec<AnyElement>,
) {
    let indent = 12.0 + (depth as f32) * 14.0;
    for sub in node.subdirs.values() {
        let dir_key = format!("{repo_id}::{}", sub.rel_path);
        let expanded = force_expand || expanded_dirs.contains(&dir_key);
        let selected = located_key.as_deref() == Some(dir_key.as_str());
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
                .h(px(24.0))
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
                    let items = build_location_menu_items(
                        cm_rid.clone(),
                        cm_rpath.clone(),
                        cm_rel.clone(),
                        cm_abs.clone(),
                        1,
                        true,
                    );
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
                        .text_size(px(11.5))
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
                status_map,
                dirty_dirs,
                p,
                out,
            );
        }
    }

    for wf in &node.files {
        let key = wf.key();
        let selected = located_key.as_deref() == Some(key.as_str());
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
                .h(px(24.0))
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
                    app_signal().update(|s| {
                        s.project_located_key = Some(f.key());
                        s.project_selected_file = Some(f.clone());
                        s.project_selected_dir = None;
                        s.focus_zone = FocusZone::ProjectTree;
                    });
                    if e.clicks >= 2 {
                        open_code_file(f, None, true);
                    }
                })
                .on_context_menu(move |e: &ContextMenuEvent| {
                    let f = wf_cm.clone();
                    let f_open = f.clone();
                    let mut items = vec![
                        MenuItem::new("打开源码").on_select(move || {
                            open_code_file(f_open.clone(), None, true);
                        }),
                        MenuItem::new("复制").on_select({
                            let name = basename(&f.rel_path);
                            move || set_clipboard(name.clone())
                        }),
                        MenuItem::separator(),
                    ];
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
                .child(icons::icon(ICO_FILE, 11.5, text_col))
                .child(
                    text(base)
                        .text_size(px(11.5))
                        .text_color(text_col)
                        .number_of_lines(1u32),
                )
                .into_element(),
        );
    }
}
