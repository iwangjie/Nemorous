use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use nsh_base::{editor, input};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;
use serde_json::Value;

use super::workbench::button;
use crate::state::{self, app_signal, current_palette};
use crate::workbench_github::{self, Action};

#[derive(Clone, Default)]
struct Store {
    open: bool,
    path: PathBuf,
    request: u64,
    cancel: Option<Arc<AtomicBool>>,
    filter: String,
    repository: String,
    prs: Vec<Value>,
    detail: Option<Value>,
    result_url: String,
    error: String,
    form: Option<Form>,
}

#[derive(Clone, Default)]
struct Form {
    number: Option<u64>,
    title: String,
    body: String,
    base: String,
    draft: bool,
}

fn store() -> Signal<Store> {
    global::<Store>()
}

pub(super) fn open() {
    let repo =
        app_signal().with_untracked(|s| s.repos.first().map(|repo| PathBuf::from(&repo.path)));
    let Some(repo) = repo else {
        state::show_toast("warn", "当前工作区没有 Git 仓库");
        return;
    };
    open_repo(repo);
}

fn open_repo(path: PathBuf) {
    close_active();
    let request = store().with_untracked(|view| view.request + 1);
    store().set(Store {
        open: true,
        path,
        request,
        filter: "open".into(),
        ..Store::default()
    });
    app_signal().update(state::enter_modal_focus);
    refresh();
}

pub(crate) fn close_active() -> bool {
    let open = store().with_untracked(|view| view.open);
    if open {
        cancel();
        let request = store().with_untracked(|view| view.request + 1);
        store().set(Store {
            request,
            ..Store::default()
        });
    }
    open
}

fn cancel() {
    store().update(|view| {
        if let Some(cancel) = view.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        view.request += 1;
    });
}

fn refresh() {
    execute(Action::List(
        store().with_untracked(|view| view.filter.clone()),
    ));
}

fn execute(action: Action) {
    enum Update {
        Rows,
        Detail,
        Comment(u64),
        Created,
    }
    let update = match &action {
        Action::List(_) => Update::Rows,
        Action::Detail(_) => Update::Detail,
        Action::Comment { number, .. } => Update::Comment(*number),
        Action::Create { .. } => Update::Created,
    };
    cancel();
    let (path, request) = store().with_untracked(|view| (view.path.clone(), view.request));
    let cancel = Arc::new(AtomicBool::new(false));
    store().update(|view| {
        view.cancel = Some(Arc::clone(&cancel));
        view.error.clear();
    });
    state::spawn_from_root(async move {
        let result = blocking(move || workbench_github::run(&path, action, &cancel)).await;
        if !store().with_untracked(|view| view.open && view.request == request) {
            return;
        }
        let mut refresh_detail = None;
        store().update(|view| {
            view.cancel = None;
            match result {
                Ok(Ok((repository, value))) => {
                    view.repository = repository;
                    match update {
                        Update::Rows => {
                            view.prs = value.as_array().cloned().unwrap_or_default();
                            view.detail = None;
                        }
                        Update::Detail => view.detail = Some(value),
                        Update::Comment(number) => {
                            view.form = None;
                            refresh_detail = Some(number);
                        }
                        Update::Created => {
                            view.form = None;
                            view.detail = None;
                            view.result_url = value.as_str().unwrap_or_default().to_owned();
                        }
                    }
                }
                Ok(Err(error)) => view.error = error.to_string(),
                Err(error) => view.error = error.to_string(),
            }
        });
        if let Some(number) = refresh_detail {
            execute(Action::Detail(number));
        }
    });
}

fn repos() {
    state::show_popup_menu(
        app_signal()
            .with_untracked(|s| s.repos.clone())
            .into_iter()
            .map(|repo| {
                let path = PathBuf::from(repo.path);
                MenuItem::new(repo.name).on_select(move || open_repo(path.clone()))
            })
            .collect(),
    );
}

fn filters() {
    state::show_popup_menu(
        [
            ("打开", "open"),
            ("已关闭", "closed"),
            ("已合并", "merged"),
            ("全部", "all"),
        ]
        .into_iter()
        .map(|(label, filter)| {
            MenuItem::new(label).on_select(move || {
                store().update(|view| view.filter = filter.into());
                refresh();
            })
        })
        .collect(),
    );
}

fn form(number: Option<u64>) {
    let base = app_signal().with_untracked(|state| state.cfg.default_base_branch.clone());
    store().update(|view| {
        view.form = Some(Form {
            number,
            base,
            ..Form::default()
        })
    });
}

fn submit() {
    let Some(form) =
        store().with_untracked(|view| view.cancel.is_none().then(|| view.form.clone()).flatten())
    else {
        return;
    };
    if let Some(number) = form.number {
        execute(Action::Comment {
            number,
            body: form.body,
        });
    } else {
        execute(Action::Create {
            title: form.title,
            body: form.body,
            base: form.base,
            draft: form.draft,
        });
    }
}

fn text_value(view: &Store) -> String {
    let Some(detail) = &view.detail else {
        return view.result_url.clone();
    };
    let val = |key: &str| detail[key].as_str().unwrap_or_default();
    let mut lines = vec![
        format!("#{} {}", detail["number"], val("title")),
        format!(
            "{} → {} · {}",
            val("headRefName"),
            val("baseRefName"),
            val("state")
        ),
        val("body").to_owned(),
    ];
    for (key, heading) in [
        ("statusCheckRollup", "检查结果"),
        ("reviews", "审阅"),
        ("comments", "评论"),
    ] {
        if let Some(entries) = detail[key].as_array() {
            lines.push(format!("\n{heading}"));
            lines.extend(entries.iter().map(|item| {
                let val = |field: &str| item[field].as_str().unwrap_or_default();
                if key == "statusCheckRollup" {
                    format!(
                        "{} · {}",
                        val("name").if_empty(val("context")),
                        val("conclusion")
                            .if_empty(val("state"))
                            .if_empty(val("status"))
                    )
                } else {
                    format!(
                        "{}\n{}",
                        item["author"]["login"].as_str().unwrap_or_default(),
                        val("body")
                    )
                }
            }));
        }
    }
    lines.join("\n")
}

trait Fallback {
    fn if_empty(self, fallback: Self) -> Self;
}
impl Fallback for &str {
    fn if_empty(self, fallback: Self) -> Self {
        if self.is_empty() { fallback } else { self }
    }
}

fn form_view(number: Option<u64>, draft: bool) -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .gap(px(4.0))
        .child(number.is_none().then(|| {
            view()
                .flex_row()
                .gap(px(5.0))
                .child(
                    input()
                        .test_id("gh-pr-title")
                        .h(px(27.0))
                        .flex_1()
                        .placeholder("PR 标题")
                        .value(|| {
                            store().with(|view| {
                                view.form
                                    .as_ref()
                                    .map(|f| f.title.clone())
                                    .unwrap_or_default()
                            })
                        })
                        .on_change(|v: &str| {
                            store().update(|view| {
                                if let Some(f) = &mut view.form {
                                    f.title = v.into();
                                }
                            })
                        }),
                )
                .child(
                    input()
                        .test_id("gh-pr-base")
                        .h(px(27.0))
                        .w(px(165.0))
                        .placeholder("目标分支")
                        .value(|| {
                            store().with(|view| {
                                view.form
                                    .as_ref()
                                    .map(|f| f.base.clone())
                                    .unwrap_or_default()
                            })
                        })
                        .on_change(|v: &str| {
                            store().update(|view| {
                                if let Some(f) = &mut view.form {
                                    f.base = v.into();
                                }
                            })
                        }),
                )
                .child(button(
                    if draft { "草稿 ✓" } else { "草稿" },
                    "gh-pr-draft",
                    || {
                        store().update(|view| {
                            if let Some(f) = &mut view.form {
                                f.draft = !f.draft;
                            }
                        })
                    },
                ))
        }))
        .child(
            editor()
                .test_id("gh-pr-body")
                .line_numbers(false)
                .soft_wrap(true)
                .flex_1()
                .placeholder(Some("正文".into()))
                .value(|| store().with(|view| view.form.as_ref().map(|f| f.body.clone().into())))
                .on_change(|v: &SharedString| {
                    store().update(|view| {
                        if let Some(form) = &mut view.form {
                            form.body = v.to_string();
                        }
                    })
                }),
        )
        .child(
            view()
                .flex_row()
                .gap(px(6.0))
                .child(button(
                    if number.is_some() {
                        "发表评论"
                    } else {
                        "创建 PR"
                    },
                    "gh-submit",
                    submit,
                ))
                .child(button("取消编辑", "gh-close-form", || {
                    store().update(|view| view.form = None)
                }))
                .child(
                    text(|| store().with(|view| view.error.clone()))
                        .text_size(px(11.0))
                        .text_color(p.danger),
                ),
        )
}

pub(super) fn layer() -> impl IntoElement {
    let opened = computed(|| store().with(|view| view.open));
    let display = computed(|| {
        store().with(|view| {
            (
                view.detail
                    .as_ref()
                    .and_then(|value| value["number"].as_u64()),
                view.detail
                    .as_ref()
                    .and_then(|value| value["url"].as_str())
                    .unwrap_or(&view.result_url)
                    .to_owned(),
                view.form.as_ref().map(|form| (form.number, form.draft)),
            )
        })
    });
    dynamic(move || {
        if !opened.get() {
            return view().into_element();
        }
        let p = current_palette();
        view()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000073))
            .items_center()
            .justify_center()
            .p(px(12.0))
            .child(
                view()
                    .test_id("github-panel")
                    .w_full()
                    .h_full()
                    .max_w(px(1040.0))
                    .max_h(px(650.0))
                    .flex_col()
                    .p(px(9.0))
                    .gap(px(6.0))
                    .rounded(px(8.0))
                    .bg(p.surface)
                    .child(
                        view()
                            .flex_row()
                            .gap(px(6.0))
                            .items_center()
                            .child(text("GitHub PR / CI").flex_1().text_color(p.text_primary))
                            .child(button("选择仓库", "gh-select-repo", repos))
                            .child(button("状态", "gh-filter", filters))
                            .child(button("刷新", "gh-refresh", refresh))
                            .child(button("取消请求", "gh-cancel", cancel))
                            .child(button("关闭", "gh-close", state::close_active_modal)),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap(px(6.0))
                            .items_center()
                            .child(
                                text(|| store().with(|view| view.repository.clone()))
                                    .text_color(p.text_muted),
                            )
                            .child(
                                text(|| {
                                    store().with(|view| {
                                        if view.cancel.is_some() {
                                            "正在加载…".into()
                                        } else {
                                            view.error.clone()
                                        }
                                    })
                                })
                                .text_color(p.text_muted),
                            )
                            .child(button("新建 PR", "gh-new", || form(None))),
                    )
                    .child(
                        view()
                            .flex_row()
                            .flex_1()
                            .min_h(px(0.0))
                            .gap(px(6.0))
                            .child(scroll().w(px(285.0)).flex_none().child(dynamic(move || {
                                let prs = store().with(|view| view.prs.clone());
                                view()
                                    .flex_col()
                                    .children(prs.into_iter().filter_map(|pr| {
                                        let number = pr["number"].as_u64()?;
                                        Some(
                                            view()
                                                .test_id(&format!("gh-pr-{number}"))
                                                .px(px(4.0))
                                                .h(px(25.0))
                                                .items_center()
                                                .cursor(Cursor::Pointer)
                                                .on_press(move || execute(Action::Detail(number)))
                                                .child(
                                                    text(format!(
                                                        "#{number} {}",
                                                        pr["title"].as_str().unwrap_or_default()
                                                    ))
                                                    .text_color(p.text_primary)
                                                    .text_size(px(11.0))
                                                    .number_of_lines(1u32),
                                                ),
                                        )
                                    }))
                                    .into_element()
                            })))
                            .child(dynamic(move || {
                                let (number, url, editing) = display.get();
                                if let Some((number, draft)) = editing {
                                    return form_view(number, draft).into_element();
                                }
                                view()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        view()
                                            .flex_row()
                                            .gap(px(5.0))
                                            .child(number.map(|num| {
                                                button("评论", "gh-comment", move || {
                                                    form(Some(num))
                                                })
                                            }))
                                            .child((!url.is_empty()).then(|| {
                                                button(
                                                    "在浏览器打开",
                                                    "gh-open-url",
                                                    move || {
                                                        let valid =
                                                            store().with_untracked(|view| {
                                                                workbench_github::safe_url(
                                                                    &view.repository,
                                                                    &url,
                                                                )
                                                            });
                                                        if valid {
                                                            let _ = open_url(url.clone());
                                                        }
                                                    },
                                                )
                                            })),
                                    )
                                    .child(
                                        editor()
                                            .test_id("gh-pr-detail")
                                            .read_only(true)
                                            .soft_wrap(true)
                                            .line_numbers(false)
                                            .flex_1()
                                            .value(|| Some(store().with(text_value).into())),
                                    )
                                    .into_element()
                            })),
                    ),
            )
            .into_element()
    })
}
