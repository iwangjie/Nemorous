use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nsh_base::{Editor, EditorApi, editor};
use nsh_core::prelude::*;

use crate::conflict_files::{self, ConflictFile, ResolveWith};
use crate::conflict_text::{self, ConflictChoice};
use crate::git::LineEnding;
use crate::git_history_actions::{self, RepoOperation};
use crate::merge_tasks::{self, MergeTask};
use crate::state::{self, app_signal, current_palette};

#[derive(Clone)]
pub(super) enum Source {
    Repo(PathBuf),
    Isolated { root: PathBuf, task: MergeTask },
}

impl Source {
    fn path(&self) -> PathBuf {
        match self {
            Self::Repo(path) => path.clone(),
            Self::Isolated { root, task } => task.worktree(root),
        }
    }
}

#[derive(Default, Clone)]
struct Workbench {
    source: Option<Source>,
    request: u64,
    files: Arc<Vec<String>>,
    initially_unresolved: Vec<String>,
    selected: Option<String>,
    loaded: Option<Arc<ConflictFile>>,
    operation: Option<RepoOperation>,
    issue: Option<String>,
    finish_error: Option<String>,
    saving: bool,
    pending_abort: bool,
    loading: bool,
    proposing: bool,
}

impl Workbench {
    fn can_control(&self) -> bool {
        self.operation.is_some() || matches!(self.source, Some(Source::Isolated { .. }))
    }

    fn can_continue(&self) -> bool {
        self.can_control()
            && !self.saving
            && !self.pending_abort
            && !self.loading
            && self.files.is_empty()
            && self.selected.is_none()
            && self.issue.is_none()
    }
}

enum FinishOutcome {
    Finished,
    MoreConflicts,
    Isolated {
        hash: String,
        push_error: Option<String>,
    },
}

#[derive(Default, Clone)]
struct Draft {
    text: String,
    original: String,
    block: usize,
    undo: Option<(String, String)>,
}

#[derive(Default)]
struct ResultEditor(Option<ComponentRef<Editor>>);

fn store() -> Signal<Workbench> {
    global::<Workbench>()
}

fn draft() -> Signal<Draft> {
    global::<Draft>()
}

fn draft_root() -> PathBuf {
    crate::config::config_dir().join("conflict-drafts")
}

pub(super) fn open(source: Source) {
    open_at(source, None);
}

pub(super) fn open_file(repo: PathBuf, path: String) {
    open_at(Source::Repo(repo), Some(path));
}

fn open_at(source: Source, path: Option<String>) {
    let request = store().with_untracked(|s| s.request.wrapping_add(1));
    let initially_unresolved = match &source {
        Source::Repo(_) => Vec::new(),
        Source::Isolated { task, .. } => task.original_conflicts.clone(),
    };
    store().set(Workbench {
        source: Some(source),
        request,
        initially_unresolved,
        ..Workbench::default()
    });
    app_signal().update(state::enter_modal_focus);
    load(path, false);
}

fn save_current_draft() {
    let snapshot = store().with_untracked(|s| {
        if s.saving {
            return None;
        }
        Some((s.source.as_ref()?.path(), s.loaded.clone()?))
    });
    if let Some((repo, file)) = snapshot {
        let text = draft().with_untracked(|d| (d.text != d.original).then(|| d.text.clone()));
        if let Some(text) = text {
            if let Err(err) = conflict_files::save_draft(&draft_root(), &repo, &file, &text) {
                state::show_toast("warn", format!("无法保存冲突草稿：{err}"));
            }
        }
    }
}

pub(crate) fn close_active() -> bool {
    let active = store().with_untracked(|s| s.source.is_some());
    if active {
        save_current_draft();
        let request = store().with_untracked(|s| s.request.wrapping_add(1));
        store().set(Workbench {
            request,
            ..Workbench::default()
        });
        draft().set(Draft::default());
        global::<ResultEditor>().set(ResultEditor::default());
    }
    active
}

pub(crate) fn intercept_close() -> bool {
    store().with_untracked(|s| s.source.is_some() && s.saving)
}

fn load(path: Option<String>, keep_draft: bool) {
    let Some(source) = store().with_untracked(|s| s.source.clone()) else {
        return;
    };
    if keep_draft {
        save_current_draft();
    }
    let request = store().with_untracked(|s| s.request.wrapping_add(1));
    store().update(|s| {
        s.request = request;
        s.selected = path.clone();
        s.loaded = None;
        s.issue = None;
        s.finish_error = None;
        s.pending_abort = false;
        s.loading = true;
        s.proposing = false;
    });
    draft().set(Draft::default());
    state::spawn_from_root(async move {
        let root = draft_root();
        let data = blocking(move || -> Result<_> {
            let repo = source.path();
            let files = merge_tasks::unresolved(&repo)?;
            let operation = git_history_actions::active_operation(&repo)?;
            let selected = path
                .filter(|path| files.contains(path))
                .or_else(|| files.first().cloned());
            let loaded = selected
                .as_deref()
                .map(|path| conflict_files::read(&repo, path));
            let text = loaded
                .as_ref()
                .and_then(|loaded| loaded.as_ref().ok())
                .map(|file| {
                    let original = file
                        .working
                        .as_deref()
                        .map_or_else(String::new, display_text);
                    let content = conflict_files::read_draft(&root, &repo, file)
                        .unwrap_or_else(|| original.clone());
                    (original, content)
                });
            Ok((files, operation, selected, loaded, text))
        })
        .await;
        if !store().with_untracked(|s| s.request == request && s.source.is_some()) {
            return;
        }
        store().update(|s| s.loading = false);
        match data {
            Ok(Ok((files, operation, selected, loaded, text))) => {
                if let Some((original, text)) = text {
                    draft().set(Draft {
                        original,
                        text,
                        ..Draft::default()
                    });
                }
                store().update(|s| {
                    for path in &files {
                        if !s.initially_unresolved.contains(path) {
                            s.initially_unresolved.push(path.clone());
                        }
                    }
                    s.files = Arc::new(files);
                    s.operation = operation;
                    s.selected = selected;
                    match loaded {
                        Some(Ok(file)) => s.loaded = Some(Arc::new(file)),
                        Some(Err(err)) => s.issue = Some(err.to_string()),
                        None => {}
                    }
                });
            }
            Ok(Err(err)) => store().update(|s| s.issue = Some(err.to_string())),
            Err(err) => store().update(|s| s.issue = Some(err.to_string())),
        }
    });
}

fn display_text(bytes: &[u8]) -> String {
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        "二进制版本".to_owned()
    } else {
        LineEnding::normalize_to_lf(&String::from_utf8_lossy(bytes))
    }
}

fn editor_handle() -> Option<std::rc::Rc<EditorApi>> {
    global::<ResultEditor>()
        .with_untracked(|r| r.0.and_then(|reference| reference.api::<EditorApi>()))
}

fn replace_draft(change: String) {
    draft().update(|d| {
        let previous = std::mem::replace(&mut d.text, change.clone());
        d.undo = Some((previous, change.clone()));
    });
    if let Some(editor) = editor_handle() {
        editor.set_value(change);
        editor.focus();
    }
}

fn undo_block() {
    let restore = draft().with_untracked(|d| {
        d.undo
            .as_ref()
            .filter(|(_, written)| written == &d.text)
            .map(|(before, _)| before.clone())
    });
    if let Some(text) = restore {
        draft().update(|d| {
            d.text = text.clone();
            d.undo = None;
        });
        if let Some(editor) = editor_handle() {
            editor.set_value(text);
        }
    }
}

fn selected_block(text: &str, index: usize) -> Result<Option<conflict_text::ConflictBlock>> {
    Ok(conflict_text::parse(text.as_bytes())?
        .into_iter()
        .nth(index))
}

pub(super) fn choose(choice: ConflictChoice) {
    let (text, block) = draft().with_untracked(|d| (d.text.clone(), d.block));
    let result = selected_block(&text, block).and_then(|selected| {
        selected
            .map(|block| conflict_text::apply(text.as_bytes(), &block, choice))
            .transpose()
    });
    match result {
        Ok(Some(bytes)) => {
            replace_draft(String::from_utf8_lossy(&bytes).into_owned());
            draft().update(|d| {
                d.block = d.block.min(
                    conflict_text::parse(d.text.as_bytes())
                        .map_or(0, |blocks| blocks.len().saturating_sub(1)),
                )
            });
        }
        Ok(None) => {}
        Err(err) => state::show_toast("warn", err.to_string()),
    }
}

fn smart() {
    let file = store().with_untracked(|s| s.loaded.clone());
    let Some(file) = file else {
        return;
    };
    let (text, index) = draft().with_untracked(|d| (d.text.clone(), d.block));
    let parsed = selected_block(&text, index).ok().flatten();
    if let Some(block) = parsed {
        if let Some(merged) = block
            .base
            .as_ref()
            .and_then(|base| conflict_text::smart_merge(base, &block.ours, &block.theirs))
        {
            let mut next = text.into_bytes();
            next.splice(block.whole, merged);
            replace_draft(String::from_utf8_lossy(&next).into_owned());
            return;
        }
    }
    if let [Some(base), Some(ours), Some(theirs)] = &file.versions {
        if let Some(merged) = conflict_text::smart_merge(base, ours, theirs) {
            let original = draft().with_untracked(|d| d.text == d.original);
            if original {
                replace_draft(display_text(&merged));
                return;
            }
        }
    }
    state::show_toast("info", "当前块需要人工处理");
}

fn propose() {
    let Some((file, request)) = store().with_untracked(|s| {
        if s.proposing || s.saving || !matches!(s.source, Some(Source::Isolated { .. })) {
            return None;
        }
        Some((s.loaded.clone()?, s.request))
    }) else {
        return;
    };
    let text = draft().with_untracked(|d| d.text.clone());
    let original = text.clone();
    let agent = app_signal().with_untracked(|s| {
        s.cfg
            .extra
            .get("ai_agent")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("auto")
            .to_owned()
    });
    store().update(|s| s.proposing = true);
    state::spawn_from_root(async move {
        let result = blocking(move || crate::merge_ai::propose(&agent, &file, &text)).await;
        if !store().with_untracked(|s| s.request == request && s.source.is_some()) {
            return;
        }
        store().update(|s| s.proposing = false);
        match result {
            Ok(Ok(candidate)) if draft().with_untracked(|d| d.text == original) => {
                replace_draft(candidate);
                state::show_toast("ok", "AI 候选已放入草稿，复核后可保存");
            }
            Ok(Ok(_)) => state::show_toast("info", "生成期间草稿有新的手工编辑"),
            Ok(Err(err)) => state::show_toast("warn", err.to_string()),
            Err(err) => state::show_toast("warn", err.to_string()),
        }
    });
}

pub(super) fn move_hunk(delta: i32) {
    let (text, current) = draft().with_untracked(|d| (d.text.clone(), d.block));
    let Ok(blocks) = conflict_text::parse(text.as_bytes()) else {
        return;
    };
    let next = current
        .saturating_add_signed(delta as isize)
        .min(blocks.len().saturating_sub(1));
    draft().update(|d| d.block = next);
    if let Some(block) = blocks.get(next) {
        let line = text.as_bytes()[..block.whole.start]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        if let Some(editor) = editor_handle() {
            editor.scroll_to_line(line as u32);
        }
    }
}

fn resolve(choice: ResolveWith) {
    let Some((repo, file, request)) = store().with_untracked(|s| {
        if s.saving {
            return None;
        }
        Some((s.source.as_ref()?.path(), s.loaded.clone()?, s.request))
    }) else {
        return;
    };
    let root = draft_root();
    let started = state::run_git_action(
        move || -> Result<()> {
            conflict_files::stage_resolution(&repo, &file, choice)?;
            conflict_files::remove_draft(&root, &repo, &file);
            Ok(())
        },
        move |result| {
            if !store().with_untracked(|s| s.request == request) {
                return;
            }
            store().update(|s| s.saving = false);
            match result {
                Ok(()) => {
                    state::show_toast("ok", "已标记为解决");
                    load(None, false);
                }
                Err(err) => store().update(|s| s.issue = Some(err.to_string())),
            }
        },
    );
    if started {
        store().update(|s| {
            s.saving = true;
            s.issue = None;
        });
    }
}

pub(super) fn save() {
    let ending = store().with_untracked(|s| s.loaded.as_ref().map(|file| file.ending));
    if let Some(ending) = ending {
        let bytes = draft().with_untracked(|d| ending.restore_bytes(&d.text));
        resolve(ResolveWith::Edited(bytes));
    }
}

pub(super) fn continue_if_ready() {
    if store().with_untracked(Workbench::can_continue) {
        finish(false, false);
    }
}

fn finish(abort: bool, push: bool) {
    let Some((source, operation, request)) = store().with_untracked(|s| {
        if s.saving || (!abort && !s.can_continue()) {
            return None;
        }
        Some((s.source.clone()?, s.operation, s.request))
    }) else {
        return;
    };
    let started = state::run_git_action(
        move || -> Result<FinishOutcome> {
            match source {
                Source::Repo(repo) => {
                    let operation =
                        operation.ok_or_else(|| anyhow::anyhow!("没有可继续的 Git 操作"))?;
                    match git_history_actions::control_operation(&repo, operation, abort) {
                        Ok(()) => Ok(FinishOutcome::Finished),
                        Err(_)
                            if !abort
                                && git_history_actions::active_operation(&repo).ok().flatten()
                                    == Some(operation)
                                && merge_tasks::unresolved(&repo)
                                    .is_ok_and(|files| !files.is_empty()) =>
                        {
                            Ok(FinishOutcome::MoreConflicts)
                        }
                        Err(error) => Err(error),
                    }
                }
                Source::Isolated { root, task } => {
                    if abort {
                        merge_tasks::abort(&root, &task)?;
                        Ok(FinishOutcome::Finished)
                    } else {
                        let hash = merge_tasks::finish(&root, &task)?;
                        let push_error = push
                            .then(|| {
                                merge_tasks::push_target(
                                    &task,
                                    &hash,
                                    &std::sync::atomic::AtomicBool::new(false),
                                )
                            })
                            .and_then(Result::err)
                            .map(|err| err.to_string());
                        Ok(FinishOutcome::Isolated { hash, push_error })
                    }
                }
            }
        },
        move |result| {
            if !store().with_untracked(|s| s.request == request) {
                return;
            }
            store().update(|s| s.saving = false);
            match result {
                Ok(FinishOutcome::MoreConflicts) => load(None, false),
                Ok(outcome) => {
                    draft().set(Draft::default());
                    state::close_active_modal();
                    if let FinishOutcome::Isolated { hash, push_error } = outcome {
                        let push_status = push_error
                            .map(|err| format!("\n\n本地合并已完成，推送失败\n{err}"))
                            .unwrap_or_default();
                        super::git_advanced::report(
                            "合并校验通过".to_owned(),
                            format!(
                                "结果提交 {hash}\n\n✓ 合并来源与任务一致\n✓ 索引没有未解决条目\n✓ 隔离工作树干净\n✓ 提交父链与合并方式一致\n✓ 非冲突索引条目保持原样\n✓ 已解决文件没有冲突标记{push_status}"
                            ),
                        );
                    }
                    state::show_toast(
                        "ok",
                        if abort {
                            "已中止 Git 操作"
                        } else {
                            "已完成 Git 操作"
                        },
                    );
                }
                Err(err) => store().update(|s| s.finish_error = Some(err.to_string())),
            }
        },
    );
    if started {
        store().update(|s| {
            s.saving = true;
            s.finish_error = None;
            s.pending_abort = false;
        });
    }
}

fn btn(label: &'static str, id: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    let p = current_palette();
    view()
        .label(label)
        .test_id(id)
        .px(px(7.0))
        .h(px(25.0))
        .rounded(px(4.0))
        .items_center()
        .justify_center()
        .cursor(Cursor::Pointer)
        .bg(p.surface2)
        .hover(move |s| s.bg(p.surface_hover))
        .on_press(action)
        .child(text(label).text_color(p.text_primary).text_size(px(11.0)))
}

fn files_panel() -> impl IntoElement {
    dynamic(|| {
        let p = current_palette();
        store().with(|s| {
            view()
                .w(px(195.0))
                .flex_none()
                .h_full()
                .border_r_1()
                .border_color(p.border)
                .flex_col()
                .child(
                    view()
                        .h(px(26.0))
                        .px(px(8.0))
                        .items_center()
                        .child(text(format!("待处理 {}", s.files.len())).text_color(p.text_muted)),
                )
                .child(
                    scroll().flex_1().child(view().flex_col().children(
                        s.initially_unresolved.iter().map(|path| {
                            let path = path.clone();
                            let unresolved = s.files.contains(&path);
                            let selected = s.selected.as_ref() == Some(&path);
                            view()
                                .test_id(&format!("conflict-file-{path}"))
                                .h(px(26.0))
                                .px(px(8.0))
                                .bg(if selected { p.accent_soft } else { p.surface })
                                .items_center()
                                .cursor(if unresolved {
                                    Cursor::Pointer
                                } else {
                                    Cursor::Default
                                })
                                .on_press({
                                    let path = path.clone();
                                    move || {
                                        if unresolved {
                                            load(Some(path.clone()), true);
                                        }
                                    }
                                })
                                .child(
                                    text(if unresolved {
                                        path
                                    } else {
                                        format!("✓ {path}")
                                    })
                                    .text_size(px(11.0))
                                    .number_of_lines(1u32)
                                    .text_color(if unresolved { p.text_primary } else { p.green }),
                                )
                        }),
                    )),
                )
                .into_element()
        })
    })
}

fn track_scroll(refs: [ComponentRef<Editor>; 3]) {
    let previous = stored([0.0_f32; 3]);
    let timer = set_interval(Duration::from_millis(40), move || {
        let editors = refs
            .each_ref()
            .map(|reference| reference.api::<EditorApi>());
        let positions = editors.each_ref().map(|editor| {
            editor
                .as_ref()
                .map_or(0.0, |editor| editor.scroll_offset().y.0)
        });
        previous.with(|previous: &mut [f32; 3]| {
            let changed = (0..3).find(|&index| (previous[index] - positions[index]).abs() > 0.5);
            if let Some(index) = changed {
                let target = positions[index];
                for (other, editor) in editors.iter().enumerate() {
                    if let Some(editor) = editor {
                        if other != index && (positions[other] - target).abs() > 0.5 {
                            let current = editor.scroll_offset();
                            editor.set_scroll_offset(point(current.x, px(target)));
                        }
                        previous[other] = editor.scroll_offset().y.0;
                    }
                }
            }
        });
    });
    cleanup(move || timer.cancel());
}

fn views(file: Arc<ConflictFile>) -> impl IntoElement {
    let p = current_palette();
    let refs = [
        component_ref::<Editor>(),
        component_ref::<Editor>(),
        component_ref::<Editor>(),
    ];
    global::<ResultEditor>().set(ResultEditor(Some(refs[1])));
    track_scroll(refs);
    let rebase = store().with_untracked(|s| s.operation == Some(RepoOperation::Rebase));
    let titles = if rebase {
        ["当前基底", "解决结果", "正在重放的提交"]
    } else {
        ["当前目标", "解决结果", "合并来源"]
    };
    let empty = || "此侧没有文件".to_owned();
    let raw = [
        file.versions[1].as_deref().map_or_else(empty, display_text),
        draft().with_untracked(|d| d.text.clone()),
        file.versions[2].as_deref().map_or_else(empty, display_text),
    ];
    let version = store().with_untracked(|s| s.request);
    view()
        .flex_1()
        .flex_col()
        .child(dynamic(move || {
            let (total, selected, can_undo) = draft().with(|d| {
                (
                    conflict_text::parse(d.text.as_bytes()).map_or(0, |blocks| blocks.len()),
                    d.block,
                    d.undo.as_ref().is_some_and(|(_, last)| last == &d.text),
                )
            });
            let side_picker = file.binary || file.stages[1].is_none() || file.stages[2].is_none();
            let ours = if file.stages[1].is_some() {
                ResolveWith::Stage(1)
            } else {
                ResolveWith::Delete
            };
            let theirs = if file.stages[2].is_some() {
                ResolveWith::Stage(2)
            } else {
                ResolveWith::Delete
            };
            let (isolated, proposing) = store().with(|s| {
                (
                    matches!(s.source, Some(Source::Isolated { .. })),
                    s.proposing,
                )
            });
            let mut tools = view()
                .flex_row()
                .h(px(32.0))
                .items_center()
                .gap(px(4.0))
                .px(px(6.0));
            if side_picker {
                tools = tools
                    .child(btn(
                        if file.stages[1].is_some() {
                            "使用整份当前版本"
                        } else {
                            "删除文件"
                        },
                        "conflict-whole-ours",
                        move || resolve(ours.clone()),
                    ))
                    .child(btn(
                        if file.stages[2].is_some() {
                            "使用整份来源版本"
                        } else {
                            "删除文件"
                        },
                        "conflict-whole-theirs",
                        move || resolve(theirs.clone()),
                    ));
            }
            if total > 0 && !file.binary {
                tools = tools
                    .child(
                        text(format!("{}/{}", (selected + 1).min(total), total))
                            .text_color(p.text_secondary),
                    )
                    .child(btn("上一块", "conflict-prev", || move_hunk(-1)))
                    .child(btn("下一块", "conflict-next", || move_hunk(1)))
                    .child(btn("取当前", "conflict-take-ours", || {
                        choose(ConflictChoice::Ours)
                    }))
                    .child(btn("取来源", "conflict-take-theirs", || {
                        choose(ConflictChoice::Theirs)
                    }))
                    .child(btn("取共同基底", "conflict-take-base", || {
                        choose(ConflictChoice::Base)
                    }))
                    .child(btn("当前后接来源", "conflict-both-ours", || {
                        choose(ConflictChoice::BothOursFirst)
                    }))
                    .child(btn("来源后接当前", "conflict-both-theirs", || {
                        choose(ConflictChoice::BothTheirsFirst)
                    }))
                    .child(btn("简单合并", "conflict-smart", smart));
            }
            if isolated && total > 0 && !file.binary {
                tools = if proposing {
                    tools.child(
                        text("正在生成候选…")
                            .text_color(p.text_muted)
                            .text_size(px(10.5)),
                    )
                } else {
                    tools.child(btn("AI 候选", "conflict-ai", propose))
                };
            }
            if can_undo {
                tools = tools.child(btn("撤销块操作", "conflict-undo", undo_block));
            }
            tools
                .child(view().flex_1())
                .child((!file.binary).then(|| btn("保存并标记解决", "conflict-save", save)))
        }))
        .child(
            view()
                .flex_1()
                .flex_row()
                .children(titles.into_iter().enumerate().map(|(i, title)| {
                    view()
                        .flex_1()
                        .flex_col()
                        .overflow_hidden()
                        .border_r_1()
                        .border_color(p.border)
                        .child(
                            view()
                                .h(px(24.0))
                                .px(px(7.0))
                                .items_center()
                                .bg(p.surface2)
                                .child(
                                    text(title).text_size(px(11.0)).text_color(p.text_secondary),
                                ),
                        )
                        .child(
                            editor()
                                .component_ref(&refs[i])
                                .test_id(["conflict-ours", "conflict-result", "conflict-theirs"][i])
                                .flex_1()
                                .default_value(Some(raw[i].clone().into()))
                                .read_only(move || {
                                    i != 1
                                        || store().with(|s| {
                                            s.saving || s.loaded.as_ref().is_some_and(|f| f.binary)
                                        })
                                })
                                .auto_focus(i == 1)
                                .on_change(move |value: &SharedString| {
                                    if i == 1 && store().with_untracked(|s| s.request == version) {
                                        draft().update(|d| d.text = value.to_string());
                                    }
                                }),
                        )
                })),
        )
}

pub(super) fn is_open() -> bool {
    store().with(|s| s.source.is_some())
}

pub(super) fn layer() -> impl IntoElement {
    let layout = computed(|| store().with(|s| (s.source.is_some(), s.request, s.loaded.is_some())));
    dynamic(move || {
        let (open, _, _) = layout.get();
        if !open {
            return view().into_element();
        }
        let p = current_palette();
        let file = store().with_untracked(|s| s.loaded.clone());
        view()
            .absolute()
            .inset_0()
            .p(px(12.0))
            .items_center()
            .justify_center()
            .bg(rgba(0x00000070))
            .child(
                view()
                    .test_id("git-conflict-workbench")
                    .w_full()
                    .max_w(px(1320.0))
                    .h_full()
                    .max_h(px(740.0))
                    .rounded(px(8.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .overflow_hidden()
                    .flex_col()
                    .child(
                        view()
                            .h(px(37.0))
                            .px(px(9.0))
                            .items_center()
                            .flex_row()
                            .gap(px(6.0))
                            .bg(p.rail_bg)
                            .child(
                                text("冲突工作台")
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary),
                            )
                            .child(dynamic(move || {
                                store().with(|s| {
                                    text(s.selected.clone().unwrap_or_default())
                                        .text_color(p.text_secondary)
                                })
                            }))
                            .child(view().flex_1())
                            .child(btn("关闭", "conflict-close", state::close_active_modal)),
                    )
                    .child(dynamic(move || {
                        let (
                            ready,
                            isolated,
                            can_control,
                            operation,
                            pending_abort,
                            issue,
                            finish_error,
                            saving,
                        ) = store().with(|s| {
                            (
                                s.can_continue(),
                                matches!(s.source, Some(Source::Isolated { .. })),
                                s.can_control(),
                                s.operation,
                                s.pending_abort,
                                s.issue.clone(),
                                s.finish_error.clone(),
                                s.saving,
                            )
                        });
                        let mut bar = view()
                            .flex_row()
                            .gap(px(5.0))
                            .items_center()
                            .px(px(9.0))
                            .py(px(4.0));
                        let mut actions = view().flex_col();
                        if let Some(issue) = issue {
                            actions = actions.child(
                                text(issue)
                                    .text_color(p.danger)
                                    .text_size(px(11.0))
                                    .number_of_lines(3u32)
                                    .px(px(9.0)),
                            );
                        }
                        if let Some(error) = finish_error {
                            actions = actions.child(
                                text(error)
                                    .test_id("conflict-finish-error")
                                    .text_color(p.danger)
                                    .text_size(px(11.0))
                                    .number_of_lines(3u32)
                                    .px(px(9.0)),
                            );
                        }
                        if saving {
                            return actions
                                .child(bar.child(text("Git 操作进行中…").text_color(p.text_muted)))
                                .into_element();
                        }
                        if ready {
                            bar = bar.child(btn(
                                match (isolated, operation) {
                                    (true, _) => "完成本地合并",
                                    (_, Some(RepoOperation::Merge)) => "继续合并",
                                    (_, Some(RepoOperation::Rebase)) => "继续变基",
                                    (_, Some(RepoOperation::CherryPick)) => "继续 Cherry-pick",
                                    (_, Some(RepoOperation::Revert)) => "继续还原",
                                    _ => "继续 Git 操作",
                                },
                                "conflict-continue",
                                || finish(false, false),
                            ));
                            if isolated {
                                bar = bar.child(btn(
                                    "完成并推送",
                                    "conflict-continue-push",
                                    || finish(false, true),
                                ));
                            }
                        }
                        if pending_abort {
                            bar = bar
                                .child(
                                    text("中止将丢弃本次未保存的合并结果。").text_color(p.danger),
                                )
                                .child(btn("确认中止", "conflict-abort-confirm", || {
                                    finish(true, false)
                                }))
                                .child(btn("取消", "conflict-abort-cancel", || {
                                    store().update(|s| s.pending_abort = false)
                                }));
                        } else if can_control {
                            bar = bar.child(btn("中止…", "conflict-abort", || {
                                store().update(|s| s.pending_abort = true)
                            }));
                        }
                        actions.child(bar).into_element()
                    }))
                    .child(
                        view().flex_1().flex_row().child(files_panel()).child(
                            file.map(|file| views(file).into_element())
                                .unwrap_or_else(|| {
                                    let hint = store().with(|s| {
                                        if s.loading {
                                            "正在读取冲突…"
                                        } else if s.files.is_empty() && s.issue.is_none() {
                                            "没有待处理的冲突"
                                        } else {
                                            "选择左侧文件查看冲突"
                                        }
                                    });
                                    view()
                                        .flex_1()
                                        .p(px(10.0))
                                        .child(text(hint).text_color(p.text_muted))
                                        .into_element()
                                }),
                        ),
                    ),
            )
            .into_element()
    })
}
