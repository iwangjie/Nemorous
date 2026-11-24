use std::path::Path;
use std::sync::Arc;

use nsh_core::prelude::*;

use crate::file_revisions::{self, Revision};
use crate::git::WorkspaceFile;
use crate::state::{self, EditorTabKind, FocusZone, RootOwnerHolder, app_signal, current_palette};

#[derive(Clone, Default)]
struct Picker {
    workspace: String,
    file: Option<WorkspaceFile>,
    entries: Arc<Vec<Revision>>,
    selected: usize,
    generation: u64,
    loading: bool,
    error: Option<String>,
}

fn signal() -> Signal<Picker> {
    global::<Picker>()
}

pub fn active_working_file() -> Option<WorkspaceFile> {
    app_signal().with_untracked(|s| {
        if s.focus_zone == FocusZone::ProjectTree {
            return s.project_selected_file.clone();
        }
        if s.focus_zone == FocusZone::GitFiles {
            let (commit, file) = s
                .selected_commit
                .as_ref()
                .zip(s.selected_commit_file.as_ref())?;
            return s
                .workspace_files
                .iter()
                .find(|path| path.repo_id == commit.repo_id && path.rel_path == file.path)
                .cloned();
        }
        match s.editor_tab {
            EditorTabKind::File => s
                .code_tabs
                .get(s.active_code_tab_idx)
                .map(|tab| tab.file.clone()),
            EditorTabKind::WorkingDiff => {
                let diff = s.working_diff.as_ref()?;
                s.workspace_files
                    .iter()
                    .find(|file| file.key() == diff.file.key())
                    .cloned()
            }
            _ => s.change_selection.as_ref().and_then(|selection| {
                s.workspace_files
                    .iter()
                    .find(|file| file.key() == selection.file.key())
                    .cloned()
            }),
        }
    })
}

pub fn open_active() {
    if let Some(file) = active_working_file() {
        open(file);
    } else {
        state::show_toast("info", "请先选中一个工作区文件");
    }
}

pub fn open(file: WorkspaceFile) {
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let Some(root) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let generation = signal().with_untracked(|s| s.generation.wrapping_add(1));
    signal().set(Picker {
        workspace: workspace.clone(),
        file: Some(file.clone()),
        generation,
        loading: true,
        ..Picker::default()
    });
    app_signal().update(state::enter_modal_focus);
    root.run(|| {
        spawn(async move {
            let result = blocking(move || {
                file_revisions::list(Path::new(&file.repo_path), "HEAD", &file.rel_path, 60)
            })
            .await;
            if !app_signal().with_untracked(|s| s.cfg.workspace_path == workspace) {
                return;
            }
            signal().update(|s| {
                if s.generation != generation {
                    return;
                }
                s.loading = false;
                match result {
                    Ok(Ok(entries)) => {
                        s.entries =
                            Arc::new(entries.into_iter().filter(|entry| entry.exists).collect());
                    }
                    Ok(Err(err)) => s.error = Some(format!("读取文件修订失败：{err:#}")),
                    Err(err) => s.error = Some(format!("读取任务中断：{err}")),
                }
            });
        })
    });
}

pub fn show_history(file: WorkspaceFile) {
    let repo = app_signal()
        .with_untracked(|s| s.repos.iter().find(|repo| repo.id == file.repo_id).cloned());
    if let Some(repo) = repo {
        state::open_file_revision_history(repo, "HEAD".into(), file.rel_path);
    }
}

pub fn show_active_history() {
    if let Some(file) = active_working_file() {
        show_history(file);
    } else {
        state::show_toast("info", "请先选中一个工作区文件");
    }
}

pub fn is_open() -> bool {
    signal().with(|s| s.file.is_some())
}

pub fn close_active() -> bool {
    let mut changed = false;
    signal().update(|s| {
        if s.file.is_some() {
            changed = true;
            *s = Picker {
                generation: s.generation.wrapping_add(1),
                ..Picker::default()
            };
        }
    });
    changed
}

pub fn move_selection(delta: isize) {
    signal().update(|s| {
        if !s.entries.is_empty() {
            s.selected = s
                .selected
                .saturating_add_signed(delta)
                .min(s.entries.len() - 1);
        }
    });
}

pub fn accept() {
    let selected = signal().with_untracked(|s| s.selected);
    choose(selected);
}

fn choose(index: usize) {
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let target = signal().with_untracked(|s| {
        if s.workspace != workspace {
            return None;
        }
        Some((s.file.clone()?, s.entries.get(index)?.clone()))
    });
    if let Some((file, revision)) = target {
        state::close_active_modal();
        super::comparison::open_revision(file, revision);
    }
}

pub fn modal_layer() -> impl IntoElement {
    let open = computed(is_open);
    dynamic(move || {
        if open.get() {
            modal().into_element()
        } else {
            view().into_element()
        }
    })
}

fn modal() -> impl IntoElement {
    let p = current_palette();
    let focus = element_ref();
    after_answer(move || focus.focus());
    view()
        .test_id("file-revision-picker")
        .absolute()
        .inset_0()
        .p(px(14.0))
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .element_ref(&focus)
                .focusable(true)
                .w_full()
                .max_w(px(660.0))
                .h_full()
                .max_h(px(420.0))
                .rounded(px(12.0))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .flex_col()
                .overflow_hidden()
                .child(
                    view()
                        .h(px(42.0))
                        .px(px(14.0))
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            text("文件修订版本")
                                .text_size(px(13.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_primary),
                        )
                        .child(
                            text("关闭")
                                .text_size(px(11.0))
                                .text_color(p.text_control)
                                .on_press(state::close_active_modal),
                        ),
                )
                .child(dynamic(revisions)),
        )
}

fn revisions() -> impl IntoElement {
    let p = current_palette();
    let (error, loading, entries, selected) =
        signal().with(|s| (s.error.clone(), s.loading, s.entries.clone(), s.selected));
    let status = if loading {
        Some("正在读取历史…".to_string())
    } else if entries.is_empty() {
        Some(error.unwrap_or_else(|| "没有可比较的修订版本".to_string()))
    } else {
        None
    };
    scroll().flex_1().child(
        view()
            .flex_col()
            .py(px(4.0))
            .children(status.map(|message| {
                view()
                    .px(px(14.0))
                    .py(px(10.0))
                    .child(text(message).text_size(px(12.0)).text_color(p.text_muted))
            }))
            .children(entries.iter().enumerate().map(|(index, entry)| {
                view()
                    .test_id(&format!("file-revision-{}", entry.short_hash))
                    .h(px(42.0))
                    .px(px(14.0))
                    .flex_col()
                    .justify_center()
                    .cursor(Cursor::Pointer)
                    .bg(if index == selected {
                        p.accent_soft
                    } else {
                        p.surface
                    })
                    .hover(move |style| style.bg(p.surface_hover))
                    .on_press(move |event: &PressEvent| {
                        if event.clicks >= 2 {
                            choose(index);
                        } else {
                            signal().update(|s| s.selected = index);
                        }
                    })
                    .child(
                        text(entry.subject.clone())
                            .number_of_lines(1u32)
                            .text_size(px(12.0))
                            .text_color(p.text_primary),
                    )
                    .child(
                        text(format!(
                            "{} · {} · {} · {}",
                            entry.short_hash,
                            entry.author,
                            super::git_pane::local_author_date(&entry.date),
                            entry.path,
                        ))
                        .number_of_lines(1u32)
                        .text_size(px(10.5))
                        .text_color(p.text_muted),
                    )
            })),
    )
}
