use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use nsh_core::element::desktop::{MenuItem, popup_menu};
use nsh_core::prelude::*;

use crate::git::{RepoInfo, WorkspaceFile};
use crate::paths::basename;
use crate::state::{
    self, FocusZone, LeftTab, WorkspaceData, app_signal, current_palette, open_code_file, open_code_file_preview,
};
use crate::theme::{Palette, status_text_color};
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};
use crate::workspace_ops;

use super::left_tool_window::{ToolWindow, button, tree_scroll, workspace_notice};

const FULL_PROJECT_HEADER: f32 = 172.0;

fn workspace_root() -> PathBuf {
    app_signal().with_untracked(|s| PathBuf::from(&s.cfg.workspace_path))
}

fn new_menu_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("文件…").on_select(|| super::workspace_menu::new_project_item(false)),
        MenuItem::new("目录…").on_select(|| super::workspace_menu::new_project_item(true)),
    ]
}

fn project_options(width: f32) -> Vec<MenuItem> {
    let (follow_editor, has_directory, show_ignored) = app_signal().with_untracked(|s| {
        (s.cfg.project_follow_editor, s.project_selected_dir.is_some(), s.cfg.project_show_ignored)
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
        MenuItem::new("显示被忽略的文件")
            .checked(show_ignored)
            .on_select(|| {
                app_signal().update(|s| {
                    s.cfg.project_show_ignored = !s.cfg.project_show_ignored;
                });
                state::save_config_quiet();
            }),
    ])];
    if width < FULL_PROJECT_HEADER {
        items.extend([
            MenuItem::separator(),
            MenuItem::submenu("新建", new_menu_items()),
            MenuItem::new("定位当前编辑文件").on_select(state::locate_current_in_project_tree),
            MenuItem::new("展开选中目录").disabled(!has_directory).on_select(expand_selected),
            MenuItem::new("全部折叠").on_select(collapse_all),
        ]);
    }
    items.push(MenuItem::separator());
    items.push(MenuItem::new("从磁盘重新加载").shortcut("alt+mod+y").on_select(reload_from_disk));
    items.push(MenuItem::submenu("仓库管理", super::workspace_menu::repository_items()));
    items
}

fn reload_from_disk() {
    workspace_ops::clear_ignore_cache();
    app_signal().update(|_| {});
    state::refresh_workspace_async(None);
}

fn expand_selected() {
    app_signal().update(|s| {
        let Some((_, directory)) = &s.project_selected_dir else {
            return;
        };
        let directory = directory.clone();
        s.project_workspace_expanded = true;
        let dir_path = Path::new(&directory);
        for parent in dir_path.ancestors().take_while(|path| !path.as_os_str().is_empty()) {
            s.expanded_project_dirs.insert(crate::paths::normalize_slashes(&parent.to_string_lossy()));
        }
        let ws_root = PathBuf::from(&s.cfg.workspace_path);
        let mut add = Vec::new();
        for file in s.workspace_files.iter() {
            let Some(rel) = workspace_ops::workspace_rel_path(&ws_root, Path::new(&file.abs_path)) else {
                continue;
            };
            let path = Path::new(&rel);
            if path.starts_with(dir_path) {
                for parent in path.ancestors().skip(1).take_while(|parent| !parent.as_os_str().is_empty()) {
                    add.push(crate::paths::normalize_slashes(&parent.to_string_lossy()));
                }
            }
        }
        s.expanded_project_dirs.extend(add);
    });
}

fn collapse_all() {
    app_signal().update(|s| {
        s.expanded_project_dirs.clear();
        s.project_workspace_expanded = false;
        s.focus_zone = FocusZone::ProjectTree;
    });
}

fn toggle_dir(s: &mut state::AppState, rel: &str, is_root: bool) {
    if is_root {
        s.project_workspace_expanded = !s.project_workspace_expanded;
    } else if !s.expanded_project_dirs.remove(rel) {
        s.expanded_project_dirs.insert(rel.to_owned());
    }
}

// ============================================================================
// 文件操作：剪切 / 复制 / 粘贴 / 重命名 / 删除
// ============================================================================

fn set_project_clipboard(path: &Path, cut: bool) {
    let path = path.to_path_buf();
    app_signal().update(|s| {
        s.project_clipboard = Some(state::ProjectClipboard { path: path.clone(), cut });
    });
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        set_clipboard(name.to_owned());
    }
    state::show_toast("ok", if cut { "已剪切" } else { "已复制" });
}

/// 当前选中的文件系统节点（不含工作区根目录）。
fn selected_entry() -> Option<PathBuf> {
    app_signal().with_untracked(|s| {
        let root = PathBuf::from(&s.cfg.workspace_path);
        let path = s.project_selected_path.clone()?;
        (!workspace_ops::path_eq(&path, &root)).then_some(path)
    })
}

fn selected_target_dir() -> PathBuf {
    let root = workspace_root();
    let Some(path) = app_signal().with_untracked(|s| s.project_selected_path.clone()) else {
        return root;
    };
    if path.is_dir() {
        path
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or(root)
    }
}

pub(super) fn cut_selected() {
    if let Some(path) = selected_entry() {
        set_project_clipboard(&path, true);
    }
}

pub(super) fn paste_selected() {
    paste_project_entry(selected_target_dir());
}

pub(super) fn rename_selected() {
    if let Some(path) = selected_entry() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        rename_project_entry(path, name);
    }
}

pub(super) fn delete_selected() {
    if let Some(path) = selected_entry() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        delete_project_entry(path, name);
    }
}

fn entry_menu(path: &Path, is_dir: bool, is_root: bool, clipboard_available: bool) -> Vec<MenuItem> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    let paste_dir = if is_dir {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let copy_path = path.to_path_buf();
    let cut_path = path.to_path_buf();
    let rename_path = path.to_path_buf();
    let rename_name = name.clone();
    let delete_path = path.to_path_buf();
    let mut items = vec![
        MenuItem::new("剪切")
            .shortcut("mod+x")
            .disabled(is_root)
            .on_select(move || set_project_clipboard(&cut_path, true)),
        MenuItem::new("复制")
            .shortcut("mod+c")
            .disabled(is_root)
            .on_select(move || set_project_clipboard(&copy_path, false)),
        MenuItem::new("粘贴")
            .shortcut("mod+v")
            .disabled(!clipboard_available)
            .on_select(move || paste_project_entry(paste_dir.clone())),
    ];
    if !is_root {
        items.push(MenuItem::separator());
        items.push(
            MenuItem::new("重命名…")
                .shortcut("shift+f6")
                .on_select(move || rename_project_entry(rename_path.clone(), rename_name.clone())),
        );
        items.push(
            MenuItem::new("删除")
                .shortcut("backspace")
                .on_select(move || delete_project_entry(delete_path.clone(), name.clone())),
        );
    }
    items
}

fn rename_project_entry(path: PathBuf, current_name: String) {
    super::git_prompt::open("重命名", current_name, move |new_name| {
        let new_name = new_name.trim().to_owned();
        anyhow::ensure!(!new_name.is_empty(), "名称不能为空");
        let path = path.clone();
        let workspace = workspace_root();
        anyhow::ensure!(
            state::run_git_action(
                move || workspace_ops::rename_project_item(&workspace, &path, &new_name),
                move |result| match result {
                    Ok(new_path) => {
                        let is_dir = new_path.is_dir();
                        app_signal().update(|s| {
                            state::select_project_fs_node(s, &new_path, is_dir);
                            s.project_reveal_request = s.project_reveal_request.wrapping_add(1);
                        });
                        state::show_toast("ok", "已重命名");
                    }
                    Err(err) => state::show_toast("err", err.to_string()),
                },
            ),
            "其他操作正在运行"
        );
        Ok(())
    });
}

fn delete_project_entry(path: PathBuf, name: String) {
    let workspace = workspace_root();
    let parent = path.parent().map(Path::to_path_buf);
    super::git_prompt::confirm(format!("删除 {name}？此操作不可撤销"), move || {
        let workspace = workspace.clone();
        let path = path.clone();
        let parent = parent.clone();
        if !state::run_git_action(
            move || workspace_ops::delete_project_item(&workspace, &path),
            move |result| match result {
                Ok(()) => {
                    app_signal().update(|s| {
                        if let Some(parent) = &parent {
                            state::select_project_fs_node(s, parent, true);
                        }
                        s.project_reveal_request = s.project_reveal_request.wrapping_add(1);
                    });
                    state::show_toast("ok", "已删除");
                }
                Err(err) => state::show_toast("err", err.to_string()),
            },
        ) {
            state::show_toast("warn", "其他操作正在运行");
        }
    });
}

fn paste_project_entry(dest: PathBuf) {
    let Some(clipboard) = app_signal().with_untracked(|s| s.project_clipboard.clone()) else {
        return;
    };
    let workspace = workspace_root();
    let cut = clipboard.cut;
    let source = clipboard.path;
    if !state::run_git_action(
        move || {
            if cut {
                workspace_ops::move_project_item(&workspace, &source, &dest)
            } else {
                workspace_ops::copy_project_item(&workspace, &source, &dest)
            }
        },
        move |result| match result {
            Ok(new_path) => {
                let is_dir = new_path.is_dir();
                app_signal().update(|s| {
                    if cut {
                        s.project_clipboard = None;
                    }
                    if let Some(parent) = new_path.parent()
                        && let Some(rel) = workspace_ops::workspace_rel_path(Path::new(&s.cfg.workspace_path), parent)
                        && !rel.is_empty()
                    {
                        s.expanded_project_dirs.insert(rel);
                    }
                    state::select_project_fs_node(s, &new_path, is_dir);
                    s.project_reveal_request = s.project_reveal_request.wrapping_add(1);
                });
                state::show_toast("ok", if cut { "已移动" } else { "已复制" });
            }
            Err(err) => state::show_toast("err", err.to_string()),
        },
    ) {
        state::show_toast("warn", "其他操作正在运行");
    }
}

/// 路径所属仓库信息：`(repo_id, repo_path, 仓库内相对路径)`；不在任何仓库内时回落到工作区。
fn location_for(abs: &Path) -> (String, String, String) {
    app_signal().with_untracked(|s| {
        if let Some(repo) = workspace_ops::find_owning_repo(&s.repos, abs) {
            let rel = abs
                .strip_prefix(&repo.path)
                .ok()
                .map(|p| crate::paths::normalize_slashes(&p.to_string_lossy()))
                .unwrap_or_default();
            (repo.id.clone(), repo.path.clone(), rel)
        } else {
            let rel = workspace_ops::workspace_rel_path(Path::new(&s.cfg.workspace_path), abs).unwrap_or_default();
            (String::new(), s.cfg.workspace_path.clone(), rel)
        }
    })
}

fn git_repo_items(repo: &RepoInfo) -> Vec<MenuItem> {
    let mut items = super::sync_dialog::menu(Some(repo.clone()));
    items.push(MenuItem::separator());
    items.push(MenuItem::new("分支管理…").on_select(super::branch_matrix::open));
    items.push(MenuItem::submenu("Git 高级", super::git_advanced::repo_items(repo.clone())));
    if super::mani_panel::enabled() {
        let path = PathBuf::from(&repo.path);
        items.push(MenuItem::new("运行 Mani 任务…").on_select(move || super::mani_panel::open_project(path.clone())));
    }
    items
}

// ============================================================================
// 树渲染
// ============================================================================

#[derive(Clone)]
struct Child {
    name: String,
    rel: String,
    abs: PathBuf,
    is_dir: bool,
    ignored: bool,
}

struct Ctx {
    ws_root: PathBuf,
    repos: Vec<RepoInfo>,
    expanded: HashSet<String>,
    workspace_expanded: bool,
    located: (Option<String>, Vec<String>),
    located_row: nsh_core::element::ElementRef,
    status_map: HashMap<String, String>,
    dirty_dirs: HashSet<String>,
    repo_status: HashMap<String, (String, usize, usize)>,
    show_ignored: bool,
    /// 过滤模式或磁盘读取失败时，使用已发现的文件列表构造的目录内容。
    virtual_children: Option<HashMap<String, Vec<Child>>>,
    force_expand: bool,
    p: Palette,
    font_size: f32,
}

fn virtual_children(ws_root: &Path, files: &[WorkspaceFile], filter: &str) -> HashMap<String, Vec<Child>> {
    let mut map: HashMap<String, BTreeMap<(bool, String), Child>> = HashMap::new();
    for file in files {
        let abs = PathBuf::from(&file.abs_path);
        let Some(rel) = workspace_ops::workspace_rel_path(ws_root, &abs) else {
            continue;
        };
        if rel.is_empty() || (!filter.is_empty() && !rel.to_ascii_lowercase().contains(filter)) {
            continue;
        }
        let mut current = rel.as_str();
        let mut is_dir = false;
        loop {
            let (parent, name) = current.rsplit_once('/').unwrap_or(("", current));
            let child_abs = ws_root.join(current);
            map.entry(parent.to_owned()).or_default().insert(
                (!is_dir, name.to_ascii_lowercase()),
                Child { name: name.to_owned(), rel: current.to_owned(), abs: child_abs, is_dir, ignored: false },
            );
            if parent.is_empty() {
                break;
            }
            current = parent;
            is_dir = true;
        }
    }
    map.into_iter().map(|(k, v)| (k, v.into_values().collect())).collect()
}

fn children_of(ctx: &Ctx, rel: &str, abs: &Path) -> Vec<Child> {
    if let Some(map) = &ctx.virtual_children {
        return map.get(rel).cloned().unwrap_or_default();
    }
    workspace_ops::read_dir_entries(&ctx.ws_root, abs, &ctx.repos)
        .into_iter()
        .filter(|entry| !entry.ignored || ctx.show_ignored)
        .map(|entry| Child {
            name: entry.name,
            rel: entry.rel_path,
            abs: entry.abs_path,
            is_dir: entry.is_dir,
            ignored: entry.ignored,
        })
        .collect()
}

fn ignored_color(p: Palette) -> Hsla {
    p.amber.opacity(if p.is_dark { 0.7 } else { 0.78 })
}

struct DirRow {
    rel: String,
    abs: PathBuf,
    name: String,
    depth: usize,
    ignored: bool,
    is_root: bool,
    expanded: bool,
    repo: Option<RepoInfo>,
}

fn push_dir_row(ctx: &Ctx, d: DirRow, out: &mut Vec<AnyElement>) {
    let p = ctx.p;
    let font_size = ctx.font_size;
    let selected = ctx.located.0.as_deref() == Some(d.rel.as_str());
    let dirty = !d.is_root && ctx.dirty_dirs.contains(&d.rel);
    let folder_col = if d.ignored {
        ignored_color(p)
    } else if dirty {
        p.blue
    } else {
        p.text_control
    };
    let name_col = if d.ignored { ignored_color(p) } else { p.text_primary };
    let git = d.repo.as_ref().and_then(|repo| ctx.repo_status.get(&repo.id).cloned());
    let indent = 8.0 + (d.depth as f32) * 14.0;
    let force = ctx.force_expand;
    let hint = d.is_root.then(|| crate::paths::display_path(&ctx.ws_root.to_string_lossy()));
    let test_id = if d.is_root { "project-workspace-root".to_owned() } else { format!("project-dir-{}", d.rel) };
    let toggle_id = if d.is_root { "project-root-toggle".to_owned() } else { format!("project-dir-toggle-{}", d.rel) };

    let (abs_press, rel_press, root_press) = (d.abs.clone(), d.rel.clone(), d.is_root);
    let (abs_menu, rel_menu, root_menu, repo_menu) = (d.abs.clone(), d.rel.clone(), d.is_root, d.repo.clone());
    let (rel_chev, root_chev) = (d.rel.clone(), d.is_root);

    out.push(
        view()
            .test_id(&test_id)
            .when(selected, |row| row.element_ref(&ctx.located_row))
            .h(px(font_size + 12.5))
            .pl(px(indent))
            .pr(px(8.0))
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .bg(if selected { p.active_row_bg } else { transparent_black() })
            .cursor(Cursor::Pointer)
            .hover(move |s| if selected { s } else { s.bg(p.surface_hover) })
            .on_press(move |e: &PressEvent| {
                app_signal().update(|s| {
                    state::select_project_fs_node(s, &abs_press, true);
                    s.focus_zone = FocusZone::ProjectTree;
                    // 工作区根目录单击即切换；其他目录沿用 IDEA 的双击切换。
                    if !force && ((root_press && e.clicks == 1) || (!root_press && e.clicks >= 2)) {
                        toggle_dir(s, &rel_press, root_press);
                    }
                });
            })
            .on_context_menu(move |e: &ContextMenuEvent| {
                app_signal().update(|s| {
                    state::select_project_fs_node(s, &abs_menu, true);
                    s.focus_zone = FocusZone::ProjectTree;
                });
                let clipboard = app_signal().with_untracked(|s| s.project_clipboard.is_some());
                let mut items = vec![super::workspace_menu::new_item_menu(), MenuItem::separator()];
                items.extend(entry_menu(&abs_menu, true, root_menu, clipboard));
                items.push(MenuItem::separator());
                let search_dir = abs_menu.to_string_lossy().to_string();
                items.push(
                    MenuItem::new("在目录中查找")
                        .shortcut("mod+shift+f")
                        .on_select(move || crate::ui::content_search::open_for_directory(search_dir.clone())),
                );
                let terminal_dir = abs_menu.clone();
                items.push(
                    MenuItem::new("在终端中打开")
                        .on_select(move || super::terminal_panel::open_in_directory(terminal_dir.clone())),
                );
                let (repo_id, repo_path, repo_rel) = location_for(&abs_menu);
                let _ = &rel_menu;
                items.extend(build_location_menu_items(
                    repo_id,
                    repo_path,
                    repo_rel,
                    abs_menu.to_string_lossy().to_string(),
                    1,
                    true,
                ));
                if let Some(repo) = &repo_menu {
                    items.push(MenuItem::separator());
                    items.push(MenuItem::submenu("Git", git_repo_items(repo)));
                }
                items.push(MenuItem::separator());
                items.push(MenuItem::new("从磁盘重新加载").shortcut("alt+mod+y").on_select(reload_from_disk));
                state::show_popup_menu_at(items, e.page_position);
            })
            .child(
                super::controls::button()
                    .test_id(&toggle_id)
                    .w(px(14.0))
                    .h(px(16.0))
                    .on_press(move || {
                        app_signal().update(|s| toggle_dir(s, &rel_chev, root_chev));
                    })
                    .child(icons::icon(
                        if d.expanded { ICO_CHEVRON_DOWN } else { ICO_CHEVRON_RIGHT },
                        10.0,
                        p.text_muted,
                    )),
            )
            .child(icons::icon(ICO_FOLDER, 12.0, folder_col))
            .child(
                text(d.name.clone())
                    .flex_none()
                    .text_size(px(font_size))
                    .font_weight(if d.is_root { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                    .text_color(name_col)
                    .number_of_lines(1u32),
            )
            .child(git.map(|(branch, ahead, behind)| {
                view()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        text(branch)
                            .text_size(px(font_size - 1.0))
                            .text_color(p.text_muted)
                            .number_of_lines(1u32),
                    )
                    .child((ahead > 0).then(|| {
                        text(format!("↑{ahead}")).text_size(px(font_size - 1.0)).text_color(p.green)
                    }))
                    .child((behind > 0).then(|| {
                        text(format!("↓{behind}")).text_size(px(font_size - 1.0)).text_color(p.blue)
                    }))
            }))
            .child(hint.map(|hint| {
                text(hint)
                    .text_size(px(font_size - 1.0))
                    .text_color(p.text_muted)
                    .number_of_lines(1u32)
            }))
            .into_element(),
    );
}

fn push_file_row(ctx: &Ctx, child: &Child, depth: usize, out: &mut Vec<AnyElement>) {
    let p = ctx.p;
    let font_size = ctx.font_size;
    let selected = ctx.located.0.as_deref() == Some(child.rel.as_str()) || ctx.located.1.contains(&child.rel);
    let text_col = if child.ignored {
        ignored_color(p)
    } else {
        ctx.status_map
            .get(&child.rel)
            .map(|kind| status_text_color(kind, p))
            .unwrap_or(if selected { p.text_primary } else { p.text_secondary })
    };
    let indent = 8.0 + (depth as f32) * 14.0 + 19.0;
    let wf = workspace_ops::resolve_workspace_file(&ctx.ws_root, &ctx.repos, &child.abs);
    let abs_press = child.abs.clone();
    let abs_menu = child.abs.clone();
    let rel_press = child.rel.clone();
    let rel_menu = child.rel.clone();
    let wf_press = wf.clone();
    let wf_menu = wf.clone();
    let ignored = child.ignored;

    out.push(
        view()
            .test_id(&format!("project-file-{}", child.rel))
            .when(ctx.located.0.as_deref() == Some(child.rel.as_str()), |row| row.element_ref(&ctx.located_row))
            .h(px(font_size + 12.5))
            .pl(px(indent))
            .pr(px(8.0))
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .bg(if selected { p.active_row_bg } else { transparent_black() })
            .cursor(Cursor::Pointer)
            .hover(move |s| if selected { s } else { s.bg(p.surface_hover) })
            .on_press(move |e: &PressEvent| {
                let extended = if apple_keys() { e.modifiers.cmd } else { e.modifiers.ctrl };
                app_signal().update(|s| {
                    if extended {
                        if let Some(index) = s.project_compare_keys.iter().position(|key| *key == rel_press) {
                            s.project_compare_keys.remove(index);
                            let ws_root = PathBuf::from(&s.cfg.workspace_path);
                            s.project_selected_file = s.project_compare_keys.last().and_then(|key| {
                                workspace_ops::resolve_workspace_file(&ws_root, &s.repos, &ws_root.join(key))
                            });
                        } else {
                            s.project_compare_keys.push(rel_press.clone());
                            s.project_selected_file = wf_press.clone();
                        }
                        s.project_located_key = s.project_compare_keys.last().cloned();
                        s.project_selected_dir = None;
                        s.project_selected_path = s
                            .project_selected_file
                            .as_ref()
                            .map(|file| PathBuf::from(&file.abs_path))
                            .or_else(|| Some(abs_press.clone()));
                    } else {
                        state::select_project_fs_node(s, &abs_press, false);
                    }
                    s.focus_zone = FocusZone::ProjectTree;
                });
                if !extended && e.clicks >= 2 && let Some(file) = wf_press.clone() {
                    open_code_file(file, None, true);
                }
            })
            .on_context_menu(move |e: &ContextMenuEvent| {
                app_signal().update(|s| {
                    if !s.project_compare_keys.contains(&rel_menu) {
                        state::select_project_fs_node(s, &abs_menu, false);
                    } else {
                        s.project_located_key = Some(rel_menu.clone());
                        s.project_selected_path = Some(abs_menu.clone());
                        s.project_selected_dir = None;
                        s.project_selected_file = wf_menu.clone();
                    }
                    s.focus_zone = FocusZone::ProjectTree;
                });
                let clipboard = app_signal().with_untracked(|s| s.project_clipboard.is_some());
                let mut items = Vec::new();
                if let Some(file) = &wf_menu {
                    let (open, preview) = (file.clone(), file.clone());
                    items.push(MenuItem::new("打开源码").on_select(move || open_code_file(open.clone(), None, true)));
                    items.push(MenuItem::new("预览").on_select(move || open_code_file_preview(preview.clone())));
                    items.push(MenuItem::separator());
                }
                items.extend(entry_menu(&abs_menu, false, false, clipboard));
                items.push(MenuItem::separator());
                let marked = app_signal().with_untracked(|s| {
                    s.project_compare_keys.len() == 2 && s.project_compare_keys.contains(&rel_menu)
                });
                if marked {
                    items.push(MenuItem::new("比较选中的文件").on_select(|| {
                        super::comparison::open_project_selection();
                    }));
                }
                let (repo_id, repo_path, repo_rel) = location_for(&abs_menu);
                items.extend(build_location_menu_items(
                    repo_id,
                    repo_path,
                    repo_rel,
                    abs_menu.to_string_lossy().to_string(),
                    1,
                    false,
                ));
                if let Some(file) = &wf_menu
                    && !ignored
                    && app_signal().with_untracked(|s| s.repos.iter().any(|repo| repo.id == file.repo_id))
                {
                    let (history, revision) = (file.clone(), file.clone());
                    items.push(MenuItem::separator());
                    items.push(MenuItem::submenu("Git", [
                        MenuItem::new("查看文件历史")
                            .on_select(move || super::revision_picker::show_history(history.clone())),
                        MenuItem::new("与修订版本比较…")
                            .on_select(move || super::revision_picker::open(revision.clone())),
                    ]));
                }
                state::show_popup_menu_at(items, e.page_position);
            })
            .child(icons::file_icon(&child.name, 12.0, p))
            .child(
                text(child.name.clone())
                    .flex_none()
                    .text_size(px(font_size))
                    .text_color(text_col)
                    .number_of_lines(1u32),
            )
            .into_element(),
    );
}

fn render_children(ctx: &Ctx, rel: &str, abs: &Path, depth: usize, out: &mut Vec<AnyElement>) {
    for child in children_of(ctx, rel, abs) {
        if child.is_dir {
            let expanded = ctx.force_expand || ctx.expanded.contains(&child.rel);
            let repo = abs_repo(ctx, &child.abs);
            push_dir_row(
                ctx,
                DirRow {
                    rel: child.rel.clone(),
                    abs: child.abs.clone(),
                    name: child.name.clone(),
                    depth,
                    ignored: child.ignored,
                    is_root: false,
                    expanded,
                    repo,
                },
                out,
            );
            if expanded {
                render_children(ctx, &child.rel, &child.abs, depth + 1, out);
            }
        } else {
            push_file_row(ctx, &child, depth, out);
        }
    }
}

/// 目录本身是仓库根时返回该仓库（目录内有 `.git`）。
fn abs_repo(ctx: &Ctx, abs: &Path) -> Option<RepoInfo> {
    if !abs.join(".git").exists() {
        return None;
    }
    workspace_ops::find_exact_repo(&ctx.repos, abs).cloned()
}

pub fn project_pane() -> impl IntoElement {
    let tool = ToolWindow::new(LeftTab::Project);
    let located_row = element_ref();
    let reveal_request = computed(|| app_signal().with(|s| s.project_reveal_request));
    let filter = computed(|| app_signal().with(|s| s.project_filter.trim().to_ascii_lowercase()));
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
            .child(button("project-new", "新建", ICO_PLUS, p, false, true, || {
                let items = new_menu_items();
                state::spawn_from_root(async move {
                    popup_menu(items).await;
                });
            }))
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
                    let filter_text = filter.get();
                    let (ctx, has_notice) = app_signal().with(|s| {
                        let ws_root = PathBuf::from(&s.cfg.workspace_path);
                        let has_notice = s.workspace_notice(WorkspaceData::Files).is_some();
                        let mut status_map: HashMap<String, String> = HashMap::new();
                        let mut dirty_dirs: HashSet<String> = HashSet::new();
                        let mut repo_status = HashMap::new();
                        for st in &s.statuses {
                            repo_status.insert(st.repo.id.clone(), (st.branch.clone(), st.ahead, st.behind));
                            for cf in &st.files {
                                let abs = Path::new(&st.repo.path).join(cf.path.trim_end_matches('/'));
                                let Some(rel) = workspace_ops::workspace_rel_path(&ws_root, &abs) else {
                                    continue;
                                };
                                let mut parent = Path::new(&rel).parent();
                                while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
                                    dirty_dirs.insert(crate::paths::normalize_slashes(&dir.to_string_lossy()));
                                    parent = dir.parent();
                                }
                                status_map.insert(rel, cf.kind.clone());
                            }
                        }
                        let force_expand = !filter_text.is_empty();
                        let use_fs = !force_expand && !has_notice;
                        let virtual_children = (!use_fs)
                            .then(|| virtual_children(&ws_root, &s.workspace_files, &filter_text));
                        (
                            Ctx {
                                repos: s.repos.clone(),
                                expanded: s.expanded_project_dirs.clone(),
                                workspace_expanded: s.project_workspace_expanded,
                                located: (s.project_located_key.clone(), s.project_compare_keys.clone()),
                                located_row,
                                status_map,
                                dirty_dirs,
                                repo_status,
                                show_ignored: s.cfg.project_show_ignored,
                                virtual_children,
                                force_expand,
                                p,
                                font_size,
                                ws_root,
                            },
                            has_notice,
                        )
                    });

                    if ctx.ws_root.as_os_str().is_empty() || !ctx.ws_root.is_dir() {
                        return view()
                            .py(px(if has_notice { 0.0 } else { 32.0 }))
                            .items_center()
                            .child((!has_notice).then(|| text("未打开项目").text_size(px(12.0)).text_color(p.text_muted)))
                            .into_element();
                    }

                    let name = basename(&ctx.ws_root.to_string_lossy());
                    let expanded = ctx.force_expand || ctx.workspace_expanded;
                    let root_repo = abs_repo(&ctx, &ctx.ws_root);
                    let mut rows: Vec<AnyElement> = Vec::new();
                    push_dir_row(
                        &ctx,
                        DirRow {
                            rel: String::new(),
                            abs: ctx.ws_root.clone(),
                            name: if name.is_empty() { "工作区".into() } else { name },
                            depth: 0,
                            ignored: false,
                            is_root: true,
                            expanded,
                            repo: root_repo,
                        },
                        &mut rows,
                    );
                    if expanded {
                        render_children(&ctx, "", &ctx.ws_root, 1, &mut rows);
                    }
                    view().flex_col().py(px(4.0)).children(rows).into_element()
                }),
            ),
        ))
}
