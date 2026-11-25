use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use nsh_base::editor;
use nsh_core::prelude::*;

use crate::state::{self, app_signal, current_palette};
use crate::workbench_process::RunProcess;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Panel {
    Terminal,
    Run,
    Mani,
    Logs,
}

impl Panel {
    fn title(self) -> &'static str {
        match self {
            Self::Terminal => "终端",
            Self::Run => "运行 / 调试 / 测试",
            Self::Mani => "Mani",
            Self::Logs => "日志",
        }
    }
}

#[derive(Default)]
struct Dock(Option<Panel>);

#[derive(Default)]
struct Running {
    job: Option<RunProcess>,
    stopped: bool,
}

#[derive(Clone, Default)]
struct Output {
    active: bool,
    status: String,
    name: String,
    bytes: Vec<u8>,
    command_log: Vec<String>,
    runtime_log: Vec<String>,
    show_runtime: bool,
}

#[derive(Default)]
struct Resource<T>(RefCell<T>);

pub(super) fn with_resource<T: Default + 'static, R>(get: impl FnOnce(&T) -> R) -> R {
    global::<Resource<T>>().with_untracked(|inner| get(&inner.0.borrow()))
}

pub(super) fn update_resource<T: Default + 'static>(change: impl FnOnce(&mut T)) {
    global::<Resource<T>>().with_untracked(|inner| change(&mut inner.0.borrow_mut()));
}

pub(super) fn replace_resource<T: Default + 'static>(value: T) {
    update_resource(|slot| *slot = value);
}

pub(super) fn active() -> Option<Panel> {
    global::<Dock>().with(|dock| dock.0)
}

pub(super) fn hide() {
    global::<Dock>().set(Dock::default());
}

pub(super) fn reveal(panel: Panel) {
    super::content_search::hide_find_panel();
    app_signal().update(|state| state.show_git_panel = false);
    global::<Dock>().set(Dock(Some(panel)));
}

pub(super) fn toggle(panel: Panel) {
    if active() == Some(panel) {
        hide();
    } else {
        reveal(panel);
    }
}

pub(super) fn workspace() -> PathBuf {
    app_signal().with_untracked(|state| PathBuf::from(&state.cfg.workspace_path))
}

pub(super) fn command_log(line: String) {
    global::<Output>().update(|output| append_log(&mut output.command_log, line));
}

pub(super) fn runtime_log(line: String) {
    global::<Output>().update(|output| append_log(&mut output.runtime_log, line));
}

fn append_log(lines: &mut Vec<String>, line: String) {
    if lines.len() == 200 {
        lines.remove(0);
    }
    lines.push(line.chars().take(4096).collect());
}

pub(super) fn is_running() -> bool {
    global::<Output>().with(|output| output.active)
}

pub(super) fn has_active_jobs() -> bool {
    is_running() || super::debug_panel::running() || super::mani_panel::running()
}

pub(super) fn launch(label: String, mut command: Command) {
    if is_running() {
        state::show_toast("warn", "已有运行任务");
        return;
    }
    match RunProcess::spawn(&mut command) {
        Ok(process) => {
            command_log(format!("{label} · 已启动"));
            global::<Output>().update(|output| {
                output.active = true;
                output.name = label;
                output.bytes.clear();
                output.status = "运行中".into();
            });
            replace_resource(Running {
                job: Some(process),
                stopped: false,
            });
        }
        Err(error) => {
            let error = format!("{label} · {error:#}");
            runtime_log(error.clone());
            state::show_toast("warn", error);
        }
    }
}

pub(super) fn stop() {
    update_resource::<Running>(|running| {
        if let Some(job) = &mut running.job {
            job.stop();
            running.stopped = true;
        }
    });
}

fn poll_command() {
    let mut polled = None;
    update_resource::<Running>(|running| {
        if let Some(job) = &mut running.job {
            polled = Some((job.poll(), running.stopped));
        }
    });
    let Some((result, cancelled)) = polled else {
        return;
    };
    let update = match result {
        Ok(update) => update,
        Err(error) => {
            replace_resource(Running::default());
            let status = format!("命令输出中断 · {error:#}");
            global::<Output>().update(|output| { output.active = false; output.status = status.clone(); });
            runtime_log(status);
            return;
        }
    };
    if !update.output.is_empty() || update.truncated {
        global::<Output>().update(|output| {
            if update.truncated {
                output
                    .bytes
                    .extend_from_slice("\n[部分输出已省略]\n".as_bytes());
            }
            output.bytes.extend(update.output);
            if output.bytes.len() > 512 * 1024 {
                let start = output.bytes.len() - 512 * 1024;
                output.bytes.drain(..start);
            }
        });
    }
    if let Some(status) = update.status {
        let status = if cancelled {
            "已停止".into()
        } else if let Some(code) = status.code() {
            format!("退出码 {code}")
        } else {
            "进程已终止".into()
        };
        let label = global::<Output>().with_untracked(|output| output.name.clone());
        global::<Output>().update(|output| { output.active = false; output.status = status.clone(); });
        if cancelled || !update.status.is_some_and(|status| status.success()) {
            runtime_log(format!("{label} · {status}"));
        }
        command_log(format!("{label} · {status}"));
        replace_resource(Running::default());
    }
}

pub(super) fn setup() {
    let running = computed(is_running);
    effect(move || {
        if running.get() {
            let timer = set_interval(Duration::from_millis(33), poll_command);
            cleanup(move || timer.cancel());
        }
    });
    let directory = computed(|| app_signal().with(|state| state.cfg.workspace_path.clone()));
    effect(move || {
        let _ = directory.get();
        cleanup(move || {
            replace_resource(Running::default());
            global::<Output>().update(|output| output.active = false);
            super::terminal_panel::shutdown();
            super::mani_panel::shutdown();
            super::debug_panel::stop();
        });
    });
}

pub(super) fn button(
    label: &'static str,
    id: &'static str,
    action: impl Fn() + 'static,
) -> impl IntoElement + Interactive {
    let p = current_palette();
    super::controls::button()
        .test_id(id)
        .label(label)
        .rounded(px(4.0))
        .h(px(24.0))
        .px(px(7.0))
        .bg(p.surface2)
        .cursor(Cursor::Pointer)
        .disabled_style(|style| style.opacity(0.5))
        .on_press(action)
        .child(text(label).text_color(p.text_control).text_size(px(11.0)))
}

pub(super) fn output() -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(
            text(|| global::<Output>().with(|output| output.status.clone()))
                .test_id("workbench-run-status")
                .text_color(p.text_muted)
                .text_size(px(11.0)),
        )
        .child(
            editor()
                .test_id("workbench-run-output")
                .read_only(true)
                .line_numbers(false)
                .flex_1()
                .value(|| {
                    global::<Output>().with(|output| {
                        Some(String::from_utf8_lossy(&output.bytes).into_owned().into())
                    })
                }),
        )
}

fn logs() -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .gap(px(4.0))
        .child(
            view()
                .flex_row()
                .gap(px(4.0))
                .child(button("命令", "workbench-logs-commands", || {
                    global::<Output>().update(|log| log.show_runtime = false)
                }))
                .child(button("运行状态", "workbench-logs-runtime", || {
                    global::<Output>().update(|log| log.show_runtime = true)
                }))
                .child(button("清空", "workbench-logs-clear", || {
                    global::<Output>().update(|log| {
                        if log.show_runtime {
                            log.runtime_log.clear();
                        } else {
                            log.command_log.clear();
                        }
                    })
                })),
        )
        .child(
            editor()
                .test_id("workbench-log-output")
                .read_only(true)
                .line_numbers(false)
                .flex_1()
                .value(|| {
                    global::<Output>().with(|log| {
                        Some(
                            if log.show_runtime {
                                log.runtime_log.join("\n")
                            } else {
                                log.command_log.join("\n")
                            }
                            .into(),
                        )
                    })
                }),
        )
}

pub(super) fn height() -> Points {
    px(app_signal().with(|state| match active() {
        Some(Panel::Terminal) => state.cfg.terminal_panel_height,
        Some(Panel::Run) => state
            .cfg
            .extra
            .get("run_panel_height")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(270.0) as f32,
        _ => state
            .cfg
            .extra
            .get("workbench_aux_height")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(245.0) as f32,
    }))
}

pub(super) fn resize(height: &Points) {
    let panel = active();
    let height = height.0;
    app_signal().update(|state| match panel {
        Some(Panel::Terminal) => state.cfg.terminal_panel_height = height,
        Some(Panel::Run) => {
            state
                .cfg
                .extra
                .insert("run_panel_height".into(), serde_json::json!(height));
        }
        _ => {
            state
                .cfg
                .extra
                .insert("workbench_aux_height".into(), serde_json::json!(height));
        }
    });
}

pub(super) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some(panel) = active() else {
            return view().into_element();
        };
        let p = current_palette();
        view()
            .test_id("workbench-dock")
            .w_full()
            .h_full()
            .flex_col()
            .border_t_1()
            .border_color(p.border_strong)
            .bg(p.surface)
            .overflow_hidden()
            .child(
                view()
                    .h(px(30.0))
                    .px(px(8.0))
                    .flex_none()
                    .flex_row()
                    .items_center()
                    .child(
                        text(panel.title())
                            .flex_1()
                            .text_color(p.text_primary)
                            .text_size(px(12.0)),
                    )
                    .child(button("隐藏", "workbench-hide", hide)),
            )
            .child(
                view()
                    .px(px(7.0))
                    .pb(px(5.0))
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_col()
                    .child(match panel {
                        Panel::Terminal => super::terminal_panel::body().into_element(),
                        Panel::Run => super::run_panel::body().into_element(),
                        Panel::Mani => super::mani_panel::body().into_element(),
                        Panel::Logs => logs().into_element(),
                    }),
            )
            .into_element()
    })
}
