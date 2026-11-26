use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use super::icons::{self, ICO_CLOSE};
use crate::git::RepoInfo;
use crate::git_remotes::{self, RemoteAction, RemoteStatus, Report};
use crate::state::{self, app_signal, current_palette};

#[derive(Clone, Default)]
struct SyncDialog {
    open: bool,
    state: Option<SyncState>,
}

#[derive(Clone)]
struct SyncState {
    action: RemoteAction,
    stop: Arc<AtomicBool>,
    reports: Option<Vec<Report>>,
}

#[derive(Clone)]
struct PullProgress {
    done: usize,
    total: usize,
    current: String,
    stop: Arc<AtomicBool>,
}

pub(super) fn label(action: RemoteAction) -> &'static str {
    match action {
        RemoteAction::Fetch => "获取",
        RemoteAction::Pull => "拉取",
        RemoteAction::Push => "推送",
    }
}

pub(super) fn start_all(action: RemoteAction) {
    let repos = app_signal().with_untracked(|s| s.repos.clone());
    start(repos, action);
}

pub(super) fn start(repos: Vec<RepoInfo>, action: RemoteAction) {
    if repos.is_empty() {
        state::show_toast("warn", "工作区没有可用仓库");
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let worker = stop.clone();
    let parallelism = app_signal().with_untracked(|s| s.cfg.parallelism());
    let dialog = global::<SyncDialog>();
    let progress_signal = global::<Option<PullProgress>>();
    let background = action == RemoteAction::Pull;
    let total = repos.len();
    let shared_progress = Arc::new(Mutex::new((0usize, String::new())));
    let worker_progress = shared_progress.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_flag = finished.clone();

    if !state::run_git_action(
        move || {
            git_remotes::run_batch_with_progress(&repos, action, &worker, parallelism, |done, _, name| {
                let mut guard = worker_progress.lock().unwrap_or_else(|e| e.into_inner());
                *guard = (done, name.to_owned());
            })
        },
        move |reports| {
            finished_flag.store(true, Ordering::Release);
            if background {
                progress_signal.set(None);
            }
            let problems = reports
                .iter()
                .filter(|report| report.status == RemoteStatus::Failed)
                .count();
            let done = reports
                .iter()
                .filter(|report| report.status == RemoteStatus::Done)
                .count();
            let skipped = reports
                .iter()
                .filter(|report| report.status == RemoteStatus::Skipped)
                .count();
            let cancelled = reports.len() - problems - done - skipped;
            super::workbench::command_log(format!(
                "{} · 成功 {done}，跳过 {skipped}，失败 {problems}，取消 {cancelled}",
                label(action)
            ));
            for report in reports.iter().filter(|report| report.status == RemoteStatus::Failed) {
                super::workbench::runtime_log(format!("{} · {}", report.repo.label(), report.detail));
            }
            let toast_msg = if background && reports.len() == 1 {
                let report = &reports[0];
                format!("拉取{}：{} · {}", match report.status {
                    RemoteStatus::Done => "完成",
                    RemoteStatus::Skipped => "已跳过",
                    RemoteStatus::Failed => "失败",
                    RemoteStatus::Cancelled => "已取消",
                }, report.repo.label(), report.detail)
            } else {
                format!(
                    "{}完成：成功 {done}，跳过 {skipped}，失败 {problems}，取消 {cancelled}",
                    label(action)
                )
            };
            dialog.update(|dialog| {
                if let Some(batch) = &mut dialog.state {
                    batch.reports = Some(reports);
                }
            });
            state::show_toast(
                if problems > 0 { "warn" } else { "ok" },
                toast_msg,
            );
        },
    ) {
        return;
    }

    dialog.set(SyncDialog {
        open: !background,
        state: Some(SyncState {
            action,
            stop: stop.clone(),
            reports: None,
        }),
    });
    if background {
        progress_signal.set(Some(PullProgress {
            done: 0,
            total,
            current: String::new(),
            stop,
        }));
        let finished_timer = finished.clone();
        let timer = set_interval(Duration::from_millis(80), move || {
            if finished_timer.load(Ordering::Acquire) {
                return;
            }
            let (done, current) = {
                let guard = shared_progress.lock().unwrap_or_else(|e| e.into_inner());
                guard.clone()
            };
            progress_signal.update(|slot| {
                if let Some(progress) = slot {
                    progress.done = done;
                    progress.current = current;
                }
            });
        });
        state::spawn_from_root(async move {
            while !finished.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
            timer.cancel();
        });
    } else {
        app_signal().update(state::enter_modal_focus);
    }
}

pub(super) fn status_progress() -> impl IntoElement {
    dynamic(|| {
        let Some(progress) = global::<Option<PullProgress>>().get() else {
            return view().into_element();
        };
        let p = current_palette();
        let ratio = if progress.total <= 1 || progress.done == 0 {
            0.25
        } else {
            (progress.done as f32 / progress.total as f32).clamp(0.12, 1.0)
        };
        let label = if progress.total > 1 {
            format!("拉取中 {}/{}", progress.done, progress.total)
        } else {
            "拉取中…".to_owned()
        };
        let stop = progress.stop;
        view()
            .test_id("git-pull-progress")
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .mr(px(8.0))
            .px(px(7.0))
            .h(px(19.0))
            .rounded(px(4.0))
            .bg(p.surface2)
            .child(
                text(label)
                    .text_size(px(10.5))
                    .text_color(p.text_secondary)
                    .number_of_lines(1u32),
            )
            .child(
                view()
                    .w(px(68.0))
                    .h(px(4.0))
                    .rounded(px(2.0))
                    .bg(p.surface3)
                    .overflow_hidden()
                    .child(
                        view()
                            .w(px((68.0 * ratio).round()))
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .bg(p.accent),
                    ),
            )
            .child(
                super::controls::button()
                    .test_id("git-pull-cancel")
                    .label("取消拉取")
                    .w(px(14.0))
                    .h(px(14.0))
                    .rounded(px(3.0))
                    .cursor(Cursor::Pointer)
                    .hover(move |s| s.bg(p.surface_hover))
                    .on_press(move || stop.store(true, Ordering::Release))
                    .child(icons::icon(ICO_CLOSE, 9.0, p.text_muted)),
            )
            .into_element()
    })
}

pub(crate) fn close_active() -> bool {
    let dialog = global::<SyncDialog>();
    let open = dialog.with_untracked(|dialog| dialog.open);
    if open {
        dialog.update(|dialog| dialog.open = false);
    }
    open
}

fn show_existing() {
    global::<SyncDialog>().update(|dialog| dialog.open = true);
    app_signal().update(state::enter_modal_focus);
}

pub(super) fn menu(repo: Option<RepoInfo>) -> Vec<MenuItem> {
    [RemoteAction::Fetch, RemoteAction::Pull, RemoteAction::Push]
        .into_iter()
        .map(|action| {
            let repo = repo.clone();
            let scope = if repo.is_some() {
                "当前仓库"
            } else {
                "全部仓库"
            };
            MenuItem::new(format!("{}{scope}", label(action))).on_select(move || {
                if let Some(repo) = &repo {
                    start(vec![repo.clone()], action);
                } else {
                    start_all(action);
                }
            })
        })
        .collect()
}

pub(super) fn recent_result_item() -> Option<MenuItem> {
    global::<SyncDialog>().with_untracked(|dialog| {
        dialog.state.is_some()
            .then(|| MenuItem::new("查看最近操作结果").on_select(show_existing))
    })
}

fn button(label: &'static str, id: &'static str, handler: impl Fn() + 'static) -> impl IntoElement {
    let p = current_palette();
    super::controls::button()
        .label(label)
        .test_id(id)
        .px(px(9.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .bg(p.surface2)
        .cursor(Cursor::Pointer)
        .hover(move |s| s.bg(p.surface_hover))
        .on_press(handler)
        .child(text(label).text_color(p.text_primary))
}

pub(super) fn layer() -> impl IntoElement {
    dynamic(|| {
        let batch = global::<SyncDialog>()
            .with(|dialog| dialog.open.then(|| dialog.state.clone()).flatten());
        let Some(batch) = batch else {
            return view().into_element();
        };
        let p = current_palette();
        let action = batch.action;
        let mut panel = view()
            .test_id("git-sync-dialog")
            .w(px(500.0))
            .h(px(380.0))
            .px(px(16.0))
            .py(px(12.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(p.border_strong)
            .gap(px(10.0))
            .bg(p.surface)
            .flex_col()
            .child(
                text(format!("{}仓库", label(action)))
                    .text_size(px(14.0))
                    .text_color(p.text_primary),
            );
        if let Some(reports) = batch.reports {
            let retry = reports
                .iter()
                .filter(|r| matches!(r.status, RemoteStatus::Failed | RemoteStatus::Cancelled))
                .map(|r| r.repo.clone())
                .collect::<Vec<_>>();
            panel = panel.child(
                scroll()
                    .flex_1()
                    .child(
                        view()
                            .flex_col()
                            .gap(px(6.0))
                            .children(reports.into_iter().map(|report| {
                                let (status, color) = match report.status {
                                    RemoteStatus::Done => ("成功", p.green),
                                    RemoteStatus::Skipped => ("跳过", p.text_muted),
                                    RemoteStatus::Failed => ("失败", p.danger),
                                    RemoteStatus::Cancelled => ("取消", p.amber),
                                };
                                view()
                                    .test_id(&format!("git-sync-result-{}", report.repo.id))
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        text(format!("{} · {status}", report.repo.label()))
                                            .text_color(color),
                                    )
                                    .child(
                                        text(report.detail)
                                            .text_size(px(11.0))
                                            .text_color(p.text_secondary),
                                    )
                            })),
                    ),
            );
            if !retry.is_empty() {
                panel = panel.child(button(
                    "重试未完成仓库",
                    "git-sync-retry",
                    move || start(retry.clone(), action),
                ));
            }
        } else {
            let stop = batch.stop;
            panel = panel
                .child(text("正在处理仓库…").text_color(p.text_muted))
                .child(button("取消操作", "git-sync-cancel", move || {
                    stop.store(true, Ordering::Release)
                }));
        }
        view()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000070))
            .items_center()
            .justify_center()
            .child(panel.child(button("关闭", "git-sync-close", state::close_active_modal)))
            .into_element()
    })
}
