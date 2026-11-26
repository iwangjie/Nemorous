use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, ensure};
use nsh_base::editor;
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::{RepoInfo, run_git_string};
use crate::git_history_actions::{self as history, ResetMode};
use crate::git_repo_admin::{
    self as admin, AdminAction, LfsAction, RemoteAction, SparseAction, SubmoduleAction, TagAction,
};
use crate::state::{self, app_signal, current_palette};
use crate::{git_remotes, git_worktrees};

#[derive(Default, Clone)]
struct Report(Option<(String, String)>);

pub(crate) fn close_active() -> bool {
    let report = global::<Report>();
    let open = report.with_untracked(|r| r.0.is_some());
    if open {
        report.set(Report::default());
    }
    open
}

pub(super) fn report(title: String, text: String) {
    global::<Report>().set(Report(Some((title, text))));
    app_signal().update(state::enter_modal_focus);
}

pub(super) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some((title, body)) = global::<Report>().with(|r| r.0.clone()) else {
            return view().into_element();
        };
        let p = current_palette();
        view()
            .absolute()
            .inset_0()
            .p(px(10.0))
            .bg(rgba(0x00000070))
            .items_center()
            .justify_center()
            .child(
                view()
                    .test_id("git-advanced-report")
                    .w_full()
                    .max_w(px(890.0))
                    .h_full()
                    .max_h(px(580.0))
                    .flex_col()
                    .bg(p.surface)
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(p.border_strong)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .h(px(36.0))
                            .px(px(10.0))
                            .bg(p.rail_bg)
                            .child(text(title).flex_1().text_color(p.text_primary))
                            .child(
                                super::controls::button()
                                    .test_id("git-advanced-report-close")
                                    .label("关闭")
                                    .cursor(Cursor::Pointer)
                                    .on_press(state::close_active_modal)
                                    .child(text("关闭").text_color(p.accent)),
                            ),
                    )
                    .child(
                        editor()
                            .test_id("git-advanced-report-editor")
                            .flex_1()
                            .read_only(true)
                            .default_value(Some(body.into()))
                            .soft_wrap(false),
                    ),
            )
            .into_element()
    })
}

fn execute(
    repo: PathBuf,
    title: &'static str,
    opens_conflict: bool,
    work: impl FnOnce(&Path) -> Result<String> + Send + 'static,
) -> bool {
    let path = repo.clone();
    state::run_git_action(
        move || work(&path),
        move |result| match result {
            Ok(_) => state::show_toast("ok", format!("{title}已完成")),
            Err(err) => {
                state::show_toast("warn", format!("{title}：{err}"));
                if opens_conflict
                    && (history::active_operation(&repo).ok().flatten().is_some()
                        || crate::merge_tasks::unresolved(&repo)
                            .is_ok_and(|files| !files.is_empty()))
                {
                    super::conflict_workbench::open(super::conflict_workbench::Source::Repo(repo));
                }
            }
        },
    )
}

fn job(
    repo: PathBuf,
    title: &'static str,
    work: impl FnOnce(&Path) -> Result<String> + Send + 'static,
) {
    execute(repo, title, false, work);
}

fn fields(input: &str) -> Result<(&str, &str)> {
    let (first, second) = input
        .trim()
        .split_once(char::is_whitespace)
        .context("请输入两个字段")?;
    let second = second.trim();
    ensure!(!second.is_empty(), "第二个字段不能为空");
    Ok((first, second))
}

fn admin_prompt(label: &'static str, repo: PathBuf, parse: fn(&str) -> Result<AdminAction>) {
    super::git_prompt::open(label, String::new(), move |value| {
        let action = parse(value)?;
        ensure!(
            execute(repo.clone(), label, false, move |path| admin::run(
                path, &action
            )),
            "另一个 Git 操作正在运行"
        );
        Ok(())
    });
}

fn admin_confirm(repo: PathBuf, title: String, action: AdminAction) {
    super::git_prompt::confirm(title, move || {
        let action = action.clone();
        job(repo.clone(), "仓库配置变更", move |path| {
            admin::run(path, &action)
        });
    });
}

#[derive(Default)]
struct RepositoryMenuGeneration(u64);

struct RepositoryMenuRequest {
    generation: u64,
    workspace: String,
}

impl RepositoryMenuRequest {
    fn start() -> Self {
        let current = global::<RepositoryMenuGeneration>();
        let generation = current.with_untracked(|value| value.0.wrapping_add(1));
        current.set(RepositoryMenuGeneration(generation));
        Self {
            generation,
            workspace: app_signal().with_untracked(|s| s.cfg.workspace_path.clone()),
        }
    }

    fn is_current(&self, repo: &Path) -> bool {
        global::<RepositoryMenuGeneration>().with_untracked(|value| value.0 == self.generation)
            && app_signal().with_untracked(|s| {
                s.cfg.workspace_path == self.workspace
                    && s.repos.iter().any(|candidate| Path::new(&candidate.path) == repo)
            })
    }
}

fn inspect_admin(repo: PathBuf) {
    let request = RepositoryMenuRequest::start();
    state::spawn_from_root(async move {
        let path = repo.clone();
        let result = blocking(move || admin::inspect(&path)).await;
        if !request.is_current(&repo) {
            return;
        }
        match result {
            Ok(Ok(summary)) => {
                let mut items = Vec::new();
                let mut tags = summary
                    .tags
                    .into_iter()
                    .map(|name| {
                        let (delete_path, push_path, push_name) =
                            (repo.clone(), repo.clone(), name.clone());
                        let menu_label = name.clone();
                        MenuItem::submenu(
                            menu_label,
                            [
                                MenuItem::new("推送到 origin").on_select(move || {
                                    let action = AdminAction::Tag(TagAction::Push {
                                        name: push_name.clone(),
                                        remote: "origin".to_owned(),
                                    });
                                    job(push_path.clone(), "推送标签", move |path| {
                                        admin::run(path, &action)
                                    });
                                }),
                                MenuItem::new("删除标签…").on_select(move || {
                                    admin_confirm(
                                        delete_path.clone(),
                                        format!("删除本地标签 {name}？"),
                                        AdminAction::Tag(TagAction::Delete { name: name.clone() }),
                                    )
                                }),
                            ],
                        )
                    })
                    .collect::<Vec<_>>();
                let new = repo.clone();
                tags.insert(
                    0,
                    MenuItem::new("创建标签…").on_select(move || {
                        admin_prompt("标签名和可选说明", new.clone(), |text| {
                            let (name, message) = text
                                .split_once(char::is_whitespace)
                                .map_or((text, None), |(name, msg)| {
                                    (name, Some(msg.trim().to_owned()))
                                });
                            ensure!(!name.is_empty(), "请输入标签名");
                            Ok(AdminAction::Tag(TagAction::Create {
                                name: name.to_owned(),
                                rev: None,
                                message,
                            }))
                        });
                    }),
                );
                items.push(MenuItem::submenu("标签", tags));

                let mut remotes = summary
                    .remotes
                    .into_iter()
                    .map(|item| {
                        let (drop, update, name) = (repo.clone(), repo.clone(), item.name.clone());
                        MenuItem::submenu(
                            format!("{} · {}", item.name, item.url),
                            [
                                MenuItem::new("修改地址…").on_select(move || {
                                    let repo = update.clone();
                                    let name = name.clone();
                                    super::git_prompt::open(
                                        "远端地址",
                                        String::new(),
                                        move |url| {
                                            ensure!(!url.is_empty(), "请输入地址");
                                            let action =
                                                AdminAction::Remote(RemoteAction::ChangeUrl {
                                                    name: name.clone(),
                                                    url: url.to_owned(),
                                                });
                                            ensure!(
                                                execute(
                                                    repo.clone(),
                                                    "修改远端",
                                                    false,
                                                    move |path| admin::run(path, &action)
                                                ),
                                                "另一个 Git 操作正在运行"
                                            );
                                            Ok(())
                                        },
                                    );
                                }),
                                MenuItem::new("删除远端…").on_select(move || {
                                    admin_confirm(
                                        drop.clone(),
                                        format!("删除远端 {}？", item.name),
                                        AdminAction::Remote(RemoteAction::Delete {
                                            name: item.name.clone(),
                                        }),
                                    )
                                }),
                            ],
                        )
                    })
                    .collect::<Vec<_>>();
                let new = repo.clone();
                remotes.insert(
                    0,
                    MenuItem::new("添加远端…").on_select(move || {
                        admin_prompt("远端名和 URL，以空格分隔", new.clone(), |text| {
                            let (name, url) = fields(text)?;
                            Ok(AdminAction::Remote(RemoteAction::Add {
                                name: name.to_owned(),
                                url: url.to_owned(),
                            }))
                        });
                    }),
                );
                items.push(MenuItem::submenu("远端", remotes));

                let mut modules = summary
                    .submodules
                    .into_iter()
                    .map(|item| {
                        let repo = repo.clone();
                        MenuItem::submenu(
                            format!("{} · {}", item.path, item.status),
                            [MenuItem::new("移除子模块…").on_select(move || {
                                admin_confirm(
                                    repo.clone(),
                                    format!("移除子模块 {}？", item.path),
                                    AdminAction::Submodule(SubmoduleAction::Remove {
                                        path: item.path.clone(),
                                    }),
                                )
                            })],
                        )
                    })
                    .collect::<Vec<_>>();
                let (add, update, sync) = (repo.clone(), repo.clone(), repo.clone());
                modules.splice(
                    ..0,
                    [
                        MenuItem::new("添加子模块…").on_select(move || {
                            admin_prompt(
                                "子模块 URL 和相对目录，以空格分隔",
                                add.clone(),
                                |text| {
                                    let (url, destination) = fields(text)?;
                                    Ok(AdminAction::Submodule(SubmoduleAction::Add {
                                        destination: destination.to_owned(),
                                        url: url.to_owned(),
                                    }))
                                },
                            )
                        }),
                        MenuItem::new("更新子模块").on_select(move || {
                            job(update.clone(), "更新子模块", |path| {
                                admin::run(path, &AdminAction::Submodule(SubmoduleAction::Update))
                            })
                        }),
                        MenuItem::new("同步子模块").on_select(move || {
                            job(sync.clone(), "同步子模块", |path| {
                                admin::run(path, &AdminAction::Submodule(SubmoduleAction::Sync))
                            })
                        }),
                    ],
                );
                items.push(MenuItem::submenu("子模块", modules));

                let (set, disable, install, pull, track) = (
                    repo.clone(),
                    repo.clone(),
                    repo.clone(),
                    repo.clone(),
                    repo.clone(),
                );
                items.push(MenuItem::submenu(
                    if summary.sparse_enabled {
                        "Sparse Checkout · 已启用"
                    } else {
                        "Sparse Checkout"
                    },
                    [
                        MenuItem::new("设置规则…").on_select(move || {
                            admin_prompt(
                                "Sparse 规则，多个规则以逗号分隔",
                                set.clone(),
                                |text| {
                                    let patterns = text
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|pattern| !pattern.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    Ok(AdminAction::Sparse(SparseAction::Set { patterns }))
                                },
                            )
                        }),
                        MenuItem::new("禁用").on_select(move || {
                            job(disable.clone(), "禁用 Sparse", |path| {
                                admin::run(path, &AdminAction::Sparse(SparseAction::Disable))
                            })
                        }),
                    ],
                ));
                if summary.lfs_available {
                    items.push(MenuItem::submenu(
                        if summary.lfs_configured { "Git LFS · 仓库已初始化" } else { "Git LFS" },
                        [
                            MenuItem::new("在此仓库初始化").on_select(move || {
                                job(install.clone(), "初始化 Git LFS", |path| {
                                    admin::run(path, &AdminAction::Lfs(LfsAction::InstallLocal))
                                })
                            }),
                            MenuItem::new("拉取 LFS 内容").on_select(move || {
                                job(pull.clone(), "拉取 Git LFS", |path| {
                                    admin::run(path, &AdminAction::Lfs(LfsAction::Pull))
                                })
                            }),
                            MenuItem::new("跟踪模式…").on_select(move || {
                                admin_prompt("Git LFS 跟踪模式", track.clone(), |text| {
                                    ensure!(!text.is_empty(), "请输入跟踪模式");
                                    Ok(AdminAction::Lfs(LfsAction::Track {
                                        patterns: vec![text.to_owned()],
                                    }))
                                })
                            }),
                        ],
                    ));
                } else {
                    items.push(MenuItem::new("Git LFS · 未安装").disabled(true));
                }
                let sign = repo.clone();
                let enabled = summary.signing.enabled;
                let signing = summary.signing.format.as_deref().unwrap_or("openpgp");
                let key = if summary.signing.key_set {
                    "已配置密钥"
                } else {
                    "默认密钥"
                };
                items.push(MenuItem::submenu(
                    format!("签名 · {signing} · {key}"),
                    [
                        MenuItem::new("启用签名")
                            .checked(enabled)
                            .on_select(move || {
                                job(sign.clone(), "更新签名", move |path| {
                                    admin::run(
                                        path,
                                        &AdminAction::Signing {
                                            enabled: !enabled,
                                            format: None,
                                            key: None,
                                        },
                                    )
                                })
                            }),
                        MenuItem::new("配置签名格式和密钥…").on_select(move || {
                            admin_prompt(
                                "签名格式及密钥 ID 或路径，以空格分隔",
                                repo.clone(),
                                |input| {
                                    let (format, key) = fields(input)?;
                                    Ok(AdminAction::Signing {
                                        enabled: true,
                                        format: Some(format.to_owned()),
                                        key: Some(key.to_owned()),
                                    })
                                },
                            )
                        }),
                    ],
                ));
                state::show_popup_menu(items);
            }
            Ok(Err(err)) => state::show_toast("warn", err.to_string()),
            Err(err) => state::show_toast("warn", err.to_string()),
        }
    });
}

fn worktrees(repo: PathBuf) {
    let request = RepositoryMenuRequest::start();
    state::spawn_from_root(async move {
        let path = repo.clone();
        let result = blocking(move || git_worktrees::list(&path)).await;
        if !request.is_current(&repo) {
            return;
        }
        let rows = match result {
            Ok(Ok(rows)) => rows,
            Ok(Err(err)) => {
                state::show_toast("warn", format!("无法读取工作树：{err:#}"));
                return;
            }
            Err(err) => {
                state::show_toast("warn", format!("无法读取工作树：{err}"));
                return;
            }
        };
        let (add, prune) = (repo.clone(), repo.clone());
        let mut items = vec![
            MenuItem::new("新建工作树…").on_select(move || {
                let repo = add.clone();
                super::git_prompt::open(
                    "工作树分支和新目录，以空格分隔",
                    String::new(),
                    move |input| {
                        let (branch, path) = fields(input)?;
                        let (branch, path) = (branch.to_owned(), PathBuf::from(path));
                        ensure!(
                            execute(repo.clone(), "创建工作树", false, move |repo| {
                                git_worktrees::create(repo, &path, &branch).map(|_| String::new())
                            }),
                            "另一个 Git 操作正在运行"
                        );
                        Ok(())
                    },
                );
            }),
            MenuItem::new("清理已失效的工作树").on_select(move || {
                job(prune.clone(), "清理工作树", |repo| {
                    git_worktrees::prune(repo).map(|_| String::new())
                })
            }),
        ];
        for (index, row) in rows.into_iter().enumerate() {
            let branch = row
                .branch
                .as_deref()
                .or(row.hash.as_deref())
                .unwrap_or("HEAD");
            let mut label = format!("{} · {}", row.path.display(), branch);
            if row.locked {
                label.push_str(" · 已锁定");
            }
            if row.prunable {
                label.push_str(" · 目录已失效");
            }
            let can_open = !row.prunable && (row.branch.is_some() || row.detached)
                && Path::new(&request.workspace) != row.path;
            let open_path = row.path.clone();
            let mut actions = vec![MenuItem::new("打开工作树")
                .disabled(!can_open)
                .on_select(move || super::workspace_menu::open_in_this_window(&open_path))];
            if index != 0 {
                let repo = repo.clone();
                let forced = repo.clone();
                let force_path = row.path.clone();
                actions.extend([
                        MenuItem::new("移除工作树…")
                            .disabled(row.locked)
                            .on_select(move || {
                                let (repo, path) = (repo.clone(), row.path.clone());
                                super::git_prompt::confirm(
                                    format!("移除工作树 {}？", path.display()),
                                    move || {
                                        let path = path.clone();
                                        job(repo.clone(), "移除工作树", move |repo| {
                                            git_worktrees::remove(repo, &path, false)
                                                .map(|_| String::new())
                                        });
                                    },
                                );
                            }),
                        MenuItem::new("强制移除…")
                            .disabled(row.locked)
                            .on_select(move || {
                                let (repo, path) = (forced.clone(), force_path.clone());
                                super::git_prompt::confirm(
                                    format!("强制移除 {} 将丢弃未提交改动。继续？", path.display()),
                                    move || {
                                        let path = path.clone();
                                        job(repo.clone(), "强制移除工作树", move |repo| {
                                            git_worktrees::remove(repo, &path, true)
                                                .map(|_| String::new())
                                        });
                                    },
                                );
                            }),
                ]);
            }
            items.push(MenuItem::submenu(label, actions));
        }
        state::show_popup_menu(items);
    });
}

fn reflog(repo: PathBuf) {
    let request = RepositoryMenuRequest::start();
    state::spawn_from_root(async move {
        let path = repo.clone();
        let result = blocking(move || -> Result<_> {
            Ok((
                history::resolve_commit(&path, "HEAD")?,
                history::reflog(&path, 30)?,
            ))
        })
        .await;
        if !request.is_current(&repo) {
            return;
        }
        let Ok(Ok((expected, entries))) = result else {
            state::show_toast("warn", "无法读取 reflog");
            return;
        };
        let items = entries
            .into_iter()
            .map(|entry| {
                let label = format!("{} {} · {}", entry.selector, &entry.oid[..8], entry.subject);
                let mut items = [
                    ("Soft", ResetMode::Soft),
                    ("Mixed", ResetMode::Mixed),
                    ("Hard", ResetMode::Hard),
                    ("Keep", ResetMode::Keep),
                ]
                .into_iter()
                .map(|(label, mode)| {
                    let (repo, oid, expected) = (repo.clone(), entry.oid.clone(), expected.clone());
                    MenuItem::new(format!("{label} 重置…")).on_select(move || {
                        let (repo, oid, expected) = (repo.clone(), oid.clone(), expected.clone());
                        let suffix = if mode == ResetMode::Hard {
                            " 将覆盖暂存区与工作区。"
                        } else {
                            ""
                        };
                        super::git_prompt::confirm(
                            format!("将 HEAD 重置到 {}？{suffix}", &oid[..8]),
                            move || {
                                let (oid, expected) = (oid.clone(), expected.clone());
                                job(repo.clone(), "重置", move |path| {
                                    ensure!(
                                        history::resolve_commit(path, "HEAD")? == expected,
                                        "HEAD 已改变，请重新选择 reflog 条目"
                                    );
                                    history::reset(path, &oid, mode).map(|_| String::new())
                                });
                            },
                        );
                    })
                })
                .collect::<Vec<_>>();
                let repo = repo.clone();
                items.push(MenuItem::new("分离 HEAD 检出…").on_select(move || {
                    let (repo, oid) = (repo.clone(), entry.oid.clone());
                    super::git_prompt::confirm(
                        format!("在 {} 分离 HEAD？", &oid[..8]),
                        move || {
                            let oid = oid.clone();
                            job(repo.clone(), "分离 HEAD", move |path| {
                                history::checkout_detached(path, &oid).map(|_| String::new())
                            });
                        },
                    );
                }));
                MenuItem::submenu(label, items)
            })
            .collect::<Vec<_>>();
        state::show_popup_menu(items);
    });
}

fn compare(repo: PathBuf, three_dot: bool) {
    super::git_prompt::open(
        "基线与目标引用，以空格分隔",
        "main HEAD".into(),
        move |input| {
            let (left, right) = fields(input)?;
            let (left, right, repo) = (left.to_owned(), right.to_owned(), repo.clone());
            compare_refs(repo, left, right, three_dot);
            Ok(())
        },
    );
}

pub(super) fn compare_refs(repo: PathBuf, left: String, right: String, three_dot: bool) {
    state::spawn_from_root(async move {
        let label = format!("{left} {} {right}", if three_dot { "..." } else { ".." });
        match blocking(move || history::ref_diff(&repo, &left, &right, three_dot)).await {
            Ok(Ok(patch)) => report(
                label,
                if patch.is_empty() {
                    "无差异".to_owned()
                } else {
                    patch
                },
            ),
            Ok(Err(err)) => state::show_toast("warn", err.to_string()),
            Err(err) => state::show_toast("err", err.to_string()),
        }
    });
}

pub(super) fn commit_items(commit: &crate::git::CommitEntry) -> Vec<MenuItem> {
    let repo = PathBuf::from(&commit.repo_path);
    let (pick, revert, compare) = (repo.clone(), repo.clone(), repo);
    let (first, second, third) = (
        commit.hash.clone(),
        commit.hash.clone(),
        commit.hash.clone(),
    );
    vec![
        MenuItem::new("Cherry-pick 此提交").on_select(move || {
            let oid = first.clone();
            execute(pick.clone(), "Cherry-pick", true, move |dir| {
                history::cherry_pick(dir, &oid).map(|_| String::new())
            });
        }),
        MenuItem::new("Revert 此提交…").on_select(move || {
            let repo = revert.clone();
            let oid = second.clone();
            super::git_prompt::confirm(
                format!("创建反向提交以撤销 {}？", &oid[..8]),
                move || {
                    let oid = oid.clone();
                    execute(repo.clone(), "Revert", true, move |dir| {
                        history::revert(dir, &oid, None).map(|_| String::new())
                    });
                },
            );
        }),
        MenuItem::new("与 HEAD 比较提交树").on_select(move || {
            compare_refs(compare.clone(), third.clone(), "HEAD".to_owned(), false);
        }),
    ]
}

pub(super) fn stash_workspace_items() -> [MenuItem; 2] {
    [
        MenuItem::new("贮藏全部仓库").on_select(|| stash_workspace(false)),
        MenuItem::new("恢复全部仓库的最近贮藏…").on_select(|| {
            super::git_prompt::confirm("逐仓弹出最近的 stash？", || stash_workspace(true));
        }),
    ]
}

fn stash_workspace(pop: bool) {
    let repos = app_signal().with_untracked(|state| state.repos.clone());
    if repos.is_empty() {
        state::show_toast("warn", "当前工作区没有仓库");
        return;
    }
    let title = if pop {
        "恢复全部贮藏"
    } else {
        "贮藏全部改动"
    };
    state::run_git_action(
        move || {
            repos
                .into_iter()
                .map(|repo| {
                    let path = Path::new(&repo.path);
                    let result = (|| -> Result<Option<String>> {
                        let probe = if pop {
                            ["stash", "list", "-1", "--format=%H"].as_slice()
                        } else {
                            ["status", "--porcelain", "-z", "-uall"].as_slice()
                        };
                        if run_git_string(path, probe)?.is_empty() {
                            return Ok(None);
                        }
                        let args = if pop {
                            ["stash", "pop", "--index", "stash@{0}"].as_slice()
                        } else {
                            [
                                "stash",
                                "push",
                                "--include-untracked",
                                "--message",
                                "nemorous: 工作区贮藏",
                            ]
                            .as_slice()
                        };
                        run_git_string(path, args).map(Some)
                    })();
                    match result {
                        Ok(Some(_)) => format!("{} · 已完成", repo.label()),
                        Ok(None) => format!("{} · 跳过", repo.label()),
                        Err(err) => format!("{} · 失败\n{err}", repo.label()),
                    }
                })
                .collect::<Vec<_>>()
        },
        move |rows| report(title.to_owned(), rows.join("\n\n")),
    );
}

pub(super) fn list_stash(repo: PathBuf) {
    let request = RepositoryMenuRequest::start();
    state::spawn_from_root(async move {
        let dir = repo.clone();
        let result =
            blocking(move || run_git_string(&dir, &["stash", "list", "--format=%gd%x00%H%x00%gs"]))
                .await;
        if !request.is_current(&repo) {
            return;
        }
        let Ok(Ok(list)) = result else {
            state::show_toast("warn", "无法读取 stash");
            return;
        };
        let create = repo.clone();
        let mut items = vec![MenuItem::new("贮藏所有改动…").on_select(move || {
            let repo = create.clone();
            super::git_prompt::open("贮藏说明", String::new(), move |message| {
                let message = message.to_owned();
                ensure!(
                    execute(repo.clone(), "创建 stash", false, move |dir| {
                        run_git_string(
                            dir,
                            &[
                                "stash",
                                "push",
                                "--include-untracked",
                                "--message",
                                &message,
                            ],
                        )
                    }),
                    "另一个 Git 操作正在运行"
                );
                Ok(())
            });
        })];
        for line in list.lines() {
            let fields = line.split('\0').collect::<Vec<_>>();
            if fields.len() != 3 || !crate::git_index::oid_ok(fields[1]) {
                continue;
            }
            let oid = fields[1].to_owned();
            let selector = fields[0].to_owned();
            let label = format!("{} · {}", selector, fields[2]);
            let (show, apply, pop, drop) = (repo.clone(), repo.clone(), repo.clone(), repo.clone());
            let show_ref = oid.clone();
            let apply_ref = oid.clone();
            let pop_ref = oid.clone();
            let pop_at = selector.clone();
            items.push(MenuItem::submenu(
                label,
                [
                    MenuItem::new("查看 Diff").on_select(move || {
                        let (path, oid) = (show.clone(), show_ref.clone());
                        state::spawn_from_root(async move {
                            match blocking(move || {
                                run_git_string(
                                    &path,
                                    &["stash", "show", "--patch", "--include-untracked", &oid],
                                )
                            })
                            .await
                            {
                                Ok(Ok(patch)) => report("Stash Diff".to_owned(), patch),
                                Ok(Err(err)) => state::show_toast("warn", err.to_string()),
                                Err(err) => state::show_toast("warn", err.to_string()),
                            }
                        });
                    }),
                    MenuItem::new("应用").on_select(move || {
                        let oid = apply_ref.clone();
                        execute(apply.clone(), "应用 stash", true, move |dir| {
                            run_git_string(dir, &["stash", "apply", "--index", &oid])
                        });
                    }),
                    MenuItem::new("弹出").on_select(move || {
                        let (oid, at) = (pop_ref.clone(), pop_at.clone());
                        execute(pop.clone(), "弹出 stash", true, move |dir| {
                            ensure!(
                                history::resolve_commit(dir, &at)? == oid,
                                "stash 列表已经改变"
                            );
                            run_git_string(dir, &["stash", "pop", "--index", &at])
                        });
                    }),
                    MenuItem::new("删除 stash…").on_select(move || {
                        let (path, oid, at) = (drop.clone(), oid.clone(), selector.clone());
                        super::git_prompt::confirm(format!("删除 {at}？"), move || {
                            let (oid, at) = (oid.clone(), at.clone());
                            job(path.clone(), "删除 stash", move |dir| {
                                ensure!(
                                    history::resolve_commit(dir, &at)? == oid,
                                    "stash 列表已经改变"
                                );
                                run_git_string(dir, &["stash", "drop", &at])
                            });
                        });
                    }),
                ],
            ));
        }
        state::show_popup_menu(items);
    });
}

fn single_commit_action(repo: PathBuf, label: &'static str, action: fn(&Path, &str) -> Result<()>) {
    super::git_prompt::open(label, String::new(), move |spec| {
        ensure!(!spec.is_empty(), "请输入有效的 Git 引用");
        let spec = spec.to_owned();
        ensure!(
            execute(repo.clone(), label, true, move |dir| {
                let oid = history::resolve_commit(dir, &spec)?;
                action(dir, &oid).map(|_| String::new())
            }),
            "另一个 Git 操作正在运行"
        );
        Ok(())
    });
}

pub(super) fn rebase(repo: PathBuf) {
    single_commit_action(repo, "Rebase 到引用", history::rebase);
}

fn force_push(repo: PathBuf) {
    super::git_prompt::confirm(
        "使用当前远端跟踪提交作为租约，强制推送当前分支？",
        move || {
            job(repo.clone(), "租约推送", |dir| {
                let local = run_git_string(dir, &["symbolic-ref", "--quiet", "HEAD"])?
                    .trim()
                    .to_owned();
                let info = run_git_string(
                    dir,
                    &[
                        "for-each-ref",
                        "--format=%(refname)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream)",
                        &local,
                    ],
                )?;
                let row = info
                    .lines()
                    .find(|record| record.split('\0').next() == Some(&local))
                    .context("当前分支没有远端跟踪")?;
                let fields = row.split('\0').collect::<Vec<_>>();
                ensure!(
                    fields.len() == 4
                        && !fields[1].is_empty()
                        && fields[2].starts_with("refs/heads/")
                        && fields[3].starts_with("refs/"),
                    "上游分支配置无效"
                );
                let expected = history::resolve_commit(dir, fields[3])?;
                let lease = format!("--force-with-lease={}:{expected}", fields[2]);
                let spec = format!("{local}:{}", fields[2]);
                git_remotes::network(
                    dir,
                    &["push", &lease, "--", fields[1], &spec],
                    &AtomicBool::new(false),
                )?;
                Ok(String::new())
            });
        },
    );
}

pub(super) fn repo_items(repo: RepoInfo) -> Vec<MenuItem> {
    let path = PathBuf::from(&repo.path);
    let (conflict, old, admin, worktree, stash, two, three) = (
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
    );
    let revert_merge = path.clone();
    let (rebase, plan, pick, revert, lease, credentials) = (
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
        path.clone(),
    );
    vec![
        MenuItem::new("冲突工作台…").on_select(move || {
            super::conflict_workbench::open(super::conflict_workbench::Source::Repo(
                conflict.clone(),
            ))
        }),
        MenuItem::new("Reflog 与重置…").on_select(move || reflog(old.clone())),
        MenuItem::new("交互式 rebase…")
            .on_select(move || super::rebase_plan_view::prompt(plan.clone())),
        MenuItem::new("Rebase…").on_select(move || {
            single_commit_action(rebase.clone(), "Rebase 到引用", history::rebase)
        }),
        MenuItem::new("Cherry-pick…").on_select(move || {
            single_commit_action(pick.clone(), "Cherry-pick 提交", history::cherry_pick)
        }),
        MenuItem::new("Revert…").on_select(move || {
            single_commit_action(revert.clone(), "Revert 提交", |repo, oid| {
                history::revert(repo, oid, None)
            })
        }),
        MenuItem::new("Revert 合并提交…").on_select(move || {
            let repo = revert_merge.clone();
            super::git_prompt::open(
                "Revert 合并提交 · 引用与主父提交序号",
                String::new(),
                move |value| {
                    let (reference, parent) = fields(value)?;
                    let parent: u32 = parent.parse().context("请输入主父提交序号")?;
                    ensure!(parent > 0, "主父提交序号从 1 开始");
                    let reference = reference.to_owned();
                    ensure!(
                        execute(repo.clone(), "Revert", true, move |dir| {
                            let oid = history::resolve_commit(dir, &reference)?;
                            history::revert(dir, &oid, Some(parent)).map(|_| String::new())
                        }),
                        "另一个 Git 操作正在运行"
                    );
                    Ok(())
                },
            );
        }),
        MenuItem::submenu(
            "比较引用",
            [
                MenuItem::new("双点比较…").on_select(move || compare(two.clone(), false)),
                MenuItem::new("三点比较…").on_select(move || compare(three.clone(), true)),
            ],
        ),
        MenuItem::new("强制推送（带租约）…").on_select(move || force_push(lease.clone())),
        MenuItem::new("Git 凭据与 SSH…")
            .on_select(move || super::credentials_view::open(credentials.clone())),
        MenuItem::new("Stash…").on_select(move || list_stash(stash.clone())),
        MenuItem::new("工作树…").on_select(move || worktrees(worktree.clone())),
        MenuItem::new("标签、远端、子模块与仓库配置…")
            .on_select(move || inspect_admin(admin.clone())),
    ]
}
