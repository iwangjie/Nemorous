use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result, ensure};
use nsh_base::{Editor, EditorApi, editor};
use nsh_core::prelude::*;

use crate::git::{self, ChangedFile, LineEnding};
use crate::git_index::{self, IndexEntry};
use crate::selected_changes::{self, Selection};
use crate::state::{self, app_signal, current_palette};

#[derive(Clone)]
struct Versions {
    texts: [String; 3],
    index: Option<IndexEntry>,
    ending: LineEnding,
}

#[derive(Clone, Default)]
struct Review {
    file: Option<ChangedFile>,
    generation: u64,
    data: Option<Rc<Versions>>,
    draft: String,
    issue: Option<String>,
    saving: bool,
}

#[derive(Default)]
struct IndexEditor(Option<ComponentRef<Editor>>);

fn signal() -> Signal<Review> {
    global::<Review>()
}

pub(super) fn open_active() {
    if let Some(file) =
        app_signal().with_untracked(|s| s.working_diff.as_ref().map(|w| w.file.clone()))
    {
        open(file);
    }
}

pub(super) fn open(file: ChangedFile) {
    if app_signal().with_untracked(|s| s.working_diff.as_ref().is_some_and(|w| w.is_dirty())) {
        state::show_toast("warn", "请先保存当前差异中的编辑");
        return;
    }
    let store = signal();
    let generation = store.with_untracked(|r| r.generation.wrapping_add(1));
    store.set(Review {
        file: Some(file.clone()),
        generation,
        ..Review::default()
    });
    app_signal().update(state::enter_modal_focus);
    state::spawn_from_root(async move {
        let result = blocking(move || read_versions(&file)).await;
        store.update(|r| {
            if r.generation != generation {
                return;
            }
            match result {
                Ok(Ok(versions)) => {
                    r.draft = versions.texts[1].clone();
                    r.data = Some(Rc::new(versions));
                }
                Ok(Err(err)) => r.issue = Some(format!("{err:#}")),
                Err(err) => r.issue = Some(err.to_string()),
            }
        });
    });
}

fn read_versions(file: &ChangedFile) -> Result<Versions> {
    let dir = Path::new(&file.repo_path);
    let entry = git_index::read_index_entry(dir, &file.path)?;
    let index = entry
        .as_ref()
        .map(|entry| git_index::read_index_blob(dir, entry))
        .transpose()?
        .unwrap_or_default();
    let ending = LineEnding::detect(&index);
    let head = crate::git_commit::head_oid(dir)?
        .as_deref()
        .map(|head| git_index::tree_file(dir, head, file.old_path_or_current()))
        .transpose()?
        .flatten();
    let head = head
        .as_ref()
        .map(|entry| git_index::read_index_blob(dir, entry))
        .transpose()?
        .unwrap_or_default();
    let worktree = git::read_worktree_source(Path::new(&file.abs_path()))
        .with_context(|| format!("无法读取工作区文件 {}", file.path))?;
    ensure!(worktree.as_ref().is_none_or(|file| !file.binary && !file.too_large),
        "此文件超出暂存区文本审阅范围");
    let text = |blob: Vec<u8>| LineEnding::normalize_to_lf(&String::from_utf8_lossy(&blob));
    Ok(Versions {
        texts: [text(head), text(index), worktree.map_or_else(String::new, |file| file.text)],
        index: entry,
        ending,
    })
}

pub(super) fn is_open() -> bool {
    signal().with(|r| r.file.is_some())
}

fn dirty(r: &Review) -> bool {
    r.data.as_ref().is_some_and(|data| data.texts[1] != r.draft)
}

pub(crate) fn intercept_close() -> bool {
    let (busy, dirty) = signal().with_untracked(|r| (r.saving, dirty(r)));
    if busy {
        return true;
    }
    if dirty {
        save(true);
        return true;
    }
    false
}

pub(crate) fn close_active() -> bool {
    let store = signal();
    let old = store.with_untracked(|r| r.file.as_ref().map(|_| r.generation));
    if let Some(generation) = old {
        store.set(Review {
            generation: generation.wrapping_add(1),
            ..Review::default()
        });
        global::<IndexEditor>().set(IndexEditor::default());
        true
    } else {
        false
    }
}

fn active_editor() -> Option<Rc<EditorApi>> {
    global::<IndexEditor>()
        .with_untracked(|r| r.0.and_then(|reference| reference.api::<EditorApi>()))
}

pub(super) fn save_active() {
    save(false);
}

fn save(close: bool) {
    let store = signal();
    let Some((file, entry, ending, draft, generation)) = store.with_untracked(|r| {
        let data = r.data.as_ref()?;
        let entry = data.index.clone()?;
        (!r.saving && dirty(r)).then(|| {
            (
                r.file.clone().unwrap(),
                entry,
                data.ending,
                r.draft.clone(),
                r.generation,
            )
        })
    }) else {
        return;
    };
    let saved = draft.clone();
    let started = state::run_git_action(
        move || {
            git_index::replace_index_text(
                Path::new(&file.repo_path),
                &file.path,
                Some(&entry),
                Some(ending.restore_bytes(&draft)),
                &entry.mode,
            )
        },
        move |result| {
            let mut finished = false;
            store.update(|r| {
                if r.generation != generation {
                    return;
                }
                r.saving = false;
                match result {
                    Ok(Some(next)) => {
                        if let Some(data) = &mut r.data {
                            let data = Rc::make_mut(data);
                            data.index = Some(next);
                            data.texts[1] = saved;
                        }
                        r.issue = None;
                        finished = !dirty(r);
                    }
                    Ok(None) => r.issue = Some("暂存版本已不存在".to_owned()),
                    Err(err) => r.issue = Some(err.to_string()),
                }
            });
            if finished {
                if close {
                    state::close_active_modal();
                }
                state::show_toast("ok", "已保存暂存版本");
            }
        },
    );
    if started {
        store.update(|r| {
            r.saving = true;
            r.issue = None;
        });
    }
}

fn accept(from_head: bool) {
    let store = signal();
    let Some((left, right)) = store.with_untracked(|r| {
        let data = r.data.as_ref()?;
        if r.saving || data.index.is_none() {
            return None;
        }
        Some(if from_head {
            (data.texts[0].clone(), r.draft.clone())
        } else {
            (r.draft.clone(), data.texts[2].clone())
        })
    }) else {
        return;
    };
    let Some(editor) = active_editor() else {
        return;
    };
    let cursor = editor
        .selections()
        .last()
        .map_or(0, |selection| selection.head.line as usize);
    let hunks = selected_changes::exact_hunks(left.as_bytes(), right.as_bytes());
    let hunk = hunks.iter().min_by_key(|hunk| {
        let (first, last) = if from_head {
            (hunk.new_start, hunk.new_end)
        } else {
            (hunk.old_start, hunk.old_end)
        };
        cursor.abs_diff(cursor.clamp(first, last))
    });
    let Some(hunk) = hunk else {
        state::show_toast("info", "没有可选的变更块");
        return;
    };
    match selected_changes::apply_selection(
        left.as_bytes(),
        right.as_bytes(),
        &Selection::Hunks(vec![hunk.clone()]),
        from_head,
    ) {
        Ok(bytes) => {
            let changed = String::from_utf8_lossy(&bytes).into_owned();
            store.update(|r| r.draft = changed.clone());
            editor.set_value(changed);
            save(false);
        }
        Err(err) => state::show_toast("warn", err.to_string()),
    }
}

fn button(
    label: &'static str,
    id: &'static str,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .label(label)
        .px(px(8.0))
        .h(px(26.0))
        .rounded(px(4.0))
        .bg(p.surface2)
        .items_center()
        .justify_center()
        .cursor(Cursor::Pointer)
        .on_press(on_press)
        .child(text(label).text_size(px(11.5)).text_color(p.text_primary))
}

pub(super) fn layer() -> impl IntoElement {
    let loaded = computed(|| {
        signal().with(|r| {
            (
                r.file.as_ref().map(ChangedFile::key),
                r.generation,
                r.data.is_some(),
            )
        })
    });
    dynamic(move || {
        let (file, _, loaded) = loaded.get();
        let Some(file) = file else {
            return view().into_element();
        };
        let p = current_palette();
        let middle = component_ref::<Editor>();
        global::<IndexEditor>().set(IndexEditor(Some(middle)));
        let data = signal().with_untracked(|r| r.data.clone());
        let mut panel = view()
            .test_id("git-index-review")
            .w_full()
            .max_w(px(950.0))
            .h_full()
            .max_h(px(580.0))
            .flex_col()
            .rounded(px(8.0))
            .overflow_hidden()
            .border_1()
            .border_color(p.border_strong)
            .bg(p.surface)
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .px(px(12.0))
                    .h(px(38.0))
                    .gap(px(5.0))
                    .bg(p.rail_bg)
                    .child(
                        text(
                            file.split_once("::")
                                .map_or(file.as_str(), |(_, path)| path)
                                .to_owned(),
                        )
                        .flex_1()
                        .text_color(p.text_primary)
                        .number_of_lines(1u32),
                    )
                    .child(loaded.then(|| {
                        button("取消暂存当前块", "index-take-head", || accept(true))
                    }))
                    .child(loaded.then(|| {
                        button("暂存当前块", "index-take-worktree", || accept(false))
                    }))
                    .child(loaded.then(|| button("保存暂存版本", "index-save", save_active)))
                    .child(button("关闭", "index-close", state::close_active_modal)),
            )
            .child(dynamic(move || {
                let (issue, dirty, saving) =
                    signal().with(|r| (r.issue.clone(), dirty(r), r.saving));
                view()
                    .flex_row()
                    .gap(px(7.0))
                    .px(px(10.0))
                    .child(
                        issue
                            .as_ref()
                            .map(|err| text(err.clone()).text_color(p.danger).text_size(px(11.0))),
                    )
                    .child(saving.then(|| text("保存中…").text_color(p.text_muted)))
                    .child((issue.is_some() && dirty && !saving).then(|| {
                        button("放弃当前编辑并关闭", "index-discard", || {
                            signal().update(|r| {
                                if let Some(data) = &r.data {
                                    r.draft = data.texts[1].clone();
                                }
                            });
                            state::close_active_modal();
                        })
                    }))
            }));
        if let Some(data) = data {
            let editable = data.index.is_some();
            let row = view().flex_1().flex_row().children(
                ["HEAD", "暂存版本", "本地版本"]
                    .into_iter()
                    .enumerate()
                    .map(|(column, label)| {
                        let id = [
                            "index-review-head",
                            "index-review-middle",
                            "index-review-worktree",
                        ][column];
                        let ed = editor()
                            .styles(super::review_editor_styles)
                            .test_id(id)
                            .flex_1()
                            .default_value(Some(data.texts[column].clone().into()))
                            .font_size(move || px(app_signal().with(|s| s.cfg.editor_font_size)))
                            .read_only(move || {
                                column != 1 || !editable || signal().with(|r| r.saving)
                            })
                            .auto_focus(column == 1 && editable)
                            .on_change(move |value: &SharedString| {
                                if column == 1 {
                                    signal().update(|r| r.draft = value.to_string());
                                }
                            });
                        let ed = if column == 1 {
                            ed.component_ref(&middle)
                        } else {
                            ed
                        };
                        view()
                            .flex_1()
                            .flex_col()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(p.border)
                            .child(view().h(px(24.0)).px(px(10.0)).bg(p.surface2).child(
                                text(label).text_color(p.text_secondary).text_size(px(11.5)),
                            ))
                            .child(ed)
                    }),
            );
            panel = panel.child(row);
        } else {
            panel = panel.child(dynamic(move || {
                let loading = signal().with(|r| r.issue.is_none());
                view().child(loading.then(|| text("正在读取文件版本…").text_color(p.text_muted)))
            }));
        }
        view()
            .absolute()
            .inset_0()
            .p(px(12.0))
            .bg(rgba(0x00000070))
            .items_center()
            .justify_center()
            .child(panel)
            .into_element()
    })
}
