use std::path::PathBuf;

use nsh_base::input;
use nsh_core::prelude::*;

use crate::git_credentials::{self, SshKey};
use crate::state::{self, app_signal, current_palette};

#[derive(Default, Clone)]
struct Credentials {
    repo: Option<PathBuf>,
    host: String,
    user: String,
    ssh_host: String,
    keys: Vec<SshKey>,
    status: String,
}

#[derive(Default, Clone)]
struct Secret(String);

fn state() -> Signal<Credentials> {
    global::<Credentials>()
}

fn ssh_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(crate::config::config_dir)
        .join(".ssh")
}

pub(super) fn open(repo: PathBuf) {
    global::<Secret>().set(Secret::default());
    state().set(Credentials {
        repo: Some(repo),
        ..Credentials::default()
    });
    app_signal().update(state::enter_modal_focus);
    refresh_keys();
}

pub(crate) fn close_active() -> bool {
    let open = state().with_untracked(|s| s.repo.is_some());
    if open {
        state().set(Credentials::default());
        global::<Secret>().set(Secret::default());
    }
    open
}

fn refresh_keys() {
    state::spawn_from_root(async {
        let dir = ssh_dir();
        let keys = blocking(move || git_credentials::ssh_keys(&dir)).await;
        if !state().with_untracked(|s| s.repo.is_some()) {
            return;
        }
        state().update(|s| match keys {
            Ok(Ok(keys)) => s.keys = keys,
            Ok(Err(err)) => s.status = err.to_string(),
            Err(err) => s.status = err.to_string(),
        });
    });
}

fn https(save: bool) {
    let Some((repo, host, user)) =
        state().with_untracked(|s| Some((s.repo.clone()?, s.host.clone(), s.user.clone())))
    else {
        return;
    };
    let token = save.then(|| global::<Secret>().with_untracked(|s| s.0.clone()));
    global::<Secret>().set(Secret::default());
    state::spawn_from_root(async move {
        let result =
            blocking(move || git_credentials::https(&repo, &host, &user, token.as_deref())).await;
        state().update(|s| {
            s.status = match result {
                Ok(Ok(())) if save => "凭据已交给 Git 助手保存".to_owned(),
                Ok(Ok(())) => "已删除匹配的 HTTPS 凭据".to_owned(),
                Ok(Err(err)) => err.to_string(),
                Err(_) => "凭据操作已中断".to_owned(),
            };
        });
    });
}

fn ssh_status(test: bool) {
    let host = state().with_untracked(|s| s.ssh_host.clone());
    state::spawn_from_root(async move {
        let result = blocking(move || {
            if test {
                git_credentials::test_ssh(&host)
            } else {
                git_credentials::agent_keys()
            }
        })
        .await;
        state().update(|s| {
            s.status = match result {
                Ok(Ok(text)) => text,
                Ok(Err(err)) => err.to_string(),
                Err(err) => err.to_string(),
            };
        });
    });
}

fn create() {
    let repo = state().with_untracked(|s| s.repo.clone());
    state::close_active_modal();
    super::git_prompt::open(
        "新建 Ed25519 密钥 · 文件名（无口令）",
        String::new(),
        move |name| {
            let name = name.to_owned();
            let repo = repo.clone();
            state::spawn_from_root(async move {
                let dir = ssh_dir();
                let result =
                    blocking(move || git_credentials::create_key(&dir, &name, "Nemorous")).await;
                if let Some(repo) = repo {
                    open(repo);
                }
                match result {
                    Ok(Ok(_)) => state::show_toast("ok", "已创建 Ed25519 密钥"),
                    Ok(Err(err)) => state::show_toast("warn", err.to_string()),
                    Err(err) => state::show_toast("err", err.to_string()),
                }
            });
            Ok(())
        },
    );
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
        .h(px(27.0))
        .px(px(8.0))
        .rounded(px(4.0))
        .items_center()
        .justify_center()
        .bg(p.surface2)
        .cursor(Cursor::Pointer)
        .on_press(callback)
        .child(text(label).text_color(p.text_primary).text_size(px(11.0)))
}

pub(super) fn layer() -> impl IntoElement {
    let open = computed(|| state().with(|s| s.repo.is_some()));
    when(
        move || open.get(),
        || {
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
                        .test_id("git-credentials")
                        .w_full()
                        .max_w(px(580.0))
                        .h_full()
                        .max_h(px(530.0))
                        .bg(p.surface)
                        .border_1()
                        .border_color(p.border_strong)
                        .rounded(px(8.0))
                        .flex_col()
                        .px(px(11.0))
                        .py(px(10.0))
                        .gap(px(7.0))
                        .child(
                            view()
                                .flex_row()
                                .items_center()
                                .child(
                                    text("Git 凭据与 SSH")
                                        .flex_1()
                                        .text_color(p.text_primary)
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(action(
                                    "关闭",
                                    "credentials-close",
                                    state::close_active_modal,
                                )),
                        )
                        .child(text("HTTPS").text_color(p.text_primary).text_size(px(12.0)))
                        .child(
                            view()
                                .flex_row()
                                .gap(px(5.0))
                                .child(
                                    input()
                                        .test_id("credential-host")
                                        .flex_1()
                                        .h(px(28.0))
                                        .px(px(6.0))
                                        .bg(p.surface2)
                                        .placeholder("主机")
                                        .text_color(p.text_primary)
                                        .value(|| state().with(|s| s.host.clone()))
                                        .on_change(|value: &str| {
                                            state().update(|s| s.host = value.to_owned())
                                        }),
                                )
                                .child(
                                    input()
                                        .test_id("credential-user")
                                        .flex_1()
                                        .h(px(28.0))
                                        .px(px(6.0))
                                        .bg(p.surface2)
                                        .placeholder("用户名")
                                        .text_color(p.text_primary)
                                        .value(|| state().with(|s| s.user.clone()))
                                        .on_change(|value: &str| {
                                            state().update(|s| s.user = value.to_owned())
                                        }),
                                )
                                .child(
                                    input()
                                        .test_id("credential-secret")
                                        .flex_1()
                                        .h(px(28.0))
                                        .px(px(6.0))
                                        .bg(p.surface2)
                                        .placeholder("令牌")
                                        .masked(true)
                                        .text_color(p.text_primary)
                                        .value(|| global::<Secret>().with(|s| s.0.clone()))
                                        .on_change(|value: &str| {
                                            global::<Secret>().set(Secret(value.to_owned()))
                                        }),
                                ),
                        )
                        .child(
                            view()
                                .flex_row()
                                .gap(px(5.0))
                                .child(action("保存 HTTPS 凭据", "credential-save", || {
                                    https(true)
                                }))
                                .child(action("删除 HTTPS 凭据", "credential-erase", || {
                                    https(false)
                                })),
                        )
                        .child(text("SSH").text_color(p.text_primary).text_size(px(12.0)))
                        .child(
                            view()
                                .flex_row()
                                .gap(px(5.0))
                                .child(
                                    input()
                                        .test_id("ssh-host")
                                        .flex_1()
                                        .h(px(28.0))
                                        .px(px(6.0))
                                        .bg(p.surface2)
                                        .placeholder("user@host")
                                        .text_color(p.text_primary)
                                        .value(|| state().with(|s| s.ssh_host.clone()))
                                        .on_change(|value: &str| {
                                            state().update(|s| s.ssh_host = value.to_owned())
                                        }),
                                )
                                .child(action("测试连接", "ssh-test", || ssh_status(true)))
                                .child(action("Agent 密钥", "ssh-agent", || ssh_status(false)))
                                .child(action("新建密钥…", "ssh-create", create)),
                        )
                        .child(scroll().flex_1().child(dynamic(move || {
                            let keys = state().with(|s| s.keys.clone());
                            view()
                                .flex_col()
                                .gap(px(5.0))
                                .children(keys.into_iter().map(|key| {
                                    let name = key
                                        .path
                                        .file_name()
                                        .map(|part| part.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    view()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(5.0))
                                        .child(
                                            view()
                                                .flex_1()
                                                .flex_col()
                                                .child(text(name).text_color(p.text_primary))
                                                .child(
                                                    text(key.fingerprint)
                                                        .text_size(px(10.0))
                                                        .text_color(p.text_muted),
                                                ),
                                        )
                                        .child(action(
                                            "复制公钥",
                                            "ssh-copy-public",
                                            move || set_clipboard(key.public_key.clone()),
                                        ))
                                }))
                        })))
                        .child(dynamic(move || {
                            text(state().with(|s| s.status.clone()))
                                .text_size(px(11.0))
                                .text_color(p.text_secondary)
                        })),
                )
        },
    )
}
