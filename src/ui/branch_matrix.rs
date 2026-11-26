use std::collections::BTreeMap;
use std::path::Path;

use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use super::icons::{self, ICO_BRANCH, ICO_CHECK, ICO_PLUS};
use crate::config::BranchPreset;
use crate::git::RepoInfo;
use crate::git_branches::{self, BranchCandidate, BranchTarget};
use crate::state::{self, app_signal, current_palette};
use crate::theme::repo_badge_colors;

#[derive(Clone)]
struct BranchRow {
    repo: RepoInfo,
    current: String,
    branches: Result<Vec<BranchCandidate>, String>,
}

#[derive(Clone, Default)]
struct Matrix {
    open: bool,
    request: u64,
    rows: Option<Vec<BranchRow>>,
}

#[derive(Default)]
struct BranchFilter(String);

pub(super) fn open() {
    let matrix = global::<Matrix>();
    let request = matrix.with_untracked(|m| m.request.wrapping_add(1));
    let statuses = app_signal().with_untracked(|s| s.statuses.clone());
    global::<BranchFilter>().set(BranchFilter::default());
    matrix.set(Matrix {
        open: true,
        request,
        rows: None,
    });
    app_signal().update(state::enter_modal_focus);
    state::spawn_from_root(async move {
        let data = blocking(move || {
            statuses
                .into_iter()
                .map(|status| {
                    let branches = git_branches::list_branches(Path::new(&status.repo.path))
                        .map_err(|err| err.to_string());
                    BranchRow {
                        repo: status.repo,
                        current: status.branch,
                        branches,
                    }
                })
                .collect::<Vec<_>>()
        })
        .await;
        matrix.update(|m| {
            if m.open && m.request == request {
                m.rows = Some(data.unwrap_or_default());
            }
        });
    });
}

pub(crate) fn close_active() -> bool {
    let matrix = global::<Matrix>();
    let open = matrix.with_untracked(|m| m.open);
    if open {
        matrix.update(|m| {
            m.open = false;
            m.rows = None;
        });
    }
    open
}

fn run(operations: Vec<(RepoInfo, Option<BranchTarget>)>) {
    if operations.is_empty() {
        return;
    }
    let preferred = app_signal().with_untracked(|s| s.cfg.default_base_branch.clone());
    state::run_git_action(
        move || {
            let mut results = Vec::new();
            for (repo, target) in operations {
                let dir = Path::new(&repo.path);
                let result = match target {
                    Some(target) => git_branches::switch_branch(dir, &target),
                    None => git_branches::ensure_clean(dir, true)
                        .and_then(|()| git_branches::default_branch(dir, &preferred))
                        .and_then(|name| {
                            git_branches::switch_branch(dir, &BranchTarget::Existing(name))
                        })
                        .and_then(|()| {
                            let reports = crate::git_remotes::run_batch(
                                std::slice::from_ref(&repo),
                                crate::git_remotes::RemoteAction::Pull,
                                &std::sync::atomic::AtomicBool::new(false),
                                1,
                            );
                            match reports.first().filter(|report| {
                                report.status == crate::git_remotes::RemoteStatus::Failed
                            }) {
                                Some(report) => Err(anyhow::anyhow!(
                                    "已切换分支，拉取未完成：{}",
                                    report.detail
                                )),
                                None => Ok(()),
                            }
                        }),
                };
                results.push((repo.label().to_owned(), result));
            }
            results
        },
        |results| {
            let mut failed = Vec::new();
            for (repo, result) in &results {
                if let Err(err) = result {
                    failed.push(format!("{repo}：{err}"));
                }
            }
            if failed.is_empty() {
                state::show_toast("ok", format!("已切换 {} 个仓库", results.len()));
            } else {
                state::show_toast(
                    "warn",
                    format!(
                        "切换成功 {} 个，失败 {} 个\n{}",
                        results.len() - failed.len(),
                        failed.len(),
                        failed.join("\n")
                    ),
                );
            }
        },
    );
}

fn open_presets() {
    let (current, saved, repos) = app_signal().with_untracked(|s| {
        let current = s
            .statuses
            .iter()
            .filter(|status| !status.detached && !status.branch.is_empty())
            .map(|status| (status.repo.id.clone(), status.branch.clone()))
            .collect::<BTreeMap<_, _>>();
        (current, s.cfg.branch_presets.clone(), s.repos.clone())
    });
    let mut menu = vec![
        MenuItem::new("保存当前分支组合…")
            .disabled(current.is_empty())
            .on_select(move || {
                state::close_active_modal();
                let current = current.clone();
                super::git_prompt::open("分支组合名称", String::new(), move |name| {
                    anyhow::ensure!(!name.is_empty(), "请输入分支组合名称");
                    app_signal().update(|s| {
                        s.cfg.branch_presets.retain(|preset| preset.name != name);
                        s.cfg.branch_presets.insert(
                            0,
                            BranchPreset {
                                name: name.to_owned(),
                                repo_branches: current.clone(),
                            },
                        );
                    });
                    state::save_config_quiet();
                    Ok(())
                });
            }),
    ];
    for preset in saved {
        let operations = repos
            .iter()
            .filter_map(|repo| {
                let branch = preset.repo_branches.get(&repo.id)?;
                Some((repo.clone(), Some(BranchTarget::Existing(branch.clone()))))
            })
            .collect::<Vec<_>>();
        let label = preset.name.clone();
        menu.push(MenuItem::submenu(
            preset.name,
            [
                MenuItem::new("应用")
                    .disabled(operations.is_empty())
                    .on_select(move || {
                        state::close_active_modal();
                        run(operations.clone());
                    }),
                MenuItem::new("删除").on_select(move || {
                    app_signal()
                        .update(|s| s.cfg.branch_presets.retain(|preset| preset.name != label));
                    state::save_config_quiet();
                }),
            ],
        ));
    }
    state::show_popup_menu(menu);
}

fn create(repos: Vec<RepoInfo>) {
    state::close_active_modal();
    super::git_prompt::open("新建分支", String::new(), move |name| {
        anyhow::ensure!(!name.is_empty(), "请输入分支名");
        anyhow::ensure!(
            !app_signal().with_untracked(|s| s.command_busy),
            "其他 Git 操作正在运行"
        );
        run(repos
            .iter()
            .cloned()
            .map(|repo| (repo, Some(BranchTarget::New(name.to_owned()))))
            .collect());
        Ok(())
    });
}

fn action(
    label: impl Into<String>,
    test_id: impl AsRef<str>,
    p: crate::theme::Palette,
    handler: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    super::controls::button()
        .test_id(test_id.as_ref())
        .label(label.clone())
        .cursor(Cursor::Pointer)
        .h(px(26.0))
        .px(px(9.0))
        .rounded(px(5.0))
        .bg(p.surface2)
        .border_1()
        .border_color(p.border)
        .hover(move |s| s.bg(p.surface_hover))
        .on_press(handler)
        .child(text(label).text_size(px(11.5)).text_color(p.text_control))
}

fn branch_row_button(
    test_id: String,
    label: String,
    current: bool,
    badge: Option<String>,
    p: crate::theme::Palette,
    handler: impl Fn() + 'static,
) -> impl IntoElement {
    super::controls::button()
        .test_id(&test_id)
        .label(label.clone())
        .w_full()
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(5.0))
        .flex_row()
        .items_center()
        .justify_start()
        .gap(px(8.0))
        .cursor(Cursor::Pointer)
        .bg(if current { p.accent_soft } else { transparent_black() })
        .hover(move |s| s.bg(if current { p.accent_soft } else { p.surface_hover }))
        .on_press(handler)
        .child(icons::icon(
            if current { ICO_CHECK } else { ICO_BRANCH },
            12.0,
            if current { p.accent_soft_text } else { p.text_muted },
        ))
        .child(
            text(label)
                .flex_1()
                .text_left()
                .text_size(px(12.5))
                .font_weight(if current { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                .text_color(if current { p.accent_soft_text } else { p.text_primary })
                .number_of_lines(1u32),
        )
        .when_some(badge, |row, badge| {
            row.child(
                view()
                    .px(px(6.0))
                    .py(px(1.5))
                    .rounded(px(4.0))
                    .bg(if current { p.green_soft } else { p.surface2 })
                    .child(
                        text(badge)
                            .text_size(px(10.5))
                            .text_color(if current { p.green } else { p.text_muted }),
                    ),
            )
        })
}

fn matrix_rows() -> impl IntoElement {
    dynamic(|| {
        let p = current_palette();
        let query = global::<BranchFilter>().with(|f| f.0.trim().to_lowercase());
        global::<Matrix>().with(|m| {
            let Some(rows) = &m.rows else {
                return text("正在读取分支…")
                    .text_size(px(12.5))
                    .text_color(p.text_muted)
                    .into_element();
            };
            if rows.is_empty() {
                return text("工作区没有可用仓库")
                    .text_size(px(12.5))
                    .text_color(p.text_muted)
                    .into_element();
            }
            let mut common = BTreeMap::<String, Vec<RepoInfo>>::new();
            let mut result = Vec::new();
            for row in rows {
                let repo = row.repo.clone();
                let badge = repo_badge_colors(row.repo.color_idx, p.is_dark);
                let mut branch_list = view().flex_col().gap(px(2.0)).p(px(4.0));
                match &row.branches {
                    Ok(branches) => {
                        for branch in branches {
                            if !branch.name.to_lowercase().contains(&query)
                                && !row.repo.label().to_lowercase().contains(&query)
                            {
                                continue;
                            }
                            common
                                .entry(branch.name.clone())
                                .or_default()
                                .push(row.repo.clone());
                            let target = BranchTarget::Existing(branch.name.clone());
                            let repo = row.repo.clone();
                            let subject = branch.name.clone();
                            let current = row.current.clone();
                            let owner = repo.clone();
                            branch_list = branch_list.child(
                                view()
                                    .on_context_menu(move |event: &ContextMenuEvent| {
                                        if current == subject || current.is_empty() {
                                            return;
                                        }
                                        let (repo_a, repo_b, compare) =
                                            (owner.clone(), owner.clone(), owner.clone());
                                        let (from_a, to_a) = (subject.clone(), current.clone());
                                        let (from_b, to_b) = (current.clone(), subject.clone());
                                        let (base, tip) = (current.clone(), subject.clone());
                                        state::show_popup_menu_at(
                                            vec![
                                                MenuItem::new(format!("合并 {from_a} 到 {to_a}…"))
                                                    .on_select(move || {
                                                        state::close_active_modal();
                                                        super::merge_menu::open_for(
                                                            repo_a.clone(),
                                                            from_a.clone(),
                                                            to_a.clone(),
                                                        );
                                                    }),
                                                MenuItem::new(format!("合并 {from_b} 到 {to_b}…"))
                                                    .on_select(move || {
                                                        state::close_active_modal();
                                                        super::merge_menu::open_for(
                                                            repo_b.clone(),
                                                            from_b.clone(),
                                                            to_b.clone(),
                                                        );
                                                    }),
                                                MenuItem::new("与当前分支比较").on_select(
                                                    move || {
                                                        state::close_active_modal();
                                                        super::git_advanced::compare_refs(
                                                            compare.path.clone().into(),
                                                            base.clone(),
                                                            tip.clone(),
                                                            true,
                                                        );
                                                    },
                                                ),
                                            ],
                                            event.page_position,
                                        );
                                    })
                                    .child(branch_row_button(
                                        format!("branch-switch-{}-{}", row.repo.id, branch.name),
                                        branch.name.clone(),
                                        branch.current,
                                        branch.current.then(|| "当前".to_owned()),
                                        p,
                                        move || {
                                            state::close_active_modal();
                                            run(vec![(repo.clone(), Some(target.clone()))]);
                                        },
                                    )),
                            );
                        }
                    }
                    Err(err) => {
                        branch_list = branch_list.child(
                            text(err.to_string())
                                .px(px(10.0))
                                .py(px(6.0))
                                .text_color(p.danger)
                                .text_size(px(11.5)),
                        );
                    }
                }
                let entry = view()
                    .flex_col()
                    .mb(px(10.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(p.border)
                    .overflow_hidden()
                    .child(
                        view()
                            .h(px(34.0))
                            .px(px(10.0))
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .bg(p.surface2)
                            .border_b_1()
                            .border_color(p.border)
                            .child(
                                view()
                                    .w(px(18.0))
                                    .h(px(18.0))
                                    .rounded(px(4.0))
                                    .bg(badge.bg)
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        text(row.repo.monogram.clone())
                                            .text_size(px(9.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(badge.text),
                                    ),
                            )
                            .child(
                                text(row.repo.label().to_owned())
                                    .text_left()
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary),
                            )
                            .child(
                                view()
                                    .px(px(6.0))
                                    .py(px(1.5))
                                    .rounded(px(4.0))
                                    .bg(p.accent_soft)
                                    .child(
                                        text(row.current.clone())
                                            .text_size(px(11.0))
                                            .text_color(p.accent_soft_text),
                                    ),
                            )
                            .child(view().flex_1())
                            .child(
                                super::controls::button()
                                    .test_id(&format!("branch-new-{}", row.repo.id))
                                    .label("新建…")
                                    .cursor(Cursor::Pointer)
                                    .h(px(22.0))
                                    .px(px(7.0))
                                    .gap(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(p.surface)
                                    .border_1()
                                    .border_color(p.border)
                                    .hover(move |s| s.bg(p.surface_hover))
                                    .on_press(move || create(vec![repo.clone()]))
                                    .child(icons::icon(ICO_PLUS, 11.0, p.text_control))
                                    .child(
                                        text("新建")
                                            .text_size(px(11.0))
                                            .text_color(p.text_control),
                                    ),
                            ),
                    )
                    .child(branch_list);
                result.push(entry.into_element());
            }
            let common_rows: Vec<_> = common
                .into_iter()
                .filter(|(_, repos)| repos.len() > 1)
                .collect();
            let mut combined = view().flex_col();
            if !common_rows.is_empty() {
                let mut list = view().flex_col().gap(px(2.0)).p(px(4.0));
                for (name, repos) in common_rows {
                    let count = repos.len();
                    let operations = repos
                        .into_iter()
                        .map(|repo| (repo, Some(BranchTarget::Existing(name.clone()))))
                        .collect::<Vec<_>>();
                    list = list.child(branch_row_button(
                        format!("branch-multi-{name}"),
                        name.clone(),
                        false,
                        Some(format!("{count} 个仓库")),
                        p,
                        move || {
                            state::close_active_modal();
                            run(operations.clone());
                        },
                    ));
                }
                combined = combined.child(
                    view()
                        .flex_col()
                        .mb(px(12.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(p.border)
                        .overflow_hidden()
                        .child(
                            view()
                                .h(px(30.0))
                                .px(px(10.0))
                                .flex_row()
                                .items_center()
                                .bg(p.surface2)
                                .border_b_1()
                                .border_color(p.border)
                                .child(
                                    text("跨仓库批量切换")
                                        .text_left()
                                        .text_size(px(11.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_secondary),
                                ),
                        )
                        .child(list),
                );
            }
            view()
                .flex_col()
                .child(combined)
                .children(result)
                .into_element()
        })
    })
}

pub(super) fn layer() -> impl IntoElement {
    let open = computed(|| global::<Matrix>().with(|m| m.open));
    dynamic(move || {
        if !open.get() {
            return view().into_element();
        }
        let p = current_palette();
        let repos = app_signal().with_untracked(|s| s.repos.clone());
        let reset = repos.clone();
        view()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000078))
            .p(px(14.0))
            .items_center()
            .justify_center()
            .child(
                view()
                    .test_id("branch-matrix")
                    .w_full()
                    .h_full()
                    .max_w(px(680.0))
                    .max_h(px(540.0))
                    .rounded(px(9.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        view()
                            .flex_row()
                            .flex_none()
                            .h(px(44.0))
                            .px(px(16.0))
                            .items_center()
                            .border_b_1()
                            .border_color(p.border)
                            .child(
                                text("分支矩阵")
                                    .flex_1()
                                    .text_left()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary),
                            )
                            .child(action("关闭", "branch-close", p, state::close_active_modal)),
                    )
                    .child(
                        view()
                            .flex_col()
                            .flex_none()
                            .gap(px(8.0))
                            .px(px(16.0))
                            .py(px(10.0))
                            .bg(p.rail_bg)
                            .border_b_1()
                            .border_color(p.border)
                            .child(
                                super::controls::text_field(32.0, 12.5, 8.0)
                                    .test_id("branch-filter-input")
                                    .auto_focus(true)
                                    .rounded(px(5.0))
                                    .bg(p.surface)
                                    .border_1()
                                    .border_color(p.border)
                                    .text_color(p.text_primary)
                                    .placeholder("搜索分支或仓库…")
                                    .value(|| global::<BranchFilter>().with(|f| f.0.clone()))
                                    .on_change(|query: &str| {
                                        global::<BranchFilter>()
                                            .set(BranchFilter(query.to_owned()))
                                    }),
                            )
                            .child(
                                view()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(action(
                                        "在所有仓库新建…",
                                        "branch-new-all",
                                        p,
                                        move || create(repos.clone()),
                                    ))
                                    .child(action("分支组合", "branch-presets", p, open_presets))
                                    .child(action(
                                        "全部回默认分支",
                                        "branch-default-all",
                                        p,
                                        move || {
                                            state::close_active_modal();
                                            run(reset
                                                .iter()
                                                .cloned()
                                                .map(|repo| (repo, None))
                                                .collect());
                                        },
                                    )),
                            ),
                    )
                    .child(scroll().flex_1().child(view().p(px(14.0)).child(matrix_rows()))),
            )
            .into_element()
    })
}
