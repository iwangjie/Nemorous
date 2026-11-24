use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use nsh_core::element::desktop::MenuItem;
use nsh_core::file_dialog::OpenDialog;
use nsh_core::prelude::*;

use crate::state::{self, AppState, app_signal};
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
    app_signal().update(|s| {
        let mut cfg = s.cfg.clone();
        cfg.workspace_path = path.to_owned();
        cfg.add_recent_workspace(path);
        let refresh_gen = s.refresh_gen.wrapping_add(1);
        *s = AppState {
            cfg,
            refresh_gen,
            ..AppState::default()
        };
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

fn change_workspace_in_new_window(path: &Path) {
    if let Err(err) = workspace_ops::launch_window(path) {
        state::show_toast("warn", err.to_string());
        return;
    }
    app_signal().update(|s| s.cfg.add_recent_workspace(&path.to_string_lossy()));
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
            let name = source
                .trim_end_matches('/')
                .rsplit(['/', ':'])
                .next()
                .unwrap_or_default();
            let name = name.strip_suffix(".git").unwrap_or(name);
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

pub(super) fn open() {
    let (workspace, recent, visible, hidden) = app_signal().with_untracked(|s| {
        let bindings = s.cfg.repository_id_bindings.get(&s.cfg.workspace_path);
        let hidden = s.cfg.hidden_repos.iter().map(|id| {
            let label = s.hidden_repo_labels.get(id).cloned().or_else(|| {
                let (path, _) = bindings?.iter().find(|(_, bound_id)| *bound_id == id)?;
                let path = Path::new(path);
                let label_path = path.strip_prefix(&s.cfg.workspace_path).ok()
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .unwrap_or(path);
                Some(crate::paths::display_path(&label_path.to_string_lossy()))
            });
            (id.clone(), label.unwrap_or_else(|| id.clone()))
        }).collect::<Vec<_>>();
        (
            s.cfg.workspace_path.clone(),
            s.cfg.recent_workspaces.clone(),
            s.repos.clone(),
            hidden,
        )
    });
    let ready = !workspace.is_empty();
    let mut items = vec![
        MenuItem::new("打开工作区…").on_select(|| {
            choose_folder("打开工作区", |path| open_in_this_window(&path));
        }),
        MenuItem::new("在新窗口打开工作区…").on_select(|| {
            choose_folder("在新窗口打开工作区", |path| {
                change_workspace_in_new_window(&path)
            });
        }),
        MenuItem::new("在新窗口打开此工作区")
            .disabled(!ready)
            .on_select(move || {
                change_workspace_in_new_window(Path::new(&workspace));
            }),
    ];
    if !recent.is_empty() {
        items.push(MenuItem::submenu(
            "最近工作区",
            recent.into_iter().map(|path| {
                let other = path.clone();
                MenuItem::submenu(
                    crate::paths::display_path(&path),
                    [
                        MenuItem::new("打开")
                            .on_select(move || open_in_this_window(Path::new(&path))),
                        MenuItem::new("在新窗口打开")
                            .on_select(move || change_workspace_in_new_window(Path::new(&other))),
                    ],
                )
            }),
        ));
    }
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new("克隆仓库…")
            .disabled(!ready)
            .on_select(clone_repository),
    );
    items.push(
        MenuItem::new("初始化仓库…")
            .disabled(!ready)
            .on_select(init_repository),
    );
    items.push(
        MenuItem::new("导入现有仓库…")
            .disabled(!ready)
            .on_select(import_repository),
    );
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
    state::show_popup_menu(items);
}
