use std::path::Path;
use std::sync::Arc;

use nsh_base::{Editor, EditorApi, EditorPosition, editor, h_resizable, resizable_panel};
use nsh_core::prelude::*;

use crate::diff_model::{
    InlineHighlightMode, WhitespaceMode, build_side_by_side_decorations, compute_diff,
};
use crate::git::{self, LineEnding, WorkspaceFile};
use crate::paths::basename;
use crate::state::{self, EditorTabKind, FocusZone, RootOwnerHolder, app_signal, current_palette};

#[derive(Clone, PartialEq, Eq)]
struct Source {
    text: String,
    title: String,
    language: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct Pair {
    left: Source,
    right: Source,
}

#[derive(Clone, Default)]
struct ComparisonState {
    current: Option<Arc<Pair>>,
    previous: Option<(EditorTabKind, FocusZone)>,
    whitespace: WhitespaceMode,
    active_change: usize,
    generation: u64,
}

fn signal() -> Signal<ComparisonState> {
    global::<ComparisonState>()
}

fn checked_text(text: String) -> anyhow::Result<String> {
    anyhow::ensure!(
        text.len() <= git::MAX_REVIEW_BYTES,
        "对比内容超过审阅大小上限"
    );
    anyhow::ensure!(!text.as_bytes().contains(&0), "二进制内容无法比较");
    Ok(LineEnding::normalize_to_lf(&text))
}

fn file_title(file: &WorkspaceFile) -> String {
    if file.repo_name.is_empty() {
        file.rel_path.clone()
    } else {
        format!("{} / {}", file.repo_name, file.rel_path)
    }
}

fn dirty_buffer(file: &WorkspaceFile) -> Option<String> {
    app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .find(|tab| tab.file.key() == file.key() && tab.is_dirty())
            .map(|tab| tab.text.clone())
            .or_else(|| {
                s.working_diff
                    .as_ref()
                    .filter(|diff| diff.file.key() == file.key() && diff.is_dirty())
                    .map(|diff| diff.current_new_text.clone())
            })
    })
}

fn file_source(file: WorkspaceFile, live: Option<String>) -> anyhow::Result<Source> {
    let text = if let Some(text) = live {
        text
    } else {
        let worktree = git::read_review_worktree(Path::new(&file.abs_path));
        anyhow::ensure!(worktree.version.exists, "文件不存在：{}", file.rel_path);
        anyhow::ensure!(
            !worktree.binary && !worktree.too_large,
            "无法比较此文件：{}",
            file.rel_path
        );
        worktree.text
    };
    Ok(Source {
        text: checked_text(text)?,
        title: file_title(&file),
        language: Some(file.rel_path),
    })
}

fn queue(task: impl std::future::Future<Output = anyhow::Result<Pair>> + 'static) {
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let Some(root) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let generation = signal().with_untracked(|s| s.generation.wrapping_add(1));
    signal().update(|s| s.generation = generation);
    root.run(|| {
        spawn(async move {
            let result = task.await;
            if !signal().with_untracked(|s| s.generation == generation)
                || !app_signal().with_untracked(|s| s.cfg.workspace_path == workspace)
            {
                return;
            }
            match result {
                Ok(pair) => present(pair),
                Err(err) => state::show_toast("warn", err.to_string()),
            }
        })
    });
}

fn present(pair: Pair) {
    crate::code_split_runtime::capture_views();
    let (previous, whitespace) = app_signal().with_untracked(|s| {
        (
            (s.editor_tab, s.focus_zone),
            WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode),
        )
    });
    signal().update(|s| {
        s.current = Some(Arc::new(pair));
        if previous.0 != EditorTabKind::Comparison {
            s.previous = Some(previous);
        }
        s.whitespace = whitespace;
        s.active_change = 0;
    });
    activate();
}

pub fn open_project_selection() -> bool {
    let pair = app_signal().with_untracked(|s| {
        let [left, right] = s.project_compare_keys.as_slice() else {
            return None;
        };
        let left = s
            .workspace_files
            .iter()
            .find(|file| file.key() == *left)?
            .clone();
        let right = s
            .workspace_files
            .iter()
            .find(|file| file.key() == *right)?
            .clone();
        Some((left, right))
    });
    let Some((left, right)) = pair else {
        return false;
    };
    let left_live = dirty_buffer(&left);
    let right_live = dirty_buffer(&right);
    queue(async move {
        blocking(move || {
            Ok(Pair {
                left: file_source(left, left_live)?,
                right: file_source(right, right_live)?,
            })
        })
        .await?
    });
    true
}

fn revision_source(repo_path: &str, hash: &str, short_hash: &str, path: String) -> anyhow::Result<Source> {
    let spec = format!("{hash}:{path}");
    let (text, binary, too_large) = git::read_git_object_text(Path::new(repo_path), &spec)?;
    anyhow::ensure!(!binary && !too_large, "无法比较此修订版本");
    Ok(Source {
        text: checked_text(text)?,
        title: format!("{short_hash} · {path}"),
        language: Some(path),
    })
}

pub(crate) fn open_revision(file: WorkspaceFile, revision: crate::file_revisions::Revision) {
    let live = dirty_buffer(&file);
    queue(async move {
        blocking(move || Ok(Pair {
            left: revision_source(&file.repo_path, &revision.hash, &revision.short_hash, revision.path)?,
            right: file_source(file, live)?,
        })).await?
    });
}

pub(crate) fn open_historical_file(file: WorkspaceFile, revision: git::CommitEntry, path: String) {
    let live = dirty_buffer(&file);
    queue(async move {
        blocking(move || Ok(Pair {
            left: revision_source(&revision.repo_path, &revision.hash, &revision.short_hash, path)?,
            right: file_source(file, live)?,
        })).await?
    });
}

pub(crate) fn open_two_revisions(
    left: git::CommitEntry,
    left_path: String,
    right: git::CommitEntry,
    right_path: String,
) {
    queue(async move {
        blocking(move || Ok(Pair {
            left: revision_source(&left.repo_path, &left.hash, &left.short_hash, left_path)?,
            right: revision_source(&right.repo_path, &right.hash, &right.short_hash, right_path)?,
        })).await?
    });
}

pub fn open_clipboard() {
    let tab = app_signal().with_untracked(|s| {
        s.code_tabs
            .get(s.active_code_tab_idx)
            .map(|tab| (tab.file.rel_path.clone(), tab.text.clone()))
    });
    let Some((path, full_text)) = tab else { return };
    let selected = state::active_editor_api(FocusZone::CodeEditor)
        .map(|api| api.selected_text())
        .filter(|selected| !selected.is_empty());
    let title = if selected.is_some() {
        format!("{path}（选区）")
    } else {
        path.clone()
    };
    let editor_text = selected.unwrap_or(full_text);
    queue(async move {
        let clipboard = clipboard_string().await;
        anyhow::ensure!(!clipboard.is_empty(), "剪贴板没有文本");
        Ok(Pair {
            left: Source {
                text: checked_text(clipboard)?,
                title: "剪贴板".into(),
                language: None,
            },
            right: Source {
                text: checked_text(editor_text)?,
                title,
                language: Some(path),
            },
        })
    });
}

pub fn title() -> Option<String> {
    signal().with(|s| {
        s.current.as_ref().map(|pair| {
            format!(
                "{} ↔ {}",
                basename(&pair.left.title).trim(),
                basename(&pair.right.title).trim()
            )
        })
    })
}

pub fn activate() {
    app_signal().update(|s| {
        s.editor_tab = EditorTabKind::Comparison;
        s.focus_zone = FocusZone::ComparisonNew;
    });
}

pub fn close() {
    let previous = signal().with_untracked(|s| s.previous);
    signal().update(|s| {
        let generation = s.generation.wrapping_add(1);
        *s = ComparisonState {
            generation,
            ..ComparisonState::default()
        };
    });
    app_signal().update(|s| {
        if s.editor_tab != EditorTabKind::Comparison {
            return;
        }
        let available = previous.filter(|(tab, _)| match tab {
            EditorTabKind::File => !s.code_tabs.is_empty(),
            EditorTabKind::WorkingDiff => s.working_diff.is_some(),
            EditorTabKind::CommitDiff => s.commit_diff.is_some(),
            EditorTabKind::Welcome => true,
            EditorTabKind::Comparison => false,
        });
        (s.editor_tab, s.focus_zone) =
            available.unwrap_or((EditorTabKind::Welcome, FocusZone::CommitTree));
    });
    state::focus_active_editor_ref();
}

pub fn pane() -> impl IntoElement {
    let pair = computed(|| signal().with(|s| s.current.clone()));
    dynamic(move || {
        if let Some(pair) = pair.get() {
            pane_for(pair).into_element()
        } else {
            view().into_element()
        }
    })
}

fn pane_for(pair: Arc<Pair>) -> impl IntoElement {
    let p = current_palette();
    let left_ref = component_ref::<Editor>();
    let right_ref = component_ref::<Editor>();
    state::editor_refs_signal().update(|refs| {
        refs.compare_old = Some(left_ref);
        refs.compare_new = Some(right_ref);
    });
    let mode = computed(|| signal().with(|s| s.whitespace));
    let diff_pair = pair.clone();
    let diff =
        computed(move || compute_diff(&diff_pair.left.text, &diff_pair.right.text, mode.get()));
    let inline = computed(|| {
        app_signal().with(|s| InlineHighlightMode::from_config(&s.cfg.diff_inline_mode))
    });
    let decor_pair = pair.clone();
    let decorations = computed(move || {
        build_side_by_side_decorations(
            &decor_pair.left.text,
            &decor_pair.right.text,
            &diff.get().hunks,
            inline.get(),
            p,
        )
    });

    let move_change = move |delta: isize| {
        let computed = diff.get();
        if computed.hunks.is_empty() {
            return;
        }
        let len = computed.hunks.len() as isize;
        let mut change = 0;
        signal().update(|s| {
            s.active_change = (s.active_change as isize + delta).rem_euclid(len) as usize;
            change = s.active_change;
        });
        let hunk = &computed.hunks[change];
        for (reference, line) in [(left_ref, hunk.old_start), (right_ref, hunk.new_start)] {
            if let Some(api) = reference.api::<EditorApi>() {
                api.reveal_position(EditorPosition::new(line as u32, 0), true);
            }
        }
    };

    let toolbar = view()
        .h(px(35.0))
        .px(px(8.0))
        .gap(px(6.0))
        .flex_row()
        .items_center()
        .child(action_button(
            "comparison-prev-change",
            "上一处",
            move || move_change(-1),
        ))
        .child(action_button(
            "comparison-next-change",
            "下一处",
            move || move_change(1),
        ))
        .child(
            text(move || {
                let count = diff.with(|value| value.hunks.len());
                format!("变更 {}", count)
            })
            .test_id("comparison-change-count")
            .text_size(px(11.0))
            .text_color(p.text_muted),
        )
        .child(view().flex_1())
        .child(dynamic(move || {
            action_button("comparison-whitespace", mode.get().label(), move || {
                let items = [
                    WhitespaceMode::None,
                    WhitespaceMode::Trim,
                    WhitespaceMode::All,
                    WhitespaceMode::AllAndEmptyLines,
                ]
                .into_iter()
                .map(|option| {
                    nsh_core::element::desktop::MenuItem::new(option.label())
                        .checked(mode.get() == option)
                        .on_select(move || {
                            signal().update(|s| {
                                s.whitespace = option;
                                s.active_change = 0;
                            })
                        })
                })
                .collect();
                state::show_popup_menu(items);
            })
        }));

    view()
        .test_id("comparison-pane")
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .child(toolbar)
        .child(
            h_resizable()
                .flex_1()
                .child(resizable_panel().child(compare_side(
                    &pair.left,
                    left_ref,
                    true,
                    move || decorations.with(|both| both.0.clone()),
                )))
                .child(resizable_panel().child(compare_side(
                    &pair.right,
                    right_ref,
                    false,
                    move || decorations.with(|both| both.1.clone()),
                ))),
        )
}

fn action_button(
    id: &'static str,
    label: &'static str,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .h(px(24.0))
        .px(px(6.0))
        .rounded(px(4.0))
        .items_center()
        .cursor(Cursor::Pointer)
        .bg(p.surface2)
        .hover(move |style| style.bg(p.surface_hover))
        .on_press(move || on_press())
        .child(text(label).text_size(px(11.0)).text_color(p.text_control))
}

fn compare_side(
    source: &Source,
    reference: ComponentRef<Editor>,
    old: bool,
    decorations: impl Fn() -> Vec<nsh_base::EditorDecoration> + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_1()
        .flex_col()
        .child(
            view()
                .h(px(26.0))
                .px(px(8.0))
                .flex_row()
                .items_center()
                .bg(p.rail_bg)
                .test_id(if old {
                    "comparison-left-label"
                } else {
                    "comparison-right-label"
                })
                .child(
                    text(source.title.clone())
                        .text_size(px(11.0))
                        .text_color(p.text_control)
                        .number_of_lines(1u32),
                ),
        )
        .child(
            editor()
                .component_ref(&reference)
                .test_id(if old {
                    "comparison-left-editor"
                } else {
                    "comparison-right-editor"
                })
                .flex_1()
                .read_only(true)
                .auto_focus(!old)
                .default_value(Some(SharedString::from(source.text.clone())))
                .language(source.language.clone().map(SharedString::from))
                .decorations(decorations)
                .soft_wrap(|| app_signal().with(|s| s.cfg.editor_soft_wrap))
                .font_size(|| px(app_signal().with(|s| s.cfg.editor_font_size)))
                .on_focus(move || {
                    state::set_text_input_focused(false);
                    app_signal().update(|s| {
                        s.focus_zone = if old {
                            FocusZone::ComparisonOld
                        } else {
                            FocusZone::ComparisonNew
                        }
                    });
                }),
        )
}
