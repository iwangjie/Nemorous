use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, ensure};
use nsh_base::{editor, input};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use super::workbench::{self, Panel, button};
use crate::state::{self, app_signal, current_palette};
use crate::workbench_run::{self, Kind, RunConfig};

#[derive(Clone, Default)]
struct Selected(String);

#[derive(Clone, Default)]
struct Draft {
    config: Option<RunConfig>,
    env: String,
}

pub(super) fn open() {
    workbench::reveal(Panel::Run);
}

fn configs() -> Vec<RunConfig> {
    app_signal().with(|state| {
        state
            .cfg
            .extra
            .get("run_configs")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| serde_json::from_value(item.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn selected() -> Option<RunConfig> {
    let id = global::<Selected>().with(|selected| selected.0.clone());
    let options = configs();
    options
        .iter()
        .find(|config| config.id == id)
        .cloned()
        .or_else(|| options.into_iter().next())
}

fn choose() {
    state::show_popup_menu(
        configs()
            .into_iter()
            .map(|config| {
                MenuItem::new(config.name).on_select(move || {
                    global::<Selected>().set(Selected(config.id.clone()));
                })
            })
            .collect(),
    );
}

pub(super) fn edit(new: bool) {
    let config = if new {
        RunConfig::default()
    } else {
        selected().unwrap_or_default()
    };
    let env = config
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    global::<Draft>().set(Draft {
        config: Some(config),
        env,
    });
    app_signal().update(state::enter_modal_focus);
}

pub(crate) fn close_active() -> bool {
    let open = global::<Draft>().with_untracked(|draft| draft.config.is_some());
    if open {
        global::<Draft>().set(Draft::default());
    }
    open
}

fn store_configs(configs: Vec<RunConfig>) -> Result<()> {
    let value = serde_json::to_value(configs)?;
    app_signal().update(|state| {
        state.cfg.extra.insert("run_configs".into(), value);
    });
    state::save_config_quiet();
    Ok(())
}

fn save() -> Result<()> {
    let (mut config, env) = global::<Draft>()
        .with_untracked(|draft| (draft.config.clone().unwrap_or_default(), draft.env.clone()));
    ensure!(!config.name.trim().is_empty(), "请输入运行配置名称");
    if config.kind == Kind::Debug {
        ensure!(
            (config.network.trim() == "tcp" && !config.address.trim().is_empty())
                || !config.adapter.trim().is_empty(),
            "请填写调试适配器或 TCP 地址"
        );
        if !config.launch.trim().is_empty() {
            ensure!(
                serde_json::from_str::<serde_json::Value>(&config.launch)?.is_object(),
                "Launch 需要 JSON 对象"
            );
        }
    } else {
        ensure!(!config.command.trim().is_empty(), "请输入运行命令");
    }
    config.env = workbench_run::env_pairs(&env)?;
    let mut configs = configs();
    if config.id.is_empty() {
        config.id = format!(
            "run-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        configs.push(config.clone());
    } else if let Some(previous) = configs.iter_mut().find(|previous| previous.id == config.id) {
        *previous = config.clone();
    }
    store_configs(configs)?;
    global::<Selected>().set(Selected(config.id));
    state::close_active_modal();
    Ok(())
}

fn delete() {
    let Some(id) = global::<Draft>()
        .with_untracked(|draft| draft.config.as_ref().map(|config| config.id.clone()))
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    let mut configs = configs();
    configs.retain(|config| config.id != id);
    if let Err(error) = store_configs(configs) {
        state::show_toast("warn", error.to_string());
    } else {
        state::close_active_modal();
    }
}

fn run() {
    let Some(config) = selected() else {
        edit(true);
        return;
    };
    if config.kind == Kind::Debug {
        super::debug_panel::start(config);
        return;
    }
    match workbench_run::run_command(&config, &workbench::workspace()) {
        Ok(command) => workbench::launch(config.name, command),
        Err(error) => state::show_toast("warn", error.to_string()),
    }
}

pub(super) fn body() -> impl IntoElement {
    let p = current_palette();
    let debug_visible = computed(super::debug_panel::visible);
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .gap(px(5.0))
        .child(
            view()
                .flex_row()
                .gap(px(5.0))
                .h(px(26.0))
                .flex_none()
                .child(
                    view()
                        .test_id("run-select-config")
                        .h(px(24.0))
                        .px(px(8.0))
                        .bg(p.surface2)
                        .items_center()
                        .cursor(Cursor::Pointer)
                        .on_press(choose)
                        .child(
                            text(|| {
                                selected()
                                    .map(|config| config.name)
                                    .unwrap_or_else(|| "选择运行配置".into())
                            })
                            .text_color(p.text_primary),
                        ),
                )
                .child(button("新建配置", "run-new-config", || edit(true)))
                .child(button("编辑配置", "run-edit-config", || edit(false)))
                .child(button("运行", "run-start", run))
                .child(button("停止", "run-stop", || {
                    workbench::stop();
                    super::debug_panel::stop();
                })),
        )
        .child(dynamic(move || {
            if debug_visible.get() {
                super::debug_panel::body().into_element()
            } else {
                workbench::output().into_element()
            }
        }))
}

fn field(
    label: &'static str,
    id: &'static str,
    get: fn(&mut RunConfig) -> &mut String,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .flex_none()
        .child(
            text(label)
                .w(px(88.0))
                .text_color(p.text_muted)
                .text_size(px(11.0)),
        )
        .child(
            input()
                .test_id(id)
                .placeholder(label)
                .h(px(26.0))
                .flex_1()
                .px(px(5.0))
                .bg(p.surface2)
                .text_color(p.text_primary)
                .value(move || {
                    global::<Draft>().with(|draft| {
                        draft
                            .config
                            .clone()
                            .map(|mut c| get(&mut c).clone())
                            .unwrap_or_default()
                    })
                })
                .on_change(move |value: &str| {
                    global::<Draft>().update(|draft| {
                        if let Some(config) = &mut draft.config {
                            *get(config) = value.to_owned();
                        }
                    })
                }),
        )
}

pub(super) fn layer() -> impl IntoElement {
    let visible = computed(|| global::<Draft>().with(|draft| draft.config.is_some()));
    let debug_form = computed(|| global::<Draft>().with(|draft| draft.config.as_ref().is_some_and(|c| c.kind == Kind::Debug)));
    dynamic(move || {
        if !visible.get() {
            return view().into_element();
        }
        let p = current_palette();
        view()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000075))
            .items_center()
            .justify_center()
            .p(px(12.0))
            .child(
                view()
                    .test_id("run-config-modal")
                    .w_full()
                    .max_w(px(660.0))
                    .max_h(px(660.0))
                    .h_full()
                    .rounded(px(8.0))
                    .bg(p.surface)
                    .flex_col()
                    .p(px(10.0))
                    .gap(px(6.0))
                    .child(text("运行配置").text_color(p.text_primary))
                    .child(
                        view()
                            .flex_row()
                            .gap(px(6.0))
                            .children(
                                [
                                    ("Run", "run-kind-run", Kind::Run),
                                    ("Test", "run-kind-test", Kind::Test),
                                    ("Debug", "run-kind-debug", Kind::Debug),
                                ]
                                .map(|(label, id, kind)| {
                                    button(label, id, move || {
                                        global::<Draft>().update(|draft| {
                                            if let Some(config) = draft.config.as_mut() {
                                                config.kind = kind;
                                            }
                                        })
                                    })
                                }),
                            )
                            .child(
                                text(|| {
                                    global::<Draft>().with(|d| {
                                        d.config
                                            .as_ref()
                                            .map(|c| format!("{:?}", c.kind))
                                            .unwrap_or_default()
                                    })
                                })
                                .text_color(p.accent),
                            ),
                    )
                    .child(
                        scroll().flex_1().child(
                            view()
                                .flex_col()
                                .gap(px(5.0))
                                .p(px(2.0))
                                .child(field("名称", "run-field-name", |c| &mut c.name))
                                .child(field("工作目录", "run-field-cwd", |c| &mut c.cwd))
                                .child(dynamic(move || {
                                    let form = view().flex_col().gap(px(5.0));
                                    if debug_form.get() {
                                        form.child(field("适配器", "run-field-adapter", |c| {
                                            &mut c.adapter
                                        }))
                                        .child(field(
                                            "适配器参数",
                                            "run-field-adapter-args",
                                            |c| &mut c.adapter_args,
                                        ))
                                        .child(field("stdio 或 tcp", "run-field-network", |c| {
                                            &mut c.network
                                        }))
                                        .child(field("TCP 地址", "run-field-address", |c| {
                                            &mut c.address
                                        }))
                                        .child(field("程序", "run-field-program", |c| {
                                            &mut c.program
                                        }))
                                        .child(field(
                                            "程序参数",
                                            "run-field-program-args",
                                            |c| &mut c.program_args,
                                        ))
                                        .child(field("Launch JSON", "run-field-launch", |c| {
                                            &mut c.launch
                                        }))
                                        .into_element()
                                    } else {
                                        form.child(field("命令", "run-field-command", |c| {
                                            &mut c.command
                                        }))
                                        .child(field("参数", "run-field-args", |c| &mut c.args))
                                        .into_element()
                                    }
                                }))
                                .child(
                                    text("环境变量，每行一个 K=V")
                                        .text_color(p.text_muted)
                                        .text_size(px(11.0)),
                                )
                                .child(
                                    editor()
                                        .test_id("run-field-env")
                                        .line_numbers(false)
                                        .h(px(120.0))
                                        .value(|| {
                                            Some(
                                                global::<Draft>()
                                                    .with(|draft| draft.env.clone())
                                                    .into(),
                                            )
                                        })
                                        .on_change(|value: &SharedString| {
                                            global::<Draft>()
                                                .update(|draft| draft.env = value.to_string())
                                        }),
                                ),
                        ),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap(px(6.0))
                            .child(button("保存", "run-config-save", || {
                                if let Err(error) = save() {
                                    state::show_toast("warn", error.to_string());
                                }
                            }))
                            .child(button(
                                "取消",
                                "run-config-close",
                                state::close_active_modal,
                            ))
                            .child(when(
                                || {
                                    global::<Draft>().with(|draft| {
                                        draft.config.as_ref().is_some_and(|c| !c.id.is_empty())
                                    })
                                },
                                || button("删除", "run-config-delete", delete),
                            )),
                    ),
            )
            .into_element()
    })
}
