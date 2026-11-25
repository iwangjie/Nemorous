use std::path::PathBuf;

use anyhow::{Result, ensure};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git_history_actions as history;
use crate::git_rebase_plan::{self, RebaseStep, StepAction};
use crate::state::{self, app_signal, current_palette};

#[derive(Clone)]
struct Row {
    step: RebaseStep,
    subject: String,
}

#[derive(Default, Clone)]
struct Plan {
    repo: Option<PathBuf>,
    head: String,
    upstream: String,
    rows: Vec<Row>,
}

pub(super) fn prompt(repo: PathBuf) {
    super::git_prompt::open(
        "交互式 rebase · 基线分支或提交",
        "main".into(),
        move |input| {
            ensure!(!input.is_empty(), "请输入 rebase 基线");
            let input = input.to_owned();
            let path = repo.clone();
            state::spawn_from_root(async move {
                let run = path.clone();
                let data = blocking(move || -> Result<_> {
                    let head = history::resolve_commit(&run, "HEAD")?;
                    let upstream = history::resolve_commit(&run, &input)?;
                    let rows = git_rebase_plan::list_plan(&run, &upstream)?
                        .into_iter()
                        .map(|item| Row {
                            step: RebaseStep {
                                hash: item.hash,
                                action: StepAction::Pick,
                            },
                            subject: item.message.trim_end().to_owned(),
                        })
                        .collect::<Vec<_>>();
                    Ok((head, upstream, rows))
                })
                .await;
                match data {
                    Ok(Ok((head, upstream, rows))) if !rows.is_empty() => {
                        global::<Plan>().set(Plan {
                            head,
                            upstream,
                            rows,
                            repo: Some(path),
                        });
                        app_signal().update(state::enter_modal_focus);
                    }
                    Ok(Ok(_)) => state::show_toast("info", "没有待重放的提交"),
                    Ok(Err(err)) => state::show_toast("warn", err.to_string()),
                    Err(err) => state::show_toast("err", err.to_string()),
                }
            });
            Ok(())
        },
    );
}

pub(crate) fn close_active() -> bool {
    let plan = global::<Plan>();
    let open = plan.with_untracked(|p| p.repo.is_some());
    if open {
        plan.set(Plan::default());
    }
    open
}

fn execute() {
    let plan = global::<Plan>().with_untracked(Clone::clone);
    let Some(repo) = plan.repo else {
        return;
    };
    let path = repo.clone();
    let steps = plan
        .rows
        .into_iter()
        .map(|row| row.step)
        .collect::<Vec<_>>();
    let started = state::run_git_action(
        move || git_rebase_plan::run_plan(&path, &plan.head, &plan.upstream, &steps),
        move |result| match result {
            Ok(()) => state::show_toast("ok", "交互式 rebase 已完成"),
            Err(err) => {
                state::show_toast("warn", err.to_string());
                if history::active_operation(&repo).ok().flatten().is_some() {
                    super::conflict_workbench::open(super::conflict_workbench::Source::Repo(repo));
                }
            }
        },
    );
    if started {
        state::close_active_modal();
    }
}

fn select_action(hash: String, subject: String) {
    let menu = [
        ("Pick · 保留", StepAction::Pick),
        ("Reword · 改写说明", StepAction::Reword(subject)),
        ("Squash · 合并到上一条", StepAction::Squash),
        ("Drop · 跳过", StepAction::Drop),
    ]
    .into_iter()
    .map(|(label, action)| {
        let hash = hash.clone();
        MenuItem::new(label).on_select(move || {
            global::<Plan>().update(|p| {
                if let Some(row) = p.rows.iter_mut().find(|row| row.step.hash == hash) {
                    row.step.action = action.clone();
                }
            });
        })
    })
    .collect();
    state::show_popup_menu(menu);
}

fn move_row(hash: &str, direction: isize) {
    global::<Plan>().update(|p| {
        let Some(index) = p.rows.iter().position(|row| row.step.hash == hash) else {
            return;
        };
        let new = index
            .saturating_add_signed(direction)
            .min(p.rows.len().saturating_sub(1));
        p.rows.swap(index, new);
    });
}

fn button(label: &'static str, id: &str, handler: impl Fn() + 'static) -> impl IntoElement {
    let p = current_palette();
    super::controls::button()
        .label(label)
        .test_id(id)
        .px(px(7.0))
        .h(px(25.0))
        .rounded(px(4.0))
        .cursor(Cursor::Pointer)
        .bg(p.surface2)
        .on_press(handler)
        .child(text(label).text_color(p.text_primary).text_size(px(11.0)))
}

fn rows() -> impl IntoElement {
    let ordered = computed(|| {
        global::<Plan>().with(|p| {
            p.rows
                .iter()
                .map(|row| row.step.hash.clone())
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let p = current_palette();
        let rows = ordered.get();
        view().flex_col().children(rows.into_iter().map(|hash| {
            let subject = global::<Plan>().with_untracked(|p| {
                p.rows
                    .iter()
                    .find(|row| row.step.hash == hash)
                    .map(|row| row.subject.clone())
                    .unwrap_or_default()
            });
            let action_id = hash.clone();
            let up = hash.clone();
            let down = hash.clone();
            let editor_id = hash.clone();
            let text_id = hash.clone();
            let flag = computed({
                let hash = hash.clone();
                move || {
                    global::<Plan>().with(|p| {
                        p.rows.iter().find(|row| row.step.hash == hash).map(|row| {
                            match row.step.action {
                                StepAction::Pick => "Pick",
                                StepAction::Reword(_) => "Reword",
                                StepAction::Squash => "Squash",
                                StepAction::Drop => "Drop",
                            }
                        })
                    })
                }
            });
            view()
                .flex_col()
                .gap(px(4.0))
                .px(px(5.0))
                .py(px(4.0))
                .border_b_1()
                .border_color(p.border)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .child(button(
                            "步骤…",
                            &format!("rebase-action-{hash}"),
                            move || select_action(action_id.clone(), subject.clone()),
                        ))
                        .child(
                            text(move || flag.get().unwrap_or(""))
                                .text_color(p.accent)
                                .text_size(px(11.0)),
                        )
                        .child(
                            text(hash[..8].to_owned())
                                .text_color(p.text_muted)
                                .text_size(px(10.5)),
                        )
                        .child(
                            text(global::<Plan>().with_untracked(|p| {
                                p.rows
                                    .iter()
                                    .find(|row| row.step.hash == hash)
                                    .and_then(|row| row.subject.lines().next())
                                    .unwrap_or("")
                                    .to_owned()
                            }))
                            .flex_1()
                            .text_color(p.text_primary)
                            .number_of_lines(1u32),
                        )
                        .child(button("上移", &format!("rebase-up-{hash}"), move || {
                            move_row(&up, -1)
                        }))
                        .child(button(
                            "下移",
                            &format!("rebase-down-{hash}"),
                            move || move_row(&down, 1),
                        )),
                )
                .child(when(
                    move || flag.get() == Some("Reword"),
                    move || {
                        let hash = editor_id.clone();
                        let value = text_id.clone();
                        super::controls::text_field(25.0, 11.5, 6.0)
                            .test_id(&format!("rebase-word-{value}"))
                            .bg(p.surface2)
                            .placeholder("提交说明")
                            .text_color(p.text_primary)
                            .value(move || {
                                global::<Plan>().with(|p| {
                                    p.rows
                                        .iter()
                                        .find(|row| row.step.hash == value)
                                        .and_then(|row| match &row.step.action {
                                            StepAction::Reword(message) => Some(message.clone()),
                                            _ => None,
                                        })
                                        .unwrap_or_default()
                                })
                            })
                            .on_change(move |text: &str| {
                                global::<Plan>().update(|p| {
                                    if let Some(row) =
                                        p.rows.iter_mut().find(|row| row.step.hash == hash)
                                    {
                                        row.step.action = StepAction::Reword(text.to_owned());
                                    }
                                });
                            })
                    },
                ))
        }))
    })
}

pub(super) fn layer() -> impl IntoElement {
    let open = computed(|| global::<Plan>().with(|p| p.repo.is_some()));
    when(
        move || open.get(),
        || {
            let p = current_palette();
            view()
                .absolute()
                .inset_0()
                .p(px(12.0))
                .bg(rgba(0x00000070))
                .items_center()
                .justify_center()
                .child(
                    view()
                        .test_id("git-rebase-plan")
                        .w_full()
                        .max_w(px(680.0))
                        .h_full()
                        .max_h(px(550.0))
                        .flex_col()
                        .px(px(10.0))
                        .py(px(8.0))
                        .gap(px(6.0))
                        .border_1()
                        .border_color(p.border_strong)
                        .bg(p.surface)
                        .rounded(px(8.0))
                        .child(
                            text("交互式 rebase")
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_primary),
                        )
                        .child(scroll().flex_1().child(rows()))
                        .child(
                            view()
                                .flex_row()
                                .justify_end()
                                .gap(px(5.0))
                                .child(button(
                                    "取消",
                                    "rebase-plan-cancel",
                                    state::close_active_modal,
                                ))
                                .child(button("执行计划", "rebase-plan-run", execute)),
                        ),
                )
        },
    )
}
