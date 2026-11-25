use nsh_base::{Editor, EditorApi, EditorSearchResult, EditorSelection, editor};
use nsh_base::{h_resizable, resizable_panel};
use nsh_core::prelude::*;

use crate::code_split_runtime;
use crate::editor_split::PaneId;
use crate::state::{self, app_signal, current_palette};

pub fn split_code_view() -> impl IntoElement {
    h_resizable()
        .flex_1()
        .child(resizable_panel().child(split_code_pane(PaneId::Left)))
        .child(resizable_panel().child(split_code_pane(PaneId::Right)))
}

fn split_code_pane(side: PaneId) -> impl IntoElement {
    let ed_ref = component_ref::<Editor>();
    code_split_runtime::register_ref(side, ed_ref);
    let last_sync = stored((String::new(), u64::MAX));
    let suppress = stored(false);
    let bound = stored(String::new());
    let find_result = signal(EditorSearchResult::default());
    let show_find_result = computed(move || app_signal().with(|s| {
        s.code_find_open && s.editor_tab == state::EditorTabKind::File
            && s.code_split.as_ref().is_some_and(|split| split.focused == side)
    }));
    effect(move || {
        if show_find_result.get() {
            let result = find_result.get();
            if app_signal().with_untracked(|s| s.code_find_matches != result) {
                app_signal().update(|s| s.code_find_matches = result);
            }
        }
    });

    // 只以标签身份和内容版本为键；光标/滚动变化不重装编辑器。
    let sync_key = computed(move || code_split_runtime::pane_key(side));

    {
        let ed = ed_ref.clone();
        let last = last_sync.clone();
        let guard = suppress.clone();
        let tab = bound.clone();
        effect(move || {
            let Some((id, version)) = sync_key.get() else {
                return;
            };
            after_answer(move || {
                if last.with(|k| k.0 == id && k.1 == version) {
                    return;
                }
                let Some(snap) = code_split_runtime::pane(side)
                    .filter(|snapshot| snapshot.id == id && snapshot.version == version)
                else {
                    return;
                };
                let Some(api) = ed.api::<EditorApi>() else {
                    return;
                };

                let prev_id = last.with(|k| k.0.clone());
                last.replace((id.clone(), version));
                tab.replace(id.clone());

                let same_tab = prev_id == id;
                let content_changed = api.value() != snap.text;
                if !content_changed && same_tab {
                    // 自己键入回写模型导致的版本变化：保持现状，不触碰选区/滚动。
                    return;
                }

                // 同标签的版本变化来自同伴编辑：set_value 后恢复本编辑器实际选区与滚动。
                // 切标签：采用模型保存的游标与滚动。
                guard.replace(true);
                if content_changed {
                    let keep = if same_tab {
                        let sels = api.selections();
                        if sels.is_empty() {
                            None
                        } else {
                            Some((sels, api.scroll_offset()))
                        }
                    } else {
                        None
                    };
                    api.set_value(snap.text);
                    match keep {
                        Some((sels, off)) => {
                            api.set_selections(sels);
                            api.set_scroll_offset(off);
                        }
                        None => {
                            api.set_selections(vec![EditorSelection::caret(snap.cursor)]);
                            api.set_scroll_offset(point(px(snap.scroll_x), px(snap.scroll_y)));
                        }
                    }
                } else {
                    api.set_selections(vec![EditorSelection::caret(snap.cursor)]);
                    api.set_scroll_offset(point(px(snap.scroll_x), px(snap.scroll_y)));
                }
                guard.replace(false);
            });
        });
    }

    let palette = current_palette();
    let tab_list = computed(move || code_split_runtime::tab_list(side));
    let strip = dynamic(move || {
        let mut strip = view()
            .flex_row()
            .items_center()
            .px_2()
            .py_1()
            .overflow_hidden();

        for (id, title, active) in tab_list.get() {
            let tab_id = id;
            let pid = side;
            let ed = ed_ref.clone();
            let label = text(title).text_size(px(12.0)).text_color(if active {
                palette.text_primary
            } else {
                palette.text_muted
            });
            strip = strip.child(
                view()
                    .flex_row()
                    .items_center()
                    .px_2()
                    .py_1()
                    .rounded(px(5.0))
                    .cursor(Cursor::Pointer)
                    .test_id(&format!(
                        "split-tab-{}-{}",
                        if side == PaneId::Left {
                            "left"
                        } else {
                            "right"
                        },
                        tab_id
                    ))
                    .bg(if active {
                        palette.surface2
                    } else {
                        palette.surface
                    })
                    .child(label)
                    .on_press(move || {
                        code_split_runtime::capture_views();
                        code_split_runtime::select(pid, &tab_id);
                        if let Some(api) = ed.api::<EditorApi>() {
                            api.focus();
                        }
                    }),
            );
        }

        if matches!(side, PaneId::Right) {
            strip = strip.child(view().flex_1()).child(
                super::controls::button()
                    .test_id("split-code-merge")
                    .px_2()
                    .py_1()
                    .rounded(px(5.0))
                    .cursor(Cursor::Pointer)
                    .child(
                        text("合并分栏")
                            .text_size(px(12.0))
                            .text_color(palette.text_muted),
                    )
                    .on_press(code_split_runtime::merge),
            );
        }

        strip
    });

    let initial = code_split_runtime::pane(side)
        .map(|p| p.text)
        .unwrap_or_default();
    let change_guard = suppress.clone();
    let change_tab = bound.clone();
    let selection_guard = suppress.clone();
    let selection_tab = bound.clone();
    let selection_ref = ed_ref.clone();

    view()
        .test_id(match side {
            PaneId::Left => "split-code-left",
            PaneId::Right => "split-code-right",
        })
        .flex_col()
        .flex_1()
        .overflow_hidden()
        .child(strip)
        .child(
            super::blame_view::editor_body(editor()
                .styles(super::review_editor_styles)
                .component_ref(&ed_ref)
                .test_id(match side {
                    PaneId::Left => "split-editor-left",
                    PaneId::Right => "split-editor-right",
                })
                .flex_1()
                .auto_focus(code_split_runtime::pane(side).is_some_and(|pane| pane.active))
                .default_value(Some(SharedString::from(initial)))
                .language(move || {
                    code_split_runtime::file_properties(side)
                        .map(|props| SharedString::from(props.0))
                })
                .read_only(move || {
                    !code_split_runtime::file_properties(side).is_some_and(|props| props.1)
                })
                .soft_wrap(move || app_signal().with(|s| s.cfg.editor_soft_wrap))
                .font_size(move || px(super::font_zoom::FontArea::Source.current()))
                .decorations(move || crate::ui::bookmarks::editor_decorations_for_split(side))
                .search(move || {
                    app_signal().with(|s| {
                        (s.code_find_open && !s.code_search.query.is_empty()).then(|| s.code_search.clone())
                    })
                })
                .on_search_change(move |result: &EditorSearchResult| {
                    if find_result.get_untracked() != *result {
                        find_result.set(result.clone());
                    }
                })
                .on_focus(move || {
                    state::set_text_input_focused(false);
                    code_split_runtime::focus(side);
                })
                .on_change(move |val: &SharedString| {
                    if change_guard.with(|g| *g) {
                        return;
                    }
                    let tab_id = change_tab.with(|t| t.clone());
                    if tab_id.is_empty() {
                        return;
                    }
                    let Some(snap) = code_split_runtime::pane(side) else {
                        return;
                    };
                    if snap.id != tab_id {
                        return;
                    }
                    let incoming = val.to_string();
                    if snap.text == incoming {
                        return;
                    }
                    code_split_runtime::change(side, &tab_id, &incoming);
                })
                .on_selection_change(move |sels: &Vec<EditorSelection>| {
                    if selection_guard.with(|g| *g) {
                        return;
                    }
                    let tab_id = selection_tab.with(|t| t.clone());
                    if tab_id.is_empty() {
                        return;
                    }
                    let Some(last) = sels.last() else {
                        return;
                    };
                    let Some(api) = selection_ref.api::<EditorApi>() else {
                        return;
                    };
                    let off = api.scroll_offset();
                    code_split_runtime::save_view(side, &tab_id, last.head, off.x.0, off.y.0);
                })
                .on_text_press(|_: &nsh_base::EditorTextPressEvent| super::blame_view::note_text_press())
                .on_context_menu(move |ev: &nsh_base::EditorContextMenuEvent| {
                    code_split_runtime::focus(side);
                    super::code_editor::show_context_menu(ev);
                })
                .into_element(), ed_ref, Some(side)),
        )
}
