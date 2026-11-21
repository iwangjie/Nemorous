use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use nsh_base::editor;
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use super::workbench::{self, Panel, button};
use crate::state::{self, app_signal, current_palette};
use crate::workbench_mani;
use crate::workbench_process::RunProcess;

#[derive(Default)]
struct Batch {
    pending: VecDeque<(usize, Command)>,
    live: Vec<(usize, RunProcess)>,
    parallelism: usize,
}

#[derive(Default, Clone)]
struct Row {
    name: String,
    status: String,
    data: Vec<u8>,
}

#[derive(Clone, Default)]
struct Results {
    task: String,
    scope: Option<PathBuf>,
    rows: Vec<Row>,
    selected: usize,
    active: bool,
}

pub(super) fn enabled() -> bool {
    app_signal().with(|s| {
        s.cfg
            .extra
            .get("mani_enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
    })
}

pub(super) fn toggle() {
    if enabled() {
        workbench::toggle(Panel::Mani);
    }
}

pub(super) fn open_project(path: PathBuf) {
    if enabled() {
        global::<Results>().update(|r| r.scope = Some(path));
        workbench::reveal(Panel::Mani);
        choose();
    }
}

fn choose() {
    match workbench_mani::load(&workbench::workspace()) {
        Ok(manifest) if !manifest.tasks.is_empty() => {
            state::show_popup_menu(
                manifest
                    .tasks
                    .into_iter()
                    .map(|(name, task)| {
                        let label = if task.desc.is_empty() {
                            name.clone()
                        } else {
                            format!("{name} · {}", task.desc)
                        };
                        MenuItem::new(label).on_select(move || submit(&name))
                    })
                    .collect(),
            );
        }
        Ok(_) => state::show_toast("info", "当前工作区没有 Mani 任务"),
        Err(error) => state::show_toast("warn", format!("无法读取 Mani 配置 · {error}")),
    }
}

fn submit(name: &str) {
    if global::<Results>().with_untracked(|r| r.active) {
        state::show_toast("warn", "Mani 任务仍在运行");
        return;
    }
    let workspace = workbench::workspace();
    let scope = global::<Results>().with_untracked(|r| r.scope.clone());
    let (repos, parallelism) = app_signal().with_untracked(|s| {
        (
            s.repos.clone(),
            s.cfg
                .extra
                .get("concurrency")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(8)
                .clamp(1, 32) as usize,
        )
    });
    match workbench_mani::load(&workspace)
        .and_then(|m| workbench_mani::plan(&workspace, &m, name, &repos, scope.as_deref()))
    {
        Ok(plan) if !plan.is_empty() => {
            let (rows, pending) = plan
                .into_iter()
                .enumerate()
                .map(|(i, (name, cmd))| {
                    (
                        Row {
                            name,
                            status: "等待".into(),
                            data: Vec::new(),
                        },
                        (i, cmd),
                    )
                })
                .unzip();
            workbench::command_log(format!("Mani · {name}"));
            global::<Results>().set(Results {
                task: name.into(),
                scope,
                rows,
                selected: 0,
                active: true,
            });
            workbench::replace_resource(Batch {
                pending,
                live: Vec::new(),
                parallelism,
            });
            poll();
        }
        Ok(_) => state::show_toast("info", "当前范围没有可运行的项目"),
        Err(error) => state::show_toast("warn", error.to_string()),
    }
}

fn poll() {
    let mut done = false;
    workbench::update_resource::<Batch>(|batch| {
        global::<Results>().update(|results| {
            while batch.live.len() < batch.parallelism {
                let Some((index, mut command)) = batch.pending.pop_front() else {
                    break;
                };
                let row = &mut results.rows[index];
                match RunProcess::spawn(&mut command) {
                    Ok(process) => {
                        row.status = "运行中".into();
                        batch.live.push((index, process));
                    }
                    Err(error) => {
                        row.status = "失败".into();
                        row.data = error.to_string().into_bytes();
                    }
                }
            }
            batch.live.retain_mut(|(index, process)| {
                let row = &mut results.rows[*index];
                match process.poll() {
                    Ok(update) => {
                        row.data.extend(update.output);
                        if row.data.len() > 256 * 1024 {
                            row.data.drain(..row.data.len() - 256 * 1024);
                        }
                        if let Some(status) = update.status {
                            let text = String::from_utf8_lossy(&row.data);
                            let label = if !status.success()
                                || text
                                    .lines()
                                    .any(|line| line.to_uppercase().starts_with("FAIL"))
                            {
                                "失败"
                            } else if text.trim_start().to_uppercase().starts_with("SKIP") {
                                "跳过"
                            } else {
                                "完成"
                            };
                            row.status = label.into();
                            return false;
                        }
                        true
                    }
                    Err(error) => {
                        row.status = "失败".into();
                        row.data.extend(error.to_string().bytes());
                        false
                    }
                }
            });
            done = batch.pending.is_empty() && batch.live.is_empty();
            if done {
                results.active = false;
            }
        });
    });
    if done {
        let lines = global::<Results>().with_untracked(|r| {
            r.rows
                .iter()
                .map(|row| format!("{} · {}", row.name, row.status))
                .collect::<Vec<_>>()
        });
        workbench::command_log(lines.join("\n"));
    }
}

pub(super) fn shutdown() {
    workbench::replace_resource(Batch::default());
    global::<Results>().set(Results::default());
}

fn cancel() {
    workbench::replace_resource(Batch::default());
    global::<Results>().update(|results| {
        results.active = false;
        for row in &mut results.rows {
            if matches!(row.status.as_str(), "运行中" | "等待") {
                row.status = "已取消".into();
            }
        }
    });
}

pub(super) fn setup() {
    let active = computed(|| global::<Results>().with(|r| r.active));
    effect(move || {
        if active.get() {
            let timer = set_interval(Duration::from_millis(60), poll);
            cleanup(move || timer.cancel());
        }
    });
}

pub(super) fn body() -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .gap(px(5.0))
        .child(
            view()
                .flex_row()
                .gap(px(5.0))
                .h(px(25.0))
                .flex_none()
                .child(button("选择任务", "mani-select", choose))
                .child(button("全部项目", "mani-all", || {
                    global::<Results>().update(|r| r.scope = None);
                    choose();
                }))
                .child(button("取消", "mani-cancel", cancel))
                .child(button("清空结果", "mani-clear", || {
                    if !global::<Results>().with_untracked(|r| r.active) {
                        global::<Results>().set(Results::default());
                    }
                }))
                .child(
                    text(|| global::<Results>().with(|r| r.task.clone()))
                        .text_size(px(11.0))
                        .text_color(p.accent),
                ),
        )
        .child(
            view()
                .flex_row()
                .flex_1()
                .min_h(px(0.0))
                .child(scroll().w(px(225.0)).flex_none().child(dynamic(move || {
                    let lines = global::<Results>().with(|r| {
                        r.rows
                            .iter()
                            .map(|row| format!("{} · {}", row.name, row.status))
                            .collect::<Vec<_>>()
                    });
                    view()
                        .flex_col()
                        .children(lines.into_iter().enumerate().map(|(index, line)| {
                            view()
                                .h(px(23.0))
                                .px(px(4.0))
                                .cursor(Cursor::Pointer)
                                .on_press(move || {
                                    global::<Results>().update(|r| r.selected = index)
                                })
                                .child(text(line).text_color(p.text_primary).text_size(px(11.0)))
                        }))
                        .into_element()
                })))
                .child(
                    editor()
                        .test_id("mani-output")
                        .read_only(true)
                        .line_numbers(false)
                        .flex_1()
                        .value(|| {
                            global::<Results>().with(|r| {
                                r.rows.get(r.selected).map(|row| {
                                    String::from_utf8_lossy(&row.data).into_owned().into()
                                })
                            })
                        }),
                ),
        )
}
