use std::path::{Path, PathBuf};

use nsh_core::element::desktop::MenuItem;

use crate::git::RepoInfo;
use crate::git_remotes::RemoteAction;
use crate::keymap::Keymap;
use crate::state::{self, AppState, EditorTabKind, LeftTab, app_signal};

use super::{
    branch_matrix, content_search, git_advanced, git_prompt, guard_app_command, menu_binding,
    merge_menu, revision_picker, sync_dialog, workbench, workspace_menu,
};

#[derive(Clone, PartialEq)]
pub(super) struct Context {
    workspace: bool,
    recent: Vec<String>,
    repos: Vec<RepoInfo>,
    local_history: bool,
    can_save: bool,
    busy: bool,
}

impl Context {
    pub(super) fn from_state(s: &AppState) -> Self {
        Self {
            workspace: !s.cfg.workspace_path.is_empty(),
            recent: s
                .cfg
                .recent_workspaces
                .iter()
                .filter(|path| *path != &s.cfg.workspace_path)
                .cloned()
                .collect(),
            repos: s.repos.clone(),
            local_history: matches!(
                s.editor_tab,
                EditorTabKind::File | EditorTabKind::WorkingDiff
            ),
            can_save: s.code_tabs.iter().any(|tab| tab.is_dirty())
                || s.working_diff.as_ref().is_some_and(|diff| diff.is_dirty()),
            busy: s.command_busy,
        }
    }
}

fn item(label: &'static str, disabled: bool, action: impl Fn() + 'static) -> MenuItem {
    MenuItem::new(label)
        .on_select(guard_app_command(action))
        .disabled(disabled)
}

pub(super) fn file(context: &Context, modal: bool, keys: &Keymap) -> MenuItem {
    let closed = modal || !context.workspace;
    let writing = modal || context.busy;
    let recent = context.recent.iter().map(|path| {
        let path = path.clone();
        let name = crate::paths::basename(&path);
        let label = if context
            .recent
            .iter()
            .filter(|candidate| crate::paths::basename(candidate) == name)
            .count()
            > 1
        {
            crate::paths::display_path(&path)
        } else {
            name
        };
        MenuItem::new(label).on_select(guard_app_command(move || {
            workspace_menu::open_project(Path::new(&path))
        }))
    });
    MenuItem::submenu(
        "文件",
        [
            MenuItem::submenu(
                "新建",
                [
                    item("项目…", writing, workspace_menu::new_project),
                    item(
                        "从版本控制创建项目…",
                        writing,
                        workspace_menu::clone_project,
                    ),
                    MenuItem::separator(),
                    item("文件…", closed || writing, || {
                        workspace_menu::new_project_item(false)
                    }),
                    item("目录…", closed || writing, || {
                        workspace_menu::new_project_item(true)
                    }),
                ],
            )
            .disabled(modal),
            item("打开…", modal, workspace_menu::choose_project),
            if context.recent.is_empty() {
                MenuItem::new("最近项目").disabled(true)
            } else {
                MenuItem::submenu("最近项目", recent).disabled(modal)
            },
            item("关闭项目", closed || writing, workspace_menu::close_project),
            MenuItem::separator(),
            MenuItem::submenu(
                "本地历史",
                [item(
                    "显示历史…",
                    closed || !context.local_history,
                    state::open_local_history_for_active,
                )],
            )
            .disabled(closed || !context.local_history),
            menu_binding(
                item(
                    "保存全部",
                    closed || !context.can_save,
                    crate::format_runtime::save_all,
                ),
                keys,
                "save",
            ),
            menu_binding(
                item("从磁盘重新加载全部", closed || writing, || {
                    state::refresh_workspace_async(None)
                }),
                keys,
                "refresh",
            ),
            MenuItem::submenu(
                "文件属性",
                [item(
                    "在文件管理器中显示",
                    closed,
                    workspace_menu::reveal_project_item,
                )],
            )
            .disabled(closed),
        ],
    )
}

fn select_repository_action(
    label: &'static str,
    repos: &[RepoInfo],
    disabled: bool,
    act: impl Fn(RepoInfo) + Copy + 'static,
) -> MenuItem {
    if let [repo] = repos {
        let repo = repo.clone();
        return item(label, disabled, move || act(repo.clone()));
    }
    MenuItem::submenu(
        label,
        repos.iter().map(|repo| {
            let repo = repo.clone();
            MenuItem::new(repo.label().to_owned())
                .on_select(guard_app_command(move || act(repo.clone())))
        }),
    )
    .disabled(disabled || repos.is_empty())
}

fn sync_item(repos: &[RepoInfo], action: RemoteAction, disabled: bool, keys: &Keymap) -> MenuItem {
    let label = match action {
        RemoteAction::Fetch => "获取",
        RemoteAction::Pull => "拉取…",
        RemoteAction::Push => "推送…",
    };
    let perform = move |repos: Vec<RepoInfo>| {
        if action == RemoteAction::Push {
            confirm_push(repos);
        } else {
            sync_dialog::start(repos, action);
        }
    };
    let shortcut = match action {
        RemoteAction::Fetch => "",
        RemoteAction::Pull => "pull",
        RemoteAction::Push => "push",
    };
    if repos.len() < 2 {
        let repos = repos.to_vec();
        let disabled = disabled || repos.is_empty();
        return menu_binding(
            item(label, disabled, move || perform(repos.clone())),
            keys,
            shortcut,
        );
    }
    let all = repos.to_vec();
    let mut items = vec![
        item("全部仓库", disabled, move || perform(all.clone())),
        MenuItem::separator(),
    ];
    items.extend(repos.iter().map(|repo| {
        let repo = repo.clone();
        MenuItem::new(repo.label().to_owned())
            .on_select(guard_app_command(move || perform(vec![repo.clone()])))
    }));
    MenuItem::submenu(label, items).disabled(disabled)
}

pub(super) fn push_all() {
    let repos = app_signal().with_untracked(|s| s.repos.clone());
    confirm_push(repos);
}

fn confirm_push(repos: Vec<RepoInfo>) {
    if repos.is_empty() {
        state::show_toast("warn", "工作区没有可用仓库");
        return;
    }
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let destination = repos
        .iter()
        .map(RepoInfo::label)
        .collect::<Vec<_>>()
        .join("、");
    git_prompt::confirm_after_close(format!("推送改动 · {destination}"), move || {
        let current = app_signal().with_untracked(|s| {
            s.cfg.workspace_path == workspace
                && repos.iter().all(|repo| {
                    s.repos
                        .iter()
                        .any(|candidate| candidate.id == repo.id && candidate.path == repo.path)
                })
        });
        if current {
            sync_dialog::start(repos.clone(), RemoteAction::Push);
        } else {
            state::show_toast("warn", "仓库列表已变化，请重新选择推送目标");
        }
    });
}

fn show_changes() {
    super::focus_tool_window(LeftTab::Commit, false);
    if let Some(selection) = app_signal().with_untracked(|s| s.change_selection.clone()) {
        state::open_working_diff(selection.file, selection.staged, true);
    }
}

fn show_log() {
    workbench::hide();
    content_search::hide_find_panel();
    app_signal().update(|s| s.show_git_panel = true);
}

pub(super) fn git(context: &Context, modal: bool, keys: &Keymap) -> MenuItem {
    let repos = &context.repos;
    let unavailable = modal || repos.is_empty();
    let writing = unavailable || context.busy;
    MenuItem::submenu(
        "Git",
        [
            menu_binding(
                item("提交…", unavailable, super::commit_form::focus),
                keys,
                "gitCommit",
            ),
            item("显示变更", unavailable, show_changes),
            menu_binding(
                item("显示 Git 日志", unavailable, show_log),
                keys,
                "gitWindow",
            ),
            MenuItem::separator(),
            menu_binding(
                item("分支…", unavailable, branch_matrix::open),
                keys,
                "branchMatrix",
            ),
            sync_item(repos, RemoteAction::Fetch, writing, keys),
            sync_item(repos, RemoteAction::Pull, writing, keys),
            sync_item(repos, RemoteAction::Push, writing, keys),
            MenuItem::separator(),
            item("合并…", writing, merge_menu::open),
            select_repository_action("Rebase…", repos, writing, |repo| {
                git_advanced::rebase(PathBuf::from(repo.path))
            }),
            select_repository_action("Stash…", repos, writing, |repo| {
                git_advanced::list_stash(PathBuf::from(repo.path))
            }),
            MenuItem::submenu(
                "选中的文件",
                [
                    item(
                        "显示历史",
                        unavailable,
                        revision_picker::show_active_history,
                    ),
                    item("与修订版本比较…", unavailable, revision_picker::open_active),
                ],
            )
            .disabled(unavailable || !context.local_history),
            MenuItem::separator(),
            item(
                "克隆仓库…",
                modal || context.busy,
                workspace_menu::clone_project,
            ),
        ],
    )
}
