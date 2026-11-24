use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use nsh_base::{editor, input};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;
use serde_json::Value;

use super::workbench::button;
use crate::git::RepoInfo;
use crate::state::{self, app_signal, current_palette};
use crate::workbench_github::{self, Action};

#[derive(Clone, Default)]
struct Store {
    open: bool,
    path: PathBuf,
    repo_label: String,
    request: u64,
    pending: Option<PendingRequest>,
    filter: String,
    repository: String,
    prs: Vec<Value>,
    detail: Option<Value>,
    result_url: String,
    notice: Option<&'static str>,
    error: String,
    form: Option<Form>,
}

#[derive(Clone)]
enum PendingRequest {
    Read(Arc<AtomicBool>),
    Submission,
}

impl Store {
    fn submitting(&self) -> bool {
        matches!(self.pending, Some(PendingRequest::Submission))
    }

    fn cancel_read(&mut self) {
        if let Some(PendingRequest::Read(cancel)) = &self.pending {
            cancel.store(true, Ordering::Release);
            self.pending = None;
            self.request = self.request.wrapping_add(1);
        }
    }
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
    open_for_repository(None);
}

pub(super) fn open_for_repository(id: Option<&str>) {
    let repo = app_signal().with_untracked(|s| match id {
        Some(id) => s.repos.iter().find(|repo| repo.id == id).cloned(),
        None => s.repos.first().cloned(),
    });
    let Some(repo) = repo else {
        let message = if id.is_some() {
            "当前仓库已移出工作区"
        } else {
            "当前工作区没有 Git 仓库"
        };
        state::show_toast("warn", message);
        return;
    };
    open_repo(&repo);
}

fn open_repo(repo: &RepoInfo) {
    if submitting() {
        return;
    }
    cancel();
    let request = store().with_untracked(|view| view.request.wrapping_add(1));
    store().set(Store {
        open: true,
        path: PathBuf::from(&repo.path),
        repo_label: repo.label().to_owned(),
        request,
        filter: "open".into(),
        ..Store::default()
    });
    app_signal().update(state::enter_modal_focus);
    refresh();
}

pub(crate) fn close_active() -> bool {
    let open = store().with_untracked(|view| view.open && !view.submitting());
    if open {
        cancel();
        let request = store().with_untracked(|view| view.request.wrapping_add(1));
        store().set(Store {
            request,
            ..Store::default()
        });
    }
    open
}

pub(crate) fn submitting() -> bool {
    store().with(Store::submitting)
}

fn cancel() {
    store().update(Store::cancel_read);
}

fn no_pending_read() -> bool {
    store().with(|view| !matches!(view.pending, Some(PendingRequest::Read(_))))
}

fn request_pending() -> bool {
    store().with(|view| view.pending.is_some())
}

fn refresh() {
    execute(Action::List(
        store().with_untracked(|view| view.filter.clone()),
    ));
}

fn execute(action: Action) {
    let current = store().with_untracked(|view| {
        (view.open && !view.submitting()).then(|| {
            (view.path.clone(), view.repository.clone(), view.request.wrapping_add(1))
        })
    });
    let Some((path, repository, request)) = current else {
        return;
    };
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
    let cancel = Arc::new(AtomicBool::new(false));
    let pending = if action.is_submission() {
        PendingRequest::Submission
    } else {
        PendingRequest::Read(Arc::clone(&cancel))
    };
    store().update(|view| {
        view.cancel_read();
        view.request = request;
        view.pending = Some(pending);
        view.error.clear();
    });
    state::spawn_from_root(async move {
        let result =
            blocking(move || workbench_github::run(&path, &repository, action, &cancel)).await;
        if !store().with_untracked(|view| view.open && view.request == request) {
            return;
        }
        let mut follow_up = None;
        store().update(|view| {
            view.pending = None;
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
                            view.notice = Some("评论已发送");
                            follow_up = Some(Action::Detail(number));
                        }
                        Update::Created => {
                            view.form = None;
                            view.detail = None;
                            view.notice = Some("PR 已创建");
                            view.result_url = value.as_str().unwrap_or_default().to_owned();
                            follow_up = Some(Action::List(view.filter.clone()));
                        }
                    }
                }
                Ok(Err(error)) => view.error = error.to_string(),
                Err(error) => view.error = error.to_string(),
            }
        });
        if let Some(action) = follow_up {
            execute(action);
        }
    });
}

fn repos() {
    let current = store().with_untracked(|view| view.path.clone());
    state::show_popup_menu(
        app_signal()
            .with_untracked(|s| s.repos.clone())
            .into_iter()
            .map(|repo| {
                MenuItem::new(repo.label().to_owned())
                    .checked(current == Path::new(&repo.path))
                    .on_select(move || open_repo(&repo))
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
                if submitting() {
                    return;
                }
                store().update(|view| view.filter = filter.into());
                refresh();
            })
        })
        .collect(),
    );
}

fn form(number: Option<u64>) {
    if submitting() {
        return;
    }
    let base = app_signal().with_untracked(|state| state.cfg.default_base_branch.clone());
    store().update(|view| {
        view.notice = None;
        view.error.clear();
        view.form = Some(Form {
            number,
            base,
            ..Form::default()
        })
    });
}

fn edit_form(edit: impl FnOnce(&mut Form)) {
    store().update(|view| {
        if !view.submitting() && let Some(form) = &mut view.form {
            edit(form);
        }
    });
}

fn submit() {
    let Some(form) = store().with_untracked(|view| {
        view.pending.is_none().then(|| view.form.clone()).flatten()
    })
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

fn status_text(view: &Store) -> String {
    let status = match &view.pending {
        Some(PendingRequest::Read(_)) => "正在加载…",
        Some(PendingRequest::Submission) => "正在提交…",
        None => &view.error,
    };
    match view.notice {
        Some(notice) if status.is_empty() => notice.into(),
        Some(notice) => format!("{notice} · {status}"),
        None => status.into(),
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
    ];
    if let Some(checks) = detail["statusCheckRollup"].as_array().filter(|items| !items.is_empty()) {
        lines.push("\n检查结果".into());
        lines.extend(checks.iter().map(|check| {
            let val = |field: &str| check[field].as_str().unwrap_or_default();
            format!(
                "{} · {}",
                val("name").if_empty(val("context")),
                val("conclusion").if_empty(val("state")).if_empty(val("status")),
            )
        }));
    }
    let body = val("body");
    if !body.is_empty() {
        lines.push(format!("\n{body}"));
    }
    for (key, heading) in [("reviews", "审阅"), ("comments", "评论")] {
        if let Some(entries) = detail[key].as_array().filter(|items| !items.is_empty()) {
            lines.push(format!("\n{heading}"));
            lines.extend(entries.iter().map(|item| {
                format!(
                    "{}\n{}",
                    item["author"]["login"].as_str().unwrap_or_default(),
                    item["body"].as_str().unwrap_or_default(),
                )
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
                        .disabled(submitting)
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
                        .on_change(|v: &str| edit_form(|form| form.title = v.into())),
                )
                .child(
                    input()
                        .test_id("gh-pr-base")
                        .disabled(submitting)
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
                        .on_change(|v: &str| edit_form(|form| form.base = v.into())),
                )
                .child(button(
                    if draft { "草稿 ✓" } else { "草稿" },
                    "gh-pr-draft",
                    || edit_form(|form| form.draft = !form.draft),
                ).disabled(submitting))
        }))
        .child(
            editor()
                .test_id("gh-pr-body")
                .read_only(submitting)
                .line_numbers(false)
                .soft_wrap(true)
                .flex_1()
                .placeholder(Some("正文".into()))
                .value(|| store().with(|view| view.form.as_ref().map(|f| f.body.clone().into())))
                .on_change(|v: &SharedString| edit_form(|form| form.body = v.to_string())),
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
                ).disabled(request_pending))
                .child(button("取消编辑", "gh-close-form", || {
                    store().update(|view| {
                        if !view.submitting() {
                            view.form = None;
                            view.error.clear();
                        }
                    })
                }).disabled(submitting)),
        )
}

pub(super) fn layer() -> impl IntoElement {
    let opened = computed(|| store().with(|view| view.open));
    let repository_loaded = computed(|| store().with(|view| !view.repository.is_empty()));
    let rows = computed(|| store().with(|view| view.prs.clone()));
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
                            .child(button("选择仓库", "gh-select-repo", repos).disabled(submitting))
                            .child(button("状态", "gh-filter", filters).disabled(submitting))
                            .child(button("刷新", "gh-refresh", refresh).disabled(submitting))
                            .child(button("取消请求", "gh-cancel", cancel).disabled(no_pending_read))
                            .child(button("关闭", "gh-close", state::close_active_modal).disabled(submitting)),
                    )
                    .child(
                        view()
                            .flex_row()
                            .gap(px(6.0))
                            .items_center()
                            .child(
                                text(|| store().with(|view| {
                                    view.repository.as_str().if_empty(&view.repo_label).to_owned()
                                }))
                                    .text_color(p.text_muted),
                            )
                            .child(
                                text(|| store().with(status_text)).text_color(p.text_muted),
                            )
                            .child(when(move || repository_loaded.get(), || {
                                button("新建 PR", "gh-new", || form(None)).disabled(submitting)
                            })),
                    )
                    .child(
                        view()
                            .flex_row()
                            .flex_1()
                            .min_h(px(0.0))
                            .gap(px(6.0))
                            .child(scroll().w(px(285.0)).flex_none().child(dynamic(move || {
                                view()
                                    .flex_col()
                                    .children(rows.get().into_iter().filter_map(|pr| {
                                        let number = pr["number"].as_u64()?;
                                        Some(
                                            view()
                                                .test_id(&format!("gh-pr-{number}"))
                                                .px(px(4.0))
                                                .h(px(25.0))
                                                .items_center()
                                                .cursor(Cursor::Pointer)
                                                .disabled(submitting)
                                                .disabled_style(|style| style.opacity(0.5))
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
