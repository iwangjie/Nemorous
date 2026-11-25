use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use nsh_base::{menu, menu_item, menu_separator};
use nsh_core::element::desktop::{MenuItem, current_window};
use nsh_core::file_dialog::OpenDialog;
use nsh_core::prelude::*;

use super::icons::*;
use crate::state::{self, AppState, EditorTabKind, FocusZone, LeftTab, app_signal};
use crate::theme::Palette;
use crate::workspace_ops;

fn current_workspace() -> String {
    app_signal().with_untracked(|s| s.cfg.workspace_path.clone())
}

fn choose_folder(title: &'static str, selected: impl FnOnce(PathBuf) + 'static) {
    let dialog = OpenDialog::folders().title(title).button_label("选择");
    state::spawn_from_root(async move {
        match pick_paths(dialog).await {
            Ok(Some(paths)) => {
                if let Some(first) = paths.into_iter().next() {
                    selected(first);
                }
            }
            Ok(None) => {}
            Err(err) => state::show_toast("err", format!("无法打开目录选择器：{err}")),
        }
    });
}

fn switch_workspace(path: &Path) -> Result<()> {
    let path = workspace_ops::workspace_path(path)?;
    let path = path.to_str().context("无法打开工作区路径")?;
    set_workspace(path)
}

fn set_workspace(path: &str) -> Result<()> {
    if current_workspace() == path {
        return Ok(());
    }
    let (busy, dirty) = app_signal().with_untracked(|s| {
        (
            s.command_busy,
            s.code_tabs.iter().any(|tab| tab.is_dirty())
                || s.working_diff.as_ref().is_some_and(|diff| diff.is_dirty()),
        )
    });
    ensure!(!busy, "请等待当前 Git 操作结束");
    ensure!(!super::github_panel::submitting(), "请等待 GitHub 提交完成");
    ensure!(!dirty, "请先保存当前工作区的未保存编辑");
    super::github_panel::close_active();
    crate::session_runtime::save_current_workspace();
    crate::session_runtime::workspace_changed();
    app_signal().update(|s| {
        s.stop_workspace_refresh();
        let mut cfg = s.cfg.clone();
        cfg.add_recent_workspace(&s.cfg.workspace_path);
        cfg.workspace_path = path.to_owned();
        cfg.add_recent_workspace(path);
        let refresh_gen = s.refresh_gen.wrapping_add(1);
        *s = AppState::for_config(cfg);
        s.refresh_gen = refresh_gen;
        if !path.is_empty() {
            s.left_tab = LeftTab::Project;
            s.focus_zone = FocusZone::ProjectTree;
        }
    });
    state::save_config_quiet();
    state::refresh_workspace_async(None);
    Ok(())
}

fn show_result(outcome: Result<()>) {
    if let Err(err) = outcome {
        state::show_toast("warn", err.to_string());
    }
}

pub(super) fn open_in_this_window(path: &Path) {
    show_result(switch_workspace(path));
}

pub(super) fn close_project() {
    let dirty = app_signal().with_untracked(|s| {
        s.code_tabs.iter().any(|tab| tab.is_dirty())
            || s.working_diff.as_ref().is_some_and(|diff| diff.is_dirty())
    });
    if dirty {
        super::git_prompt::confirm_after_close("保存全部编辑并关闭项目？", || {
            crate::format_runtime::save_all_then(|| show_result(set_workspace("")));
        });
    } else {
        show_result(set_workspace(""));
    }
}

fn focus_project(path: &str) -> bool {
    if current_workspace() == path {
        current_window().focus();
        true
    } else {
        super::workspace_windows::focus(path)
    }
}

pub(super) fn open_project(path: &Path) {
    let path = match workspace_ops::workspace_path(path) {
        Ok(path) => path,
        Err(err) => return show_result(Err(err)),
    };
    if focus_project(&path.to_string_lossy()) {
        return;
    }
    if current_workspace().is_empty() {
        open_in_this_window(&path);
        return;
    }
    let current = path.clone();
    super::git_prompt::choose(
        format!(
            "打开项目 · {}",
            crate::paths::basename(&path.to_string_lossy())
        ),
        "此窗口",
        move || switch_workspace(&current),
        "新窗口",
        move || change_workspace_in_new_window(&path),
    );
}

pub(super) fn choose_project() {
    choose_folder("打开项目", |path| open_project(&path));
}

fn finish_project_task(result: Result<PathBuf>) {
    match result {
        Ok(path) => open_project(&path),
        Err(err) => show_result(Err(err)),
    }
}

pub(super) fn new_project() {
    choose_folder("选择项目的上一级目录", |parent| {
        super::git_prompt::open_with_option(
            "新建项目 · 名称",
            String::new(),
            Some("创建 Git 仓库"),
            move |name, git| {
                workspace_ops::new_item_path(&parent, name)?;
                let parent = parent.clone();
                let name = name.to_owned();
                let branch =
                    git.then(|| app_signal().with_untracked(|s| s.cfg.default_base_branch.clone()));
                ensure!(
                    state::run_git_action(
                        move || workspace_ops::create_project(&parent, &name, branch.as_deref()),
                        finish_project_task,
                    ),
                    "其他操作正在运行"
                );
                Ok(())
            },
        );
    });
}

pub(super) fn clone_project() {
    super::git_prompt::open(
        "克隆仓库 · Git URL 或本地路径",
        String::new(),
        |source| {
            let name = clone_name(source).to_owned();
            ensure!(!name.is_empty(), "请输入 Git URL 或本地路径");
            let source = source.to_owned();
            choose_folder("选择克隆位置的上一级目录", move |parent| {
                super::git_prompt::open("克隆仓库 · 目录名", name, move |name| {
                    let (parent, source, name) = (parent.clone(), source.clone(), name.to_owned());
                    ensure!(
                        state::run_git_action(
                            move || workspace_ops::clone_repo(&parent, &source, &name),
                            finish_project_task,
                        ),
                        "其他操作正在运行"
                    );
                    Ok(())
                });
            });
            Ok(())
        },
    );
}

fn clone_name(source: &str) -> &str {
    let name = source
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default();
    name.strip_suffix(".git").unwrap_or(name)
}

fn selected_path(s: &AppState) -> Option<(&str, &str)> {
    if s.focus_zone == FocusZone::ProjectTree {
        if let Some(dir) = &s.project_selected_dir {
            let root = s
                .repos
                .iter()
                .find(|repo| repo.id == dir.0)
                .map(|repo| repo.path.as_str())
                .or_else(|| {
                    s.plain_project()
                        .filter(|repo| repo.id == dir.0)
                        .map(|_| s.cfg.workspace_path.as_str())
                })?;
            return Some((root, &dir.1));
        }
        if let Some(file) = &s.project_selected_file {
            return Some((&file.repo_path, &file.rel_path));
        }
    }
    (s.editor_tab == EditorTabKind::File)
        .then(|| {
            let file = &s.code_tabs.get(s.active_code_tab_idx)?.file;
            Some((file.repo_path.as_str(), file.rel_path.as_str()))
        })
        .flatten()
}

pub(super) fn reveal_project_item() {
    let path = app_signal().with_untracked(|s| {
        selected_path(s).map_or_else(
            || PathBuf::from(&s.cfg.workspace_path),
            |(root, rel)| Path::new(root).join(rel),
        )
    });
    if !path.as_os_str().is_empty() {
        state::reveal_in_system_finder(&path.to_string_lossy());
    }
}

pub(super) fn new_item_menu() -> MenuItem {
    MenuItem::submenu(
        "新建",
        [
            MenuItem::new("文件…").on_select(|| new_project_item(false)),
            MenuItem::new("目录…").on_select(|| new_project_item(true)),
        ],
    )
}

pub(super) fn new_project_item(directory: bool) {
    let target = app_signal().with_untracked(|s| {
        let fallback = || {
            if let Some(root) = s.repos.iter().find(|repo| repo.is_root) {
                return Some(root.path.as_str());
            }
            match s.repos.as_slice() {
                [] if !s.cfg.workspace_path.is_empty() => Some(s.cfg.workspace_path.as_str()),
                [repo] => Some(repo.path.as_str()),
                _ => None,
            }
        };
        let (root, selected) = selected_path(s).or_else(|| fallback().map(|root| (root, "")))?;
        let repo = s
            .repos
            .iter()
            .find(|repo| repo.path == root)
            .cloned()
            .or_else(|| s.plain_project())?;
        let mut parent = Path::new(root).join(selected);
        if parent.is_file() {
            parent.pop();
        }
        Some((PathBuf::from(root), parent, repo))
    });
    let Some((root, parent, repo)) = target else {
        state::show_toast("warn", "请先在文件树选择目标目录");
        return;
    };
    let title = if directory {
        "新建目录 · 名称"
    } else {
        "新建文件 · 名称"
    };
    super::git_prompt::open(title, String::new(), move |name| {
        let (root, parent, name) = (root.clone(), parent.clone(), name.to_owned());
        let repo = repo.clone();
        ensure!(
            state::run_git_action(
                move || workspace_ops::create_project_item(&root, &parent, &name, directory),
                move |result| match result {
                    Ok(path) => {
                        if let Some(file) = workspace_ops::project_file(&repo, &path) {
                            if directory {
                                app_signal().update(|s| s.remember_directory(&repo, file.rel_path));
                                super::focus_tool_window(LeftTab::Project, false);
                            } else {
                                state::open_code_file(file, None, true);
                            }
                        }
                    }
                    Err(err) => show_result(Err(err)),
                },
            ),
            "其他操作正在运行"
        );
        Ok(())
    });
}

fn change_workspace_in_new_window(path: &Path) {
    if let Err(err) = workspace_ops::launch_window(path) {
        state::show_toast("warn", err.to_string());
        return;
    }
    app_signal().update(|s| {
        let current = s.cfg.workspace_path.clone();
        s.cfg.add_recent_workspace(&current);
        s.cfg.add_recent_workspace(&path.to_string_lossy());
    });
    state::save_config_quiet();
}

fn run_repository_task(
    work: impl FnOnce() -> Result<PathBuf> + Send + 'static,
    success: &'static str,
) -> bool {
    state::run_git_action(work, move |result| match result {
        Ok(path) => state::show_toast("ok", format!("{success} {}", path.display())),
        Err(err) => state::show_toast("err", err.to_string()),
    })
}

fn clone_repository() {
    let workspace = current_workspace();
    super::git_prompt::open(
        "克隆仓库 · Git URL 或本地路径",
        String::new(),
        move |source| {
            let name = clone_name(source);
            ensure!(!name.is_empty(), "无法从地址识别仓库目录名");
            let (workspace, name, source) = (
                PathBuf::from(&workspace),
                name.to_owned(),
                source.to_owned(),
            );
            ensure!(
                run_repository_task(
                    move || workspace_ops::clone_repo(&workspace, &source, &name),
                    "已克隆"
                ),
                "其他 Git 操作正在运行"
            );
            Ok(())
        },
    );
}

fn init_repository() {
    let (workspace, branch) = app_signal().with_untracked(|s| {
        (
            PathBuf::from(&s.cfg.workspace_path),
            s.cfg.default_base_branch.clone(),
        )
    });
    super::git_prompt::open("初始化仓库 · 目录名", String::new(), move |name| {
        ensure!(!name.is_empty(), "请输入仓库目录名");
        let (workspace, branch, name) = (workspace.clone(), branch.clone(), name.to_owned());
        ensure!(
            run_repository_task(
                move || workspace_ops::init_repo(&workspace, &name, &branch),
                "已初始化"
            ),
            "其他 Git 操作正在运行"
        );
        Ok(())
    });
}

fn import_repository() {
    let workspace = PathBuf::from(current_workspace());
    choose_folder("导入 Git 仓库", move |repo| {
        run_repository_task(
            move || workspace_ops::import_repo(&workspace, &repo),
            "已导入",
        );
    });
}

pub(super) fn repository_items() -> Vec<MenuItem> {
    let (ready, visible, hidden) = app_signal().with_untracked(|s| {
        let bindings = s.cfg.repository_id_bindings.get(&s.cfg.workspace_path);
        let hidden = s
            .cfg
            .hidden_repos
            .iter()
            .map(|id| {
                let label = s.hidden_repo_labels.get(id).cloned().or_else(|| {
                    let (path, _) = bindings?.iter().find(|(_, bound_id)| *bound_id == id)?;
                    let path = Path::new(path);
                    let workspace = Path::new(&s.cfg.workspace_path);
                    let label_path = path
                        .strip_prefix(workspace)
                        .ok()
                        .or_else(|| path.strip_prefix(workspace.canonicalize().ok()?).ok())
                        .filter(|relative| !relative.as_os_str().is_empty())
                        .unwrap_or(path);
                    Some(crate::paths::display_path(&label_path.to_string_lossy()))
                });
                (id.clone(), label.unwrap_or_else(|| id.clone()))
            })
            .collect::<Vec<_>>();
        (!s.cfg.workspace_path.is_empty(), s.repos.clone(), hidden)
    });
    let mut items = vec![
        MenuItem::new("克隆仓库…")
            .disabled(!ready)
            .on_select(clone_repository),
        MenuItem::new("初始化仓库…")
            .disabled(!ready)
            .on_select(init_repository),
        MenuItem::new("导入现有仓库…")
            .disabled(!ready)
            .on_select(import_repository),
    ];
    if !visible.is_empty() {
        items.push(MenuItem::submenu(
            "隐藏仓库",
            visible.into_iter().map(|repo| {
                MenuItem::new(repo.label().to_owned()).on_select(move || {
                    app_signal().update(|s| {
                        if !s.cfg.is_repo_hidden(&repo.id) {
                            s.cfg.hidden_repos.push(repo.id.clone());
                        }
                    });
                    state::save_config_quiet();
                    state::refresh_workspace_async(None);
                })
            }),
        ));
    }
    if !hidden.is_empty() {
        items.push(MenuItem::submenu(
            "恢复隐藏仓库",
            hidden.into_iter().map(|(id, label)| {
                MenuItem::new(label).on_select(move || {
                    app_signal().update(|s| s.cfg.hidden_repos.retain(|item| item != &id));
                    state::save_config_quiet();
                    state::refresh_workspace_async(None);
                })
            }),
        ));
    }
    items
}

fn action_row(
    label: &'static str,
    icon: &'static str,
    p: Palette,
    action: fn(),
) -> impl IntoElement {
    switcher_row(p)
        .accessibility_label(label)
        .on_select(action)
        .child(
            view()
                .w(px(420.0))
                .h(px(26.0))
                .px(px(8.0))
                .gap(px(8.0))
                .flex_row()
                .items_center()
                .child(super::icons::icon(icon, 13.0, p.text_control))
                .child(text(label).text_size(px(12.0)).text_color(p.text_primary)),
        )
}

fn switcher_row(p: Palette) -> nsh_base::MenuItemElement {
    menu_item()
        .rounded(px(5.0))
        .cursor(Cursor::Pointer)
        .hover(move |s| s.bg(p.surface_hover))
        .focus(move |s| s.bg(p.accent_soft))
        .active(move |s| s.bg(p.accent_soft))
}

fn divider(p: Palette) -> impl IntoElement {
    menu_separator().h(px(1.0)).my(px(4.0)).bg(p.border)
}

fn section(label: &'static str, p: Palette) -> impl IntoElement {
    view().px(px(10.0)).h(px(24.0)).justify_center().child(
        text(label)
            .text_size(px(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.text_muted),
    )
}

fn project_row(path: String, current: bool, p: Palette) -> impl IntoElement {
    let name = crate::paths::basename(&path);
    let label = crate::paths::display_path(&path);
    let initials: String = name.chars().take(2).flat_map(char::to_uppercase).collect();
    switcher_row(p)
        .accessibility_label(name.clone())
        .accessibility_description(label.clone())
        .on_select(move || {
            open_project(Path::new(&path));
        })
        .child(
            view()
                .w(px(420.0))
                .h(px(42.0))
                .px(px(8.0))
                .gap(px(8.0))
                .flex_row()
                .items_center()
                .child(
                    view()
                        .w(px(22.0))
                        .h(px(22.0))
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.0))
                        .bg(p.accent_soft)
                        .child(
                            text(initials)
                                .text_size(px(10.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.accent),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .gap(px(3.0))
                        .flex_col()
                        .child(
                            text(name)
                                .text_size(px(12.0))
                                .text_color(p.text_primary)
                                .number_of_lines(1u32),
                        )
                        .child(
                            text(label)
                                .text_size(px(10.5))
                                .text_color(p.text_muted)
                                .number_of_lines(1u32),
                        ),
                )
                .child(current.then(|| super::icons::icon(ICO_CHECK, 12.0, p.accent))),
        )
}

pub(super) fn toolbar(trigger: impl IntoElement, p: Palette) -> impl IntoElement {
    menu()
        .trigger(trigger)
        .offset(px(4.0))
        .popup_style(move |s| {
            s.bg(p.surface)
                .border_1()
                .border_color(p.border_strong)
                .rounded(px(8.0))
                .p(px(5.0))
                .shadow_lg()
        })
        .child(action_row("新建项目…", ICO_FILE, p, new_project))
        .child(action_row("打开…", ICO_FOLDER, p, choose_project))
        .child(action_row("克隆仓库…", ICO_BRANCH, p, clone_project))
        .child(dynamic(move || {
            let (current, recent) = app_signal().with_untracked(|s| {
                (
                    s.cfg.workspace_path.clone(),
                    s.cfg.recent_workspaces.clone(),
                )
            });
            let mut opened = super::workspace_windows::opened();
            opened.sort_unstable();
            opened.dedup();
            opened.retain(|path| *path != current);
            if !current.is_empty() {
                opened.insert(0, current.clone());
            }
            let recent: Vec<_> = recent
                .into_iter()
                .filter(|path| !opened.contains(path))
                .collect();
            let mut items = Vec::new();
            if !opened.is_empty() {
                items.push(divider(p).into_element());
                items.push(section("已打开项目", p).into_element());
                items.extend(opened.into_iter().map(|path| {
                    let active = path == current;
                    project_row(path, active, p).into_element()
                }));
            }
            if !recent.is_empty() {
                items.push(divider(p).into_element());
                items.push(section("最近项目", p).into_element());
                items.extend(
                    recent
                        .into_iter()
                        .map(|path| project_row(path, false, p).into_element()),
                );
            }
            items
        }))
}
