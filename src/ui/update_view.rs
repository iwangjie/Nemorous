use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use nsh_core::element::desktop::{restart, window_state};
use nsh_core::prelude::*;

use super::{
    settings,
    workbench::{self, button},
};
use crate::state::{self, app_signal, current_palette};
use crate::update_manifest::Manifest;
use crate::update_runtime::{self, Staged};

#[derive(Clone, Default)]
struct Update {
    checking: bool,
    pending: Option<Manifest>,
    cancel: Option<Arc<AtomicBool>>,
    staged: bool,
    deferred: bool,
    status: String,
    backup: Option<std::path::PathBuf>,
    inactive_since: Option<Instant>,
}

#[derive(Default)]
struct Prepared(Option<Staged>);

fn status() -> Signal<Update> {
    global::<Update>()
}

fn busy() -> bool {
    app_signal().with_untracked(|s| {
        s.command_busy
            || s.code_tabs.iter().any(|tab| tab.is_dirty())
            || s.working_diff.as_ref().is_some_and(|diff| diff.is_dirty())
    }) || workbench::has_active_jobs()
        || super::conflict_workbench::is_open()
}

pub(super) fn check(manual: bool) {
    if update_runtime::current_bundle().is_none() {
        if manual {
            status().update(|s| s.status = "当前安装方式由包管理器更新".into());
        }
        return;
    }
    let (auto, mirror, skip) = app_signal().with_untracked(|s| {
        (
            s.cfg.extra_bool("auto_update", true),
            s.cfg.extra_str("update_mirror", "").to_owned(),
            s.cfg.extra_str("skipped_update_version", "").to_owned(),
        )
    });
    if (!auto && !manual)
        || status()
            .with_untracked(|s| s.checking || s.cancel.is_some() || s.staged || s.backup.is_some())
    {
        return;
    }
    status().update(|s| {
        s.checking = true;
        if manual {
            s.status = "正在检查更新…".into();
            s.deferred = false;
        }
    });
    state::spawn_from_root(async move {
        let result = blocking(move || update_runtime::check(&mirror, Some(&skip))).await;
        let mut available = None;
        status().update(|s| {
            s.checking = false;
            match result {
                Ok(Ok(Some(manifest))) => {
                    let same = s
                        .pending
                        .as_ref()
                        .is_some_and(|previous| previous.version == manifest.version);
                    if !same {
                        s.deferred = false;
                        available = Some(manifest.version.clone());
                    }
                    s.status = format!("发现新版本 {}", manifest.version);
                    s.pending = Some(manifest);
                }
                Ok(Ok(None)) => {
                    s.pending = None;
                    if manual {
                        s.status = "当前没有可用更新".into();
                    }
                }
                Ok(Err(error)) if manual => s.status = error.to_string(),
                Err(error) if manual => s.status = error.to_string(),
                _ => {}
            }
        });
        if !manual {
            if let Some(version) = available {
                state::show_toast("info", format!("Nemorous {version} 可更新"));
            }
        }
    });
}

fn idle_ready(allowed: bool, focused: bool, elapsed: Duration, busy: bool) -> bool {
    allowed && !focused && elapsed >= Duration::from_secs(30) && !busy
}

fn idle_poll() {
    let focused = window_state().is_none_or(|window| window.focused);
    let allow = app_signal().with_untracked(|s| {
        s.cfg.extra_bool("auto_update", true) && s.cfg.extra_bool("auto_update_idle", true)
    });
    let mut prepare = false;
    let mut apply = false;
    let active = busy();
    status().update(|s| {
        if focused {
            s.inactive_since = None;
            return;
        }
        let since = s.inactive_since.get_or_insert_with(Instant::now);
        if idle_ready(allow, focused, since.elapsed(), active)
            && !s.deferred
            && s.pending.is_some()
            && s.cancel.is_none()
        {
            apply = s.staged;
            prepare = !apply;
        }
    });
    if apply {
        install(true);
    } else if prepare {
        download(true);
    }
}

pub(super) fn setup() {
    let period = computed(|| {
        app_signal().with(|s| {
            s.cfg.extra_bool("auto_update", true).then(|| {
                let minutes = s
                    .cfg
                    .extra
                    .get("update_check_minutes")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(30)
                    .clamp(5, 1440);
                Duration::from_secs(minutes * 60)
            })
        })
    });
    effect(move || {
        if let Some(period) = period
            .get()
            .filter(|_| update_runtime::current_bundle().is_some())
        {
            let initial = set_timeout(Duration::from_secs(20), || check(false));
            let periodic = set_interval(period, || check(false));
            cleanup(move || {
                initial.cancel();
                periodic.cancel();
            });
        }
    });
    let pending = computed(|| status().with(|s| s.pending.is_some()));
    effect(move || {
        if pending.get() {
            let timer = set_interval(Duration::from_secs(1), idle_poll);
            cleanup(move || timer.cancel());
        }
    });
    cleanup(move || {
        if let Some(cancel) = status().with_untracked(|s| s.cancel.clone()) {
            cancel.store(true, Ordering::Release);
        }
        workbench::replace_resource(Prepared::default());
    });
}

fn cancel() {
    let deferred = status().with_untracked(|s| s.cancel.clone());
    if let Some(cancel) = deferred {
        cancel.store(true, Ordering::Release);
    }
    status().update(|s| {
        s.deferred = true;
        s.status = "已推迟本次更新".into();
    });
}

fn skip() {
    let Some(version) =
        status().with_untracked(|s| s.pending.as_ref().map(|manifest| manifest.version.clone()))
    else {
        return;
    };
    cancel();
    workbench::replace_resource(Prepared::default());
    app_signal().update(|s| {
        s.cfg
            .extra
            .insert("skipped_update_version".into(), version.into());
    });
    state::save_config_quiet();
    status().update(|s| {
        s.pending = None;
        s.staged = false;
        s.status = "已跳过此版本".into();
    });
}

fn download(automatic: bool) {
    let Some(current) = update_runtime::current_bundle() else {
        return;
    };
    let Some(manifest) = status().with_untracked(|s| {
        if s.cancel.is_none() {
            s.pending.clone()
        } else {
            None
        }
    }) else {
        return;
    };
    let mirror = app_signal().with_untracked(|s| s.cfg.extra_str("update_mirror", "").to_owned());
    let cancel = Arc::new(AtomicBool::new(false));
    status().update(|s| {
        if !automatic {
            s.deferred = false;
        }
        s.cancel = Some(Arc::clone(&cancel));
        s.status = format!("正在下载并校验 {}", manifest.version);
    });
    state::spawn_from_root(async move {
        let token = Arc::clone(&cancel);
        let result =
            blocking(move || update_runtime::stage(&current, &manifest, &mirror, &token)).await;
        status().update(|s| {
            s.cancel = None;
        });
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match result {
            Ok(Ok(staged)) => {
                workbench::replace_resource(Prepared(Some(staged)));
                status().update(|s| {
                    s.staged = true;
                    s.status = "签名已通过，可以安装并重启".into();
                });
                if automatic {
                    idle_poll();
                }
            }
            Ok(Err(error)) => status().update(|s| {
                s.deferred = true;
                s.status = error.to_string();
            }),
            Err(error) => status().update(|s| {
                s.deferred = true;
                s.status = error.to_string();
            }),
        }
    });
}

fn install(automatic: bool) {
    if busy() || (automatic && window_state().is_none_or(|w| w.focused)) {
        status().update(|s| s.status = "编辑或任务仍在进行，更新将在空闲时继续".into());
        return;
    }
    let mut staged = None;
    workbench::update_resource::<Prepared>(|value| staged = value.0.take());
    let Some(staged) = staged else {
        return;
    };
    status().update(|s| {
        s.staged = false;
        s.deferred = true;
    });
    // The runner must relaunch the install path after this bundle moves to backup.
    nsh_core::restart::remember_program();
    let result = staged.apply();
    match result {
        Ok(backup) => {
            status().update(|s| {
                s.backup = Some(backup.clone());
                s.status = "已安装，原版本保存在应用目录".into();
            });
            state::save_config_quiet();
            restart();
        }
        Err(error) => status().update(|s| s.status = error.to_string()),
    }
}

pub(super) fn body() -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_col()
        .child(settings::section(
            "自动更新",
            vec![
                settings::extra_toggle("自动检查更新", "auto_update", true).into_element(),
                settings::extra_toggle("后台空闲时安装", "auto_update_idle", true).into_element(),
                settings::extra_number("检查间隔（分钟）", "update_check_minutes", 30, 5, 1440)
                    .into_element(),
                settings::extra_text("更新镜像", "update_mirror", "").into_element(),
            ],
        ))
        .child(
            text(format!("当前版本 {}", update_runtime::current_version()))
                .text_color(p.text_secondary)
                .text_size(px(12.0))
                .px(px(8.0))
                .pb(px(8.0)),
        )
        .child(
            view()
                .flex_row()
                .gap(px(5.0))
                .px(px(8.0))
                .pb(px(8.0))
                .child(button("立即检查", "update-check", || check(true)))
                .child(dynamic(|| {
                    let (pending, loading, staged) =
                        status().with(|s| (s.pending.is_some(), s.cancel.is_some(), s.staged));
                    let mut row = view().flex_row().gap(px(5.0));
                    if loading {
                        row = row.child(button("取消下载", "update-cancel", cancel));
                    } else if pending {
                        row = row
                            .child(button(
                                if staged {
                                    "安装并重启"
                                } else {
                                    "下载并验证"
                                },
                                "update-install",
                                move || {
                                    if staged {
                                        install(false);
                                    } else {
                                        download(false);
                                    }
                                },
                            ))
                            .child(button("稍后", "update-later", cancel))
                            .child(button("跳过版本", "update-skip", skip));
                    }
                    row.into_element()
                })),
        )
        .child(
            text(|| status().with(|s| s.status.clone()))
                .test_id("update-status")
                .px(px(8.0))
                .text_color(p.text_muted),
        )
        .child(
            text(|| {
                status().with(|s| {
                    s.pending
                        .as_ref()
                        .map(|manifest| manifest.notes.clone())
                        .unwrap_or_default()
                })
            })
            .px(px(8.0))
            .pt(px(8.0))
            .text_color(p.text_primary),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_update_waits_for_background_and_active_work() {
        assert!(!idle_ready(true, true, Duration::from_secs(30), false));
        assert!(!idle_ready(true, false, Duration::from_secs(29), false));
        assert!(!idle_ready(true, false, Duration::from_secs(30), true));
        assert!(!idle_ready(false, false, Duration::from_secs(30), false));
        assert!(idle_ready(true, false, Duration::from_secs(30), false));
    }
}
