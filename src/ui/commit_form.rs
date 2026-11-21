use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use nsh_base::{Editor, EditorApi, editor};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use super::git_review::IncludedHunks;
use crate::git::{ChangedFile, RepoInfo, run_git_string};
use crate::git_commit::{self, CommitRequest, CommitSelection};
use crate::git_remotes::{self, RemoteAction, RemoteStatus};
use crate::git_review_actions::read_selection_versions;
use crate::selected_changes::{Selection, apply_selection};
use crate::state::{self, app_signal, current_palette};

#[derive(Default)]
struct FormEditor(Option<ComponentRef<Editor>>);

#[derive(Clone, Default)]
struct Form {
    message: String,
    amend: bool,
    generating: bool,
    focused: bool,
}

type Target = (RepoInfo, Vec<(ChangedFile, Option<IncludedHunks>)>);

fn selected_targets() -> Vec<Target> {
    app_signal().with_untracked(|s| {
        let mut targets = Vec::new();
        for status in &s.statuses {
            let mut seen = BTreeSet::new();
            let files = status
                .files
                .iter()
                .filter_map(|file| {
                    let key = file.key();
                    if s.cfg.matches_change_list(&key)
                        && s.checked_changes.contains(&key)
                        && seen.insert(key.clone())
                    {
                        Some((file.clone(), s.included_hunks.get(&key).cloned()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if !files.is_empty() {
                targets.push((status.repo.clone(), files));
            }
        }
        targets
    })
}

fn first_repo() -> Option<RepoInfo> {
    let targets = selected_targets();
    targets.first().map(|(repo, _)| repo.clone()).or_else(|| {
        app_signal().with_untracked(|s| {
            s.change_selection
                .as_ref()
                .and_then(|selection| {
                    s.repos
                        .iter()
                        .find(|repo| repo.id == selection.file.repo_id)
                })
                .or_else(|| s.repos.first())
                .cloned()
        })
    })
}

fn form_editor() -> Option<std::rc::Rc<EditorApi>> {
    global::<FormEditor>()
        .with_untracked(|editor| editor.0.and_then(|reference| reference.api::<EditorApi>()))
}

fn set_message(value: String, focus: bool) {
    global::<Form>().update(|form| form.message = value.clone());
    if let Some(editor) = form_editor() {
        editor.set_value(value);
        if focus {
            editor.focus();
        }
    }
}

pub(super) fn has_focus() -> bool {
    global::<Form>().with(|form| form.focused)
}

pub(super) fn submit() {
    commit(false);
}

pub(super) fn focus() {
    app_signal().update(|s| {
        s.show_left_panel = true;
        s.left_tab = state::LeftTab::Commit;
        s.focus_zone = state::FocusZone::CommitTree;
    });
    if let Some(editor) = form_editor() {
        editor.focus();
    } else {
        let _ = set_timeout(std::time::Duration::from_millis(30), || {
            if let Some(editor) = form_editor() {
                editor.focus();
            }
        });
    }
}

fn unsafe_dirty_selection(targets: &[Target]) -> bool {
    app_signal().with_untracked(|s| {
        targets.iter().any(|(_, files)| {
            files.iter().any(|(file, _)| {
                let key = file.key();
                s.working_diff
                    .as_ref()
                    .is_some_and(|w| w.file.key() == key && w.is_dirty())
                    || s.code_tabs
                        .iter()
                        .any(|t| t.file.key() == key && t.is_dirty())
            })
        })
    })
}

fn commit_repo(
    repo: &RepoInfo,
    files: Vec<(ChangedFile, Option<IncludedHunks>)>,
    message: String,
    amend: bool,
) -> Result<git_commit::CommitOutcome> {
    let path = Path::new(&repo.path);
    let expected_head = git_commit::head_oid(path)?;
    let mut selections = Vec::new();
    let mut all = BTreeSet::new();
    for (file, partial) in files {
        if let Some(partial) = partial {
            let versions = read_selection_versions(&file, &partial.view)?;
            let bytes = apply_selection(
                &versions.old,
                &versions.new,
                &Selection::Hunks(partial.hunks),
                false,
            )?;
            selections.push(CommitSelection::Partial {
                path: file.path,
                expected: versions.entry,
                bytes,
                mode: versions.mode,
            });
        } else {
            all.insert(file.path);
            if !file.orig_path.is_empty() {
                all.insert(file.orig_path);
            }
        }
    }
    if !all.is_empty() {
        selections.push(CommitSelection::All {
            paths: all.into_iter().collect(),
        });
    }
    git_commit::commit_selected(
        path,
        CommitRequest {
            expected_head,
            selections,
            message,
            amend,
        },
    )
}

fn commit(push: bool) {
    let (message, amend) =
        global::<Form>().with_untracked(|form| (form.message.trim().to_owned(), form.amend));
    if message.is_empty() {
        state::show_toast("warn", "请填写提交说明");
        return;
    }
    let mut targets = selected_targets();
    if targets.is_empty() && amend {
        if let Some(repo) = first_repo() {
            targets.push((repo, Vec::new()));
        }
    }
    if targets.is_empty() {
        state::show_toast("warn", "请勾选要提交的变更");
        return;
    }
    if unsafe_dirty_selection(&targets) {
        state::show_toast("warn", "所选文件包含未保存编辑，请先保存");
        return;
    }
    log::debug!(
        "提交规划：{} 个仓库，{} 个部分勾选文件",
        targets.len(),
        targets
            .iter()
            .flat_map(|(_, files)| files)
            .filter(|(_, included)| included.is_some())
            .count()
    );
    let submitted = message.clone();
    state::run_git_action(
        move || {
            let mut results = Vec::new();
            let mut pushed = Vec::new();
            for (repo, files) in targets {
                let keys = files.iter().map(|(f, _)| f.key()).collect::<Vec<_>>();
                let result = commit_repo(&repo, files, message.clone(), amend);
                if push
                    && result
                        .as_ref()
                        .is_ok_and(|outcome| outcome.index_warning.is_none())
                {
                    pushed.push(repo.clone());
                }
                results.push((repo, keys, result));
            }
            let reports = if push {
                git_remotes::run_batch(
                    &pushed,
                    RemoteAction::Push,
                    &std::sync::atomic::AtomicBool::new(false),
                )
            } else {
                Vec::new()
            };
            (results, reports)
        },
        move |(results, reports)| {
            let mut details = Vec::new();
            let mut committed = 0;
            let mut failed = 0;
            app_signal().update(|s| {
                for (repo, keys, result) in results {
                    match result {
                        Ok(result) => {
                            committed += 1;
                            let short = &result.hash[..result.hash.len().min(8)];
                            if let Some(warning) = result.index_warning {
                                details.push(format!("{} {short}：{warning}", repo.name));
                            } else {
                                for key in keys {
                                    s.checked_changes.remove(&key);
                                    s.included_hunks.remove(&key);
                                }
                            }
                        }
                        Err(err) => {
                            failed += 1;
                            details.push(format!("{}：{err}", repo.name));
                        }
                    }
                }
            });
            for report in reports {
                if report.status != RemoteStatus::Done {
                    details.push(format!("{} 推送：{}", report.repo.name, report.detail));
                }
            }
            if failed == 0 && global::<Form>().with_untracked(|f| f.message.trim() == submitted) {
                set_message(String::new(), false);
                global::<Form>().update(|form| form.amend = false);
            }
            let reason = if details.is_empty() {
                String::new()
            } else {
                format!("\n{}", details.join("\n"))
            };
            state::show_toast(
                if details.is_empty() { "ok" } else { "warn" },
                format!("提交成功 {committed} 个仓库，失败 {failed} 个{reason}"),
            );
        },
    );
}

fn undo() {
    let Some(repo) = first_repo() else {
        return;
    };
    state::spawn_from_root(async move {
        let path = repo.path.clone();
        let result = blocking(move || git_commit::head_oid(Path::new(&path))).await;
        let Ok(Ok(Some(head))) = result else {
            state::show_toast("warn", "当前仓库没有可撤销的提交");
            return;
        };
        let label = format!(
            "撤销 {} 最近提交 {}？提交内容将保留在暂存区。",
            repo.name,
            &head[..8]
        );
        super::git_prompt::confirm(label, move || {
            let (path, expected) = (repo.path.clone(), head.clone());
            state::run_git_action(
                move || git_commit::undo_last_commit(Path::new(&path), &expected),
                |result| match result {
                    Ok(()) => state::show_toast("ok", "已撤销最近提交"),
                    Err(err) => state::show_toast("warn", err.to_string()),
                },
            );
        });
    });
}

fn history() {
    let mut repos = selected_targets()
        .into_iter()
        .map(|(repo, _)| repo)
        .collect::<Vec<_>>();
    if repos.is_empty() {
        repos = app_signal().with_untracked(|s| s.repos.clone());
    }
    state::spawn_from_root(async move {
        let subjects = blocking(move || {
            let mut seen = BTreeSet::new();
            let mut messages = Vec::new();
            for repo in repos {
                if let Ok(text) =
                    run_git_string(Path::new(&repo.path), &["log", "-20", "--format=%s"])
                {
                    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                        if seen.insert(line.to_owned()) {
                            messages.push(line.to_owned());
                        }
                    }
                }
            }
            messages.truncate(100);
            messages
        })
        .await
        .unwrap_or_default();
        if subjects.is_empty() {
            state::show_toast("info", "所选仓库没有提交记录");
        } else {
            state::show_popup_menu(
                subjects
                    .into_iter()
                    .map(|msg| {
                        MenuItem::new(msg.clone()).on_select(move || set_message(msg.clone(), true))
                    })
                    .collect(),
            );
        }
    });
}

fn generate() {
    if global::<Form>().with_untracked(|form| form.generating) {
        return;
    }
    let targets = selected_targets();
    if targets.is_empty() {
        state::show_toast("warn", "请先勾选要提交的文件");
        return;
    }
    let (agent, workspace) = app_signal().with_untracked(|s| {
        let agent = s
            .cfg
            .extra
            .get("ai_agent")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("auto")
            .to_owned();
        (agent, s.cfg.workspace_path.clone())
    });
    let draft = global::<Form>().with_untracked(|f| f.message.clone());
    global::<Form>().update(|form| form.generating = true);
    state::spawn_from_root(async move {
        let paths = targets
            .into_iter()
            .flat_map(|(_, files)| files)
            .take(40)
            .collect::<Vec<_>>();
        let result =
            blocking(move || -> Result<String> { crate::commit_message::generate(&agent, paths) })
                .await;
        global::<Form>().update(|form| form.generating = false);
        ensure_same_workspace(workspace, draft, result.ok());
    });
}

fn ensure_same_workspace(workspace: String, draft: String, result: Option<Result<String>>) {
    if !app_signal().with_untracked(|s| s.cfg.workspace_path == workspace) {
        return;
    }
    match result {
        Some(Ok(message)) if global::<Form>().with_untracked(|f| f.message == draft) => {
            set_message(message, true);
            state::show_toast("ok", "已生成提交说明");
        }
        Some(Ok(_)) => state::show_toast("info", "提交说明已手动更改"),
        Some(Err(err)) => state::show_toast("warn", err.to_string()),
        None => state::show_toast("warn", "生成提交说明的任务已中断"),
    }
}

fn more() {
    let amend = global::<Form>().with_untracked(|form| form.amend);
    state::show_popup_menu(vec![
        MenuItem::new("提交并推送").on_select(|| commit(true)),
        MenuItem::new("修订最近提交").checked(amend).on_select(|| {
            global::<Form>().update(|form| form.amend = !form.amend);
        }),
        MenuItem::new("撤销最近提交…").on_select(undo),
        MenuItem::new("提交信息历史…").on_select(history),
        MenuItem::new("AI 生成提交说明").on_select(generate),
    ]);
}

fn button(
    label: &'static str,
    id: &'static str,
    primary: bool,
    handler: impl Fn() + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .label(label)
        .px(px(8.0))
        .h(px(25.0))
        .rounded(px(4.0))
        .items_center()
        .justify_center()
        .cursor(Cursor::Pointer)
        .bg(if primary { p.accent } else { p.surface2 })
        .on_press(handler)
        .child(
            text(label)
                .text_color(if primary {
                    p.accent_text
                } else {
                    p.text_primary
                })
                .text_size(px(11.5)),
        )
}

pub(super) fn form() -> impl IntoElement {
    let reference = component_ref::<Editor>();
    global::<FormEditor>().set(FormEditor(Some(reference)));
    let initial = global::<Form>().with_untracked(|form| form.message.clone());
    view()
        .test_id("git-commit-form")
        .flex_none()
        .w_full()
        .flex_col()
        .px(px(8.0))
        .py(px(6.0))
        .gap(px(5.0))
        .border_t_1()
        .border_color(move || current_palette().border)
        .child(
            editor()
                .component_ref(&reference)
                .test_id("git-commit-message")
                .h(px(62.0))
                .default_value(Some(initial.into()))
                .placeholder(Some("提交说明".into()))
                .soft_wrap(true)
                .line_numbers(false)
                .folding(false)
                .auto_close(false)
                .font_size(px(12.0))
                .on_focus(|| {
                    state::set_text_input_focused(true);
                    global::<Form>().update(|form| form.focused = true);
                })
                .on_blur(|| {
                    state::set_text_input_focused(false);
                    global::<Form>().update(|form| form.focused = false);
                })
                .on_change(|value: &SharedString| {
                    global::<Form>().update(|form| form.message = value.to_string());
                }),
        )
        .child(
            view()
                .flex_row()
                .gap(px(5.0))
                .child(dynamic(|| {
                    let amend = global::<Form>().with(|form| form.amend);
                    button(
                        if amend { "修订" } else { "提交" },
                        "git-commit-submit",
                        true,
                        || commit(false),
                    )
                }))
                .child(button("更多…", "git-commit-more", false, more)),
        )
}
