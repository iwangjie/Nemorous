use nsh_base::{
    Editor, EditorApi, EditorDecoration, EditorPosition, editor, h_resizable, resizable_panel,
};
use nsh_core::prelude::*;

use crate::diff_model::{
    InlineHighlightMode, WhitespaceMode, build_side_by_side_decorations, compute_diff,
};
use crate::history_runtime::{self, View as HistoryView};
use crate::state::{self, app_signal, current_palette};

pub fn modal_layer() -> impl IntoElement {
    let open = computed(history_runtime::is_open);
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
    let data = computed(history_runtime::snapshot);
    let selected = signal(0usize);
    let compare = signal(true);
    let texts = computed(move || {
        data.with(|state| {
            state
                .as_ref()
                .map(|state| {
                    (
                        state.title.clone(),
                        state.snapshot_text.clone(),
                        state.current_text.clone(),
                    )
                })
                .unwrap_or_default()
        })
    });
    let diff =
        computed(move || texts.with(|(_, old, new)| compute_diff(old, new, WhitespaceMode::None)));
    let inline = computed(|| {
        app_signal().with(|s| InlineHighlightMode::from_config(&s.cfg.diff_inline_mode))
    });
    let decorations = computed(move || {
        texts.with(|(_, old, new)| {
            diff.with(|diff| build_side_by_side_decorations(old, new, &diff.hunks, inline.get(), p))
        })
    });
    let old_ref = component_ref::<Editor>();
    let new_ref = component_ref::<Editor>();
    let navigate_change = move |delta: isize| {
        let hunk = diff.with(|diff| {
            let index = selected
                .get()
                .saturating_add_signed(delta)
                .min(diff.hunks.len().saturating_sub(1));
            selected.set(index);
            diff.hunks.get(index).cloned()
        });
        if let Some(hunk) = hunk {
            for (reference, line) in [(old_ref, hunk.old_start), (new_ref, hunk.new_start)] {
                if let Some(editor) = reference.api::<EditorApi>() {
                    editor.reveal_position(EditorPosition::new(line as u32, 0), true);
                }
            }
        }
    };

    view()
        .test_id("local-history-modal")
        .absolute()
        .inset_0()
        .p(px(16.0))
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .w_full()
                .max_w(px(960.0))
                .h_full()
                .max_h(px(620.0))
                .rounded(px(12.0))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .flex_col()
                .overflow_hidden()
                .child(
                    view()
                        .h(px(44.0))
                        .px(px(14.0))
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            text(move || {
                                format!("本地历史 · {}", texts.with(|(title, _, _)| title.clone()))
                            })
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_primary)
                            .number_of_lines(1u32),
                        )
                        .child(view().flex_1())
                        .child(button(
                            "local-history-close",
                            "关闭",
                            state::close_active_modal,
                        )),
                )
                .child(
                    view()
                        .flex_1()
                        .flex_row()
                        .overflow_hidden()
                        .child(dynamic(move || snapshot_list(data.get(), selected)))
                        .child(
                            view()
                                .flex_1()
                                .flex_col()
                                .overflow_hidden()
                                .child(dynamic(move || {
                                    let ready = data.with(|state| {
                                        state.as_ref().is_some_and(|state| {
                                            state.selected_id.is_some() && !state.loading
                                        })
                                    });
                                    let changes = diff.with(|diff| diff.hunks.len());
                                    let index = selected.get().min(changes.saturating_sub(1));
                                    view()
                                        .h(px(37.0))
                                        .px(px(8.0))
                                        .gap(px(5.0))
                                        .flex_row()
                                        .items_center()
                                        .child(button(
                                            "local-history-compare",
                                            if compare.get() {
                                                "查看快照"
                                            } else {
                                                "对比当前"
                                            },
                                            move || compare.update(|value| *value = !*value),
                                        ))
                                        .child((ready && compare.get() && changes > 0).then(|| {
                                            view()
                                                .gap(px(4.0))
                                                .flex_row()
                                                .items_center()
                                                .child(button(
                                                    "local-history-hunk-prev",
                                                    "上一处",
                                                    move || navigate_change(-1),
                                                ))
                                                .child(button(
                                                    "local-history-hunk-next",
                                                    "下一处",
                                                    move || navigate_change(1),
                                                ))
                                                .child(
                                                    text(format!("{} / {}", index + 1, changes))
                                                        .text_size(px(10.5))
                                                        .text_color(p.text_muted),
                                                )
                                                .child(button(
                                                    "local-history-restore-hunk",
                                                    "恢复此变更块",
                                                    move || {
                                                        history_runtime::restore_hunk(index);
                                                        selected.set(0);
                                                    },
                                                ))
                                        }))
                                        .child(view().flex_1())
                                        .child(ready.then(|| {
                                            button(
                                                "local-history-restore-all",
                                                "恢复此版本…",
                                                history_runtime::request_restore_all,
                                            )
                                        }))
                                }))
                                .child(when(history_runtime::confirmation, confirmation))
                                .child(dynamic(move || {
                                    let message = data.with(|state| {
                                        state.as_ref().and_then(|state| {
                                            state
                                                .error
                                                .clone()
                                                .or_else(|| state.notice.clone())
                                                .or_else(|| {
                                                    state
                                                        .loading
                                                        .then(|| "正在读取本地历史…".into())
                                                })
                                        })
                                    });
                                    view().px(px(8.0)).child(message.map(|message| {
                                        text(message).text_size(px(11.0)).text_color(p.text_control)
                                    }))
                                }))
                                .child(
                                    h_resizable()
                                        .flex_1()
                                        .child(resizable_panel().child(history_editor(
                                            "local-history-snapshot",
                                            "快照",
                                            old_ref,
                                            move || texts.with(|(_, text, _)| text.clone()),
                                            move || texts.with(|(title, _, _)| title.clone()),
                                            move || {
                                                if compare.get() {
                                                    decorations.with(|pair| pair.0.clone())
                                                } else {
                                                    Vec::new()
                                                }
                                            },
                                        )))
                                        .child(
                                            resizable_panel().visible(move || compare.get()).child(
                                                history_editor(
                                                    "local-history-current",
                                                    "当前内容",
                                                    new_ref,
                                                    move || texts.with(|(_, _, text)| text.clone()),
                                                    move || {
                                                        texts.with(|(title, _, _)| title.clone())
                                                    },
                                                    move || decorations.with(|pair| pair.1.clone()),
                                                ),
                                            ),
                                        ),
                                ),
                        ),
                ),
        )
}

fn snapshot_list(state: Option<HistoryView>, selected: Signal<usize>) -> impl IntoElement {
    let p = current_palette();
    let rows = state.as_ref().into_iter().flat_map(|state| {
        state.entries.iter().map(move |entry| {
            let id = entry.id.clone();
            let stamp = entry
                .created_at
                .get(..19)
                .unwrap_or(entry.created_at.as_str())
                .replace('T', " ");
            view()
                .test_id(&format!("local-history-entry-{id}"))
                .h(px(38.0))
                .px(px(10.0))
                .flex_col()
                .justify_center()
                .cursor(Cursor::Pointer)
                .bg(if state.selected_id.as_deref() == Some(id.as_str()) {
                    p.accent_soft
                } else {
                    p.surface
                })
                .hover(move |style| style.bg(p.surface_hover))
                .on_press(move || {
                    selected.set(0);
                    history_runtime::select(&id);
                })
                .child(
                    text(entry.label.clone())
                        .text_size(px(11.5))
                        .text_color(p.text_primary)
                        .number_of_lines(1u32),
                )
                .child(text(stamp).text_size(px(10.5)).text_color(p.text_muted))
        })
    });
    scroll()
        .test_id("local-history-list")
        .w(px(190.0))
        .border_r_1()
        .border_color(p.border)
        .child(
            view()
                .flex_col()
                .child(
                    state
                        .as_ref()
                        .filter(|s| !s.loading && s.entries.is_empty())
                        .map(|_| {
                            view().p(px(12.0)).child(
                                text("暂无快照")
                                    .text_size(px(11.0))
                                    .text_color(p.text_muted),
                            )
                        }),
                )
                .children(rows),
        )
}

fn confirmation() -> impl IntoElement {
    let p = current_palette();
    view()
        .h(px(36.0))
        .px(px(8.0))
        .gap(px(5.0))
        .flex_row()
        .items_center()
        .bg(p.surface2)
        .child(
            text("覆盖当前文件？")
                .text_size(px(11.0))
                .text_color(p.text_primary),
        )
        .child(view().flex_1())
        .child(button(
            "local-history-cancel-restore",
            "取消",
            history_runtime::cancel_confirmation,
        ))
        .child(button(
            "local-history-confirm-restore",
            "确认恢复",
            history_runtime::confirm_restore,
        ))
}

fn history_editor(
    id: &'static str,
    label: &'static str,
    reference: ComponentRef<Editor>,
    value: impl Fn() -> String + 'static,
    language: impl Fn() -> String + 'static,
    decorations: impl Fn() -> Vec<EditorDecoration> + 'static,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .child(
            view()
                .h(px(24.0))
                .px(px(8.0))
                .items_center()
                .flex_row()
                .bg(p.rail_bg)
                .child(text(label).text_size(px(10.5)).text_color(p.text_muted)),
        )
        .child(
            editor()
                .component_ref(&reference)
                .test_id(id)
                .flex_1()
                .read_only(true)
                .value(move || Some(SharedString::from(value())))
                .language(move || Some(SharedString::from(language())))
                .decorations(decorations)
                .on_focus(move || {
                    state::set_text_input_focused(false);
                    state::editor_refs_signal().update(|refs| refs.modal_active = Some(reference));
                }),
        )
}

fn button(id: &'static str, label: &'static str, handler: impl Fn() + 'static) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .h(px(24.0))
        .px(px(6.0))
        .rounded(px(4.0))
        .items_center()
        .bg(p.surface2)
        .cursor(Cursor::Pointer)
        .hover(move |style| style.bg(p.surface_hover))
        .on_press(move || handler())
        .child(text(label).text_size(px(10.5)).text_color(p.text_control))
}
