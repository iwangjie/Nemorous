use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use nsh_base::input;
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::git::RepoInfo;
use crate::merge_tasks::{self, BeginMerge, MergeStrategy, MergeTask};
use crate::state::{self, app_signal, current_palette};

#[derive(Clone, Default)]
struct MergePlanner {
    open: bool,
    repos: Vec<RepoInfo>,
    selected: HashSet<String>,
    source: String,
    target: String,
    preset: String,
    strategy: Option<MergeStrategy>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Preset {
    name: String,
    repo_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    source_branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    updated_at: String,
    target_branch: String,
}

fn root() -> PathBuf {
    crate::config::config_dir().join("merge-tasks")
}

fn plan() -> Signal<MergePlanner> {
    global::<MergePlanner>()
}

pub(super) fn open() {
    let (repos, branch, target) = app_signal().with_untracked(|s| {
        (
            s.repos.clone(),
            s.statuses
                .iter()
                .find(|st| !st.detached)
                .map(|st| st.branch.clone())
                .unwrap_or_default(),
            s.cfg.release_branch.clone(),
        )
    });
    let selected = repos.iter().map(|repo| repo.id.clone()).collect();
    plan().set(MergePlanner {
        open: true,
        repos,
        selected,
        source: branch,
        target,
        ..MergePlanner::default()
    });
    app_signal().update(state::enter_modal_focus);
}

pub(super) fn open_for(repo: RepoInfo, source: String, target: String) {
    plan().set(MergePlanner {
        open: true,
        selected: HashSet::from([repo.id.clone()]),
        repos: vec![repo],
        source,
        target,
        ..MergePlanner::default()
    });
    app_signal().update(state::enter_modal_focus);
}

pub(crate) fn close_active() -> bool {
    let open = plan().with_untracked(|p| p.open);
    if open {
        plan().set(MergePlanner::default());
    }
    open
}

fn known_tasks(repos: &[RepoInfo]) -> Vec<MergeTask> {
    let paths = repos
        .iter()
        .map(|repo| PathBuf::from(&repo.path))
        .collect::<Vec<_>>();
    merge_tasks::list(&root(), &paths)
}

fn stored() -> Vec<Preset> {
    app_signal().with_untracked(|s| {
        s.cfg
            .extra
            .get("merge_presets")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    })
}

fn save_preset() {
    let entry = plan().with_untracked(|p| Preset {
        name: p.preset.trim().to_owned(),
        repo_ids: p
            .repos
            .iter()
            .filter(|repo| p.selected.contains(&repo.id))
            .map(|repo| repo.id.clone())
            .collect(),
        source_branch: p.source.trim().to_owned(),
        target_branch: p.target.trim().to_owned(),
        updated_at: chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
            .to_rfc3339(),
    });
    if entry.name.is_empty()
        || entry.source_branch.is_empty()
        || entry.target_branch.is_empty()
        || entry.repo_ids.is_empty()
    {
        state::show_toast("warn", "请填写组合名、来源分支、目标分支并选择仓库");
        return;
    }
    let mut presets = stored();
    presets.retain(|preset| preset.name != entry.name);
    presets.insert(0, entry);
    if let Ok(value) = serde_json::to_value(presets) {
        app_signal().update(|s| {
            s.cfg.extra.insert("merge_presets".to_owned(), value);
        });
        state::save_config_quiet();
        state::show_toast("ok", "已保存合并组合");
    }
}

fn presets_menu() {
    let items = stored()
        .into_iter()
        .map(|preset| {
            MenuItem::new(preset.name.clone()).on_select(move || {
                plan().update(|p| {
                    if !preset.source_branch.is_empty() {
                        p.source.clone_from(&preset.source_branch);
                    }
                    p.target.clone_from(&preset.target_branch);
                    p.selected = preset.repo_ids.iter().cloned().collect();
                });
            })
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        state::show_toast("info", "没有已保存的合并组合");
    } else {
        state::show_popup_menu(items);
    }
}

fn run() {
    let (repos, source, target, strategy) = plan().with_untracked(|p| {
        (
            p.repos
                .iter()
                .filter(|repo| p.selected.contains(&repo.id))
                .cloned()
                .collect::<Vec<_>>(),
            p.source.trim().to_owned(),
            p.target.trim().to_owned(),
            p.strategy.unwrap_or(MergeStrategy::Default),
        )
    });
    if repos.is_empty() || source.is_empty() || target.is_empty() || source == target {
        state::show_toast("warn", "请选择仓库和两个不同的分支");
        return;
    }
    let task_root = root();
    let ui_root = task_root.clone();
    let started = state::run_git_action(
        move || {
            let mut results = BTreeMap::new();
            for repo in repos {
                let result = merge_tasks::start(
                    &task_root,
                    &PathBuf::from(&repo.path),
                    &source,
                    &target,
                    strategy,
                );
                results.insert(repo.name, result);
            }
            results
        },
        move |results: BTreeMap<String, Result<BeginMerge>>| {
            let mut details = Vec::new();
            let mut pending = Vec::new();
            for (repo, result) in results {
                match result {
                    Ok(BeginMerge::Complete(hash)) => {
                        details.push(format!("{repo} 已合并 {}", &hash[..8]))
                    }
                    Ok(BeginMerge::Waiting(task)) => {
                        details.push(format!(
                            "{repo} 待处理 {} 个冲突",
                            task.original_conflicts.len()
                        ));
                        pending.push(task);
                    }
                    Err(err) => details.push(format!("{repo} 合并失败：{err}")),
                }
            }
            state::show_toast(
                if pending.is_empty() { "ok" } else { "warn" },
                details.join("\n"),
            );
            if let [task] = pending.as_slice() {
                super::conflict_workbench::open(super::conflict_workbench::Source::Isolated {
                    root: ui_root,
                    task: task.clone(),
                });
            } else if !pending.is_empty() {
                open();
            }
        },
    );
    if started {
        state::close_active_modal();
    }
}

fn action(
    label: &'static str,
    id: &'static str,
    callback: impl Fn() + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .label(label)
        .px(px(8.0))
        .h(px(26.0))
        .rounded(px(4.0))
        .cursor(Cursor::Pointer)
        .items_center()
        .justify_center()
        .bg(p.surface2)
        .hover(move |s| s.bg(p.surface_hover))
        .on_press(callback)
        .child(text(label).text_size(px(11.5)).text_color(p.text_primary))
}

fn strategy_menu() {
    let items = [
        ("默认合并", MergeStrategy::Default),
        ("强制合并提交", MergeStrategy::NoFf),
        ("仅快进", MergeStrategy::FastForward),
        ("压缩为单个提交", MergeStrategy::Squash),
    ]
    .into_iter()
    .map(|(label, strategy)| {
        MenuItem::new(label).on_select(move || plan().update(|p| p.strategy = Some(strategy)))
    })
    .collect();
    state::show_popup_menu(items);
}

pub(super) fn layer() -> impl IntoElement {
    let visible = computed(|| plan().with(|p| p.open));
    when(
        move || visible.get(),
        || {
            let p = current_palette();
            let repos = plan().with_untracked(|p| p.repos.clone());
            let tasks = known_tasks(&repos);
            view()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000070))
                .p(px(10.0))
                .child(
                    view()
                        .test_id("git-merge-planner")
                        .w_full()
                        .max_w(px(580.0))
                        .h_full()
                        .max_h(px(535.0))
                        .flex_col()
                        .px(px(13.0))
                        .py(px(10.0))
                        .gap(px(6.0))
                        .bg(p.surface)
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(p.border_strong)
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .child(
                                    text("多仓合并")
                                        .flex_1()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_primary),
                                )
                                .child(action("关闭", "merge-close", state::close_active_modal)),
                        )
                        .child(
                            view()
                                .flex_row()
                                .gap(px(6.0))
                                .child(
                                    input()
                                        .test_id("merge-source")
                                        .flex_1()
                                        .h(px(27.0))
                                        .px(px(6.0))
                                        .bg(p.surface2)
                                        .placeholder("来源分支")
                                        .text_color(p.text_primary)
                                        .value(|| plan().with(|p| p.source.clone()))
                                        .on_change(|text: &str| {
                                            plan().update(|p| p.source = text.to_owned())
                                        }),
                                )
                                .child(
                                    input()
                                        .test_id("merge-target")
                                        .flex_1()
                                        .h(px(27.0))
                                        .px(px(6.0))
                                        .bg(p.surface2)
                                        .placeholder("目标分支")
                                        .text_color(p.text_primary)
                                        .value(|| plan().with(|p| p.target.clone()))
                                        .on_change(|text: &str| {
                                            plan().update(|p| p.target = text.to_owned())
                                        }),
                                )
                                .child(action("合并方式…", "merge-strategy", strategy_menu)),
                        )
                        .child(dynamic(move || {
                            let label = plan().with(|p| {
                                match p.strategy.unwrap_or(MergeStrategy::Default) {
                                    MergeStrategy::Default => "默认合并",
                                    MergeStrategy::NoFf => "强制合并提交",
                                    MergeStrategy::FastForward => "仅快进",
                                    MergeStrategy::Squash => "压缩为单个提交",
                                }
                            });
                            text(label).text_size(px(11.0)).text_color(p.text_muted)
                        }))
                        .child(
                            scroll().flex_1().child(
                                view()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .children(repos.into_iter().map(|repo| {
                                        let id = repo.id.clone();
                                        let selected_id = id.clone();
                                        view()
                                            .test_id(&format!("merge-repo-{id}"))
                                            .h(px(27.0))
                                            .items_center()
                                            .px(px(7.0))
                                            .cursor(Cursor::Pointer)
                                            .on_press(move || {
                                                plan().update(|p| {
                                                    if !p.selected.remove(&id) {
                                                        p.selected.insert(id.clone());
                                                    }
                                                })
                                            })
                                            .child(
                                                text(move || {
                                                    let checked = plan().with(|p| {
                                                        p.selected.contains(&selected_id)
                                                    });
                                                    format!(
                                                        "{} {}",
                                                        if checked { "✓" } else { "□" },
                                                        repo.name
                                                    )
                                                })
                                                .text_color(p.text_primary),
                                            )
                                    }))
                                    .children(tasks.into_iter().map(|task| {
                                        let label = format!(
                                            "继续 {} → {} · {} 个冲突",
                                            task.source,
                                            task.target,
                                            task.original_conflicts.len()
                                        );
                                        view()
                                            .test_id(&format!("merge-recover-{}", task.id))
                                            .h(px(29.0))
                                            .px(px(6.0))
                                            .items_center()
                                            .cursor(Cursor::Pointer)
                                            .bg(p.accent_soft)
                                            .rounded(px(4.0))
                                            .on_press(move || {
                                                state::close_active_modal();
                                                super::conflict_workbench::open(
                                                    super::conflict_workbench::Source::Isolated {
                                                        root: root(),
                                                        task: task.clone(),
                                                    },
                                                );
                                            })
                                            .child(text(label).text_color(p.accent_soft_text))
                                    })),
                            ),
                        )
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    input()
                                        .test_id("merge-preset-name")
                                        .flex_1()
                                        .h(px(27.0))
                                        .px(px(6.0))
                                        .bg(p.surface2)
                                        .placeholder("组合名称")
                                        .text_color(p.text_primary)
                                        .value(|| plan().with(|p| p.preset.clone()))
                                        .on_change(|text: &str| {
                                            plan().update(|p| p.preset = text.to_owned())
                                        }),
                                )
                                .child(action("保存组合", "merge-preset-save", save_preset))
                                .child(action("合并组合", "merge-preset-menu", presets_menu))
                                .child(action("开始合并", "merge-run", run)),
                        ),
                )
        },
    )
}
