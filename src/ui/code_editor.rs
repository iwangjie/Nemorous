use nsh_base::{
    Editor, EditorApi, EditorCommand, EditorContextMenuEvent, EditorSearch, EditorSearchResult,
    EditorSelection, editor, input,
};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::state::{
    self, FocusZone, app_signal, current_palette, editor_refs_signal, open_local_history_for_active,
    save_active_code_tab_now,
};
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

pub fn code_editor_pane() -> impl IntoElement {
    let ed_ref = component_ref::<Editor>();
    let split = computed(|| app_signal().with(|s| s.code_split.is_some()));
    let editor_options = computed(|| app_signal().with(|s| {
        (s.cfg.editor_soft_wrap, s.cfg.editor_font_size, s.cfg.format_on_save)
    }));
    editor_refs_signal().update(|r| r.code = Some(ed_ref));

    // 仅在活动标签 ID 或外部内容版本变化时同步 Editor，避免光标/打字抖动
    let tab_sync_memo = computed(|| {
        app_signal().with(|s| {
            s.code_tabs
                .get(s.active_code_tab_idx)
                .map(|t| (t.id.clone(), t.content_rev))
        })
    });
    let block_memo = computed(|| {
        app_signal().with(|s| {
            s.code_tabs.get(s.active_code_tab_idx).and_then(|t| {
                if t.too_large {
                    Some("文件超过 2 MiB 审阅上限，已只读拦截，避免卡顿")
                } else if t.binary {
                    Some("二进制或非 UTF-8 文件，无法以文本编辑")
                } else {
                    None
                }
            })
        })
    });
    let last_sync = stored(None::<(String, u64)>);
    effect(move || {
        if split.get() {
            last_sync.replace(None);
            return;
        }
        let Some((id, revision)) = tab_sync_memo.get() else { return };
        let old = last_sync.with(|last| last.clone());
        if old.as_ref().is_some_and(|last| last.0 == id && last.1 == revision) {
            return;
        }
        let info = app_signal().with_untracked(|s| s.code_tabs.get(s.active_code_tab_idx)
            .map(|tab| (tab.text.clone(), tab.cursor, tab.scroll_x, tab.scroll_y)));
        if let Some((content, cursor, x, y)) = info
            && let Some(api) = ed_ref.api::<EditorApi>()
        {
            let new_tab = old.as_ref().is_none_or(|last| last.0 != id);
            last_sync.replace(Some((id, revision)));
            let changed = api.value() != content;
            if changed {
                api.set_value(content);
            }
            if new_tab || changed {
                api.set_selections(vec![EditorSelection::caret(cursor)]);
                if x == 0.0 && y == 0.0 {
                    api.reveal_position(cursor, true);
                } else {
                    api.set_scroll_offset(point(px(x), px(y)));
                }
            }
            if new_tab && app_signal().with_untracked(|s| s.focus_zone == FocusZone::CodeEditor)
                && !state::text_input_focus_signal().with_untracked(|f| f.focused)
            {
                api.focus();
            }
        }
    });


    view()
        .test_id("code-editor-pane")
        .flex_1()
        .flex_col()
        .overflow_hidden()
        .bg(move || current_palette().surface)
        // 顶栏
        .child(dynamic(move || {
            let p = current_palette();
            let tab = app_signal().with(|s| s.code_tabs.get(s.active_code_tab_idx).cloned());
            let Some(tab) = tab else {
                return view().h(px(0.0)).into_element();
            };
            let is_dirty = tab.is_dirty();
            let show_opts = app_signal().with(|s| s.show_code_options);
            let wf = tab.file.clone();
            let line_1 = tab.cursor.line + 1;

            view()
                .h(px(38.0))
                .px(px(12.0))
                .flex_row()
                .items_center()
                .justify_between()
                .bg(p.rail_bg)
                .border_b_1()
                .border_color(p.border)
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .overflow_hidden()
                        .child(
                            view()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(p.accent_soft)
                                .child(
                                    text(wf.repo_name.clone())
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.accent_soft_text),
                                ),
                        )
                        .child(
                            view()
                                .cursor(Cursor::Pointer)
                                .on_context_menu({
                                    let f = wf.clone();
                                    move |e: &ContextMenuEvent| {
                                        let items = build_location_menu_items(
                                            f.repo_id.clone(),
                                            f.repo_path.clone(),
                                            f.rel_path.clone(),
                                            f.abs_path.clone(),
                                            line_1,
                                            false,
                                        );
                                        crate::state::show_popup_menu_at(items, e.page_position);
                                    }
                                })
                                .child(
                                    text(wf.rel_path.clone())
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_primary)
                                        .number_of_lines(1u32),
                                ),
                        )
                        .child(is_dirty.then(|| {
                            view()
                                .test_id("code-dirty-badge")
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(p.amber_soft)
                                .child(
                                    text("未保存")
                                        .text_size(px(10.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.amber),
                                )
                        }))
                        .child((!tab.editable).then(|| {
                            view()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(p.surface2)
                                .child(
                                    text(
                                        tab.readonly_reason
                                            .clone()
                                            .unwrap_or_else(|| "只读".to_string()),
                                    )
                                    .text_size(px(10.0))
                                    .text_color(p.text_muted),
                                )
                        })),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(is_dirty.then(|| {
                            view()
                                .test_id("code-save-btn")
                                .h(px(24.0))
                                .px(px(10.0))
                                .rounded(px(5.0))
                                .bg(p.accent)
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .hover(move |s| s.bg(p.accent_hover))
                                .on_press(|| {
                                    save_active_code_tab_now();
                                })
                                .child(
                                    text("保存")
                                        .text_size(px(11.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(p.accent_text),
                                )
                        }))
                        .child((!split.get()).then(|| {
                            view()
                                .test_id("code-split-right-btn")
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(5.0))
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .hover(move |s| s.bg(p.surface_hover))
                                .label("向右拆分")
                                .on_press(crate::code_split_runtime::split_active)
                                .child(icons::icon(ICO_COLUMNS, 13.0, p.text_control))
                        }))
                        .child(
                            view()
                                .test_id("code-git-history-btn")
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(5.0))
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .hover(move |s| s.bg(p.surface_hover))
                                .label("查看 Git 文件历史")
                                .on_press(move || super::revision_picker::show_history(wf.clone()))
                                .child(icons::icon(ICO_BRANCH, 13.0, p.text_control)),
                        )
                        .child(
                            view()
                                .test_id("code-history-btn")
                                .h(px(24.0))
                                .px(px(8.0))
                                .rounded(px(5.0))
                                .bg(p.surface2)
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .hover(move |s| s.bg(p.surface_hover))
                                .on_press(open_local_history_for_active)
                                .child(text("历史").text_size(px(11.0)).text_color(p.text_control)),
                        )
                        .child(
                            view()
                                .w(px(24.0))
                                .h(px(24.0))
                                .rounded(px(5.0))
                                .bg(if show_opts { p.accent_soft } else { transparent_black() })
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .hover(move |s| s.bg(p.surface_hover))
                                .label("显示选项")
                                .on_press(|| {
                                    app_signal().update(|s| s.show_code_options = !s.show_code_options);
                                })
                                .child(icons::icon(ICO_MORE, 13.0, p.text_control)),
                        ),
                )
                .into_element()
        }))
        // 显示选项折叠栏
        .child(when(
            move || app_signal().with(|s| s.show_code_options),
            move || {
                dynamic(move || {
                    let p = current_palette();
                    let (wrap, size, format_on_save) = editor_options.get();
                    view()
                        .h(px(32.0))
                        .px(px(12.0))
                        .gap(px(8.0))
                        .flex_row()
                        .items_center()
                        .bg(p.rail_bg)
                        .border_b_1()
                        .border_color(p.border)
                        .child(text("显示选项：").text_size(px(11.0)).text_color(p.text_muted))
                        .child(
                            view()
                                .test_id("code-format-on-save")
                                .h(px(22.0))
                                .px(px(8.0))
                                .rounded(px(4.0))
                                .bg(if format_on_save { p.accent_soft } else { p.surface2 })
                                .items_center()
                                .cursor(Cursor::Pointer)
                                .on_press(|| {
                                    app_signal().update(|s| s.cfg.format_on_save = !s.cfg.format_on_save);
                                    state::save_config_quiet();
                                })
                                .child(text("保存时格式化").text_size(px(11.0)).text_color(p.text_control)),
                        )
                        .child(
                            input()
                                .test_id("code-format-command")
                                .h(px(23.0))
                                .flex_1()
                                .px(px(6.0))
                                .rounded(px(4.0))
                                .bg(p.surface)
                                .text_size(px(11.0))
                                .text_color(p.text_primary)
                                .placeholder("格式化命令（留空按语言选择）")
                                .value(|| app_signal().with(|s| s.cfg.format_command.clone()))
                                .on_change(|value: &str| {
                                    app_signal().update(|s| s.cfg.format_command = value.to_string());
                                })
                                .on_focus(|| state::set_text_input_focused(true))
                                .on_blur(|| {
                                    state::set_text_input_focused(false);
                                    state::save_config_quiet();
                                })
                                .on_submit(|_: &str| state::save_config_quiet()),
                        )
                        .child(
                            view()
                                .h(px(22.0))
                                .px(px(8.0))
                                .rounded(px(4.0))
                                .bg(p.surface2)
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .on_press(|| {
                                    app_signal().update(|s| {
                                        s.cfg.editor_soft_wrap = !s.cfg.editor_soft_wrap;
                                    });
                                    state::save_config_quiet();
                                })
                                .child(
                                    text(if wrap { "自动换行：开" } else { "自动换行：关" })
                                        .text_size(px(11.0))
                                        .text_color(p.text_control),
                                ),
                        )
                        .child(
                            view()
                                .h(px(22.0))
                                .px(px(8.0))
                                .rounded(px(4.0))
                                .bg(p.surface2)
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .on_press(|| {
                                    app_signal().update(|s| {
                                        s.cfg.editor_font_size += 1.0;
                                        if s.cfg.editor_font_size > 17.0 {
                                            s.cfg.editor_font_size = 12.0;
                                        }
                                    });
                                    state::save_config_quiet();
                                })
                                .child(
                                    text(format!("字号：{:.0}pt", size))
                                        .text_size(px(11.0))
                                        .text_color(p.text_control),
                                ),
                        )
                })
            },
        ))
        // 查找与替换条（保持输入框实例稳定，避免打字重建丢失焦点！）
        .child(when(
            move || app_signal().with(|s| s.code_find_open),
            move || {
                let replace_open_memo = computed(|| app_signal().with(|s| s.code_replace_open));
                view()
                    .test_id("code-find-bar")
                    .px(px(12.0))
                    .py(px(6.0))
                    .gap(px(6.0))
                    .flex_col()
                    .bg(move || current_palette().rail_bg)
                    .border_b_1()
                    .border_color(move || current_palette().border)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                input()
                                    .test_id("code-find-input")
                                    .w(px(220.0))
                                    .h(px(24.0))
                                    .px(px(8.0))
                                    .rounded(px(4.0))
                                    .bg(move || current_palette().surface)
                                    .border_1()
                                    .border_color(move || current_palette().border)
                                    .text_size(px(11.5))
                                    .text_color(move || current_palette().text_primary)
                                    .placeholder("查找…")
                                    .value(move || app_signal().with(|s| s.code_find_query.clone()))
                                    .on_focus(|| state::set_text_input_focused(true))
                                    .on_blur(|| state::set_text_input_focused(false))
                                    .on_change(|v: &str| {
                                        app_signal().update(|s| s.code_find_query = v.to_string());
                                    })
                                    .on_submit(move |_: &str| {
                                        if let Some(api) = state::active_editor_api(FocusZone::CodeEditor) {
                                            api.run(EditorCommand::FindNext);
                                        }
                                    }),
                            )
                            .child(
                                text(move || {
                                    let matches = app_signal().with(|s| s.code_find_matches);
                                    if matches.0 == 0 {
                                        "0 个结果".to_string()
                                    } else if let Some(cur) = matches.1 {
                                        format!("{}/{}", cur + 1, matches.0)
                                    } else {
                                        format!("{} 个结果", matches.0)
                                    }
                                })
                                .text_size(px(11.0))
                                .text_color(move || current_palette().text_muted),
                            )
                            .child(
                                view()
                                    .test_id("code-find-prev")
                                    .h(px(22.0))
                                    .px(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(move || current_palette().surface2)
                                    .items_center()
                                    .justify_center()
                                    .cursor(Cursor::Pointer)
                                    .on_press(move || {
                                        if let Some(api) = state::active_editor_api(FocusZone::CodeEditor) {
                                            api.run(EditorCommand::FindPrevious);
                                        }
                                    })
                                    .child(
                                        text("上一个")
                                            .text_size(px(10.5))
                                            .text_color(move || current_palette().text_control),
                                    ),
                            )
                            .child(
                                view()
                                    .test_id("code-find-next")
                                    .h(px(22.0))
                                    .px(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(move || current_palette().surface2)
                                    .items_center()
                                    .justify_center()
                                    .cursor(Cursor::Pointer)
                                    .on_press(move || {
                                        if let Some(api) = state::active_editor_api(FocusZone::CodeEditor) {
                                            api.run(EditorCommand::FindNext);
                                        }
                                    })
                                    .child(
                                        text("下一个")
                                            .text_size(px(10.5))
                                            .text_color(move || current_palette().text_control),
                                    ),
                            )
                            .child(
                                view()
                                    .test_id("code-find-toggle-replace")
                                    .h(px(22.0))
                                    .px(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(move || {
                                        if replace_open_memo.get() {
                                            current_palette().accent_soft
                                        } else {
                                            current_palette().surface2
                                        }
                                    })
                                    .items_center()
                                    .justify_center()
                                    .cursor(Cursor::Pointer)
                                    .on_press(|| {
                                        app_signal().update(|s| s.code_replace_open = !s.code_replace_open);
                                    })
                                    .child(
                                        text("替换")
                                            .text_size(px(10.5))
                                            .text_color(move || current_palette().text_control),
                                    ),
                            )
                            .child(view().flex_1())
                            .child(
                                view()
                                    .w(px(20.0))
                                    .h(px(20.0))
                                    .items_center()
                                    .justify_center()
                                    .cursor(Cursor::Pointer)
                                    .on_press(|| {
                                        state::set_text_input_focused(false);
                                        app_signal().update(|s| {
                                            s.code_find_open = false;
                                            s.code_replace_open = false;
                                        });
                                    })
                                    .child(icons::icon(ICO_CLOSE, 10.0, current_palette().text_muted)),
                            ),
                    )
                    .child(when(
                        move || replace_open_memo.get(),
                        move || {
                            view()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    input()
                                        .test_id("code-replace-input")
                                        .w(px(220.0))
                                        .h(px(24.0))
                                        .px(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(move || current_palette().surface)
                                        .border_1()
                                        .border_color(move || current_palette().border)
                                        .text_size(px(11.5))
                                        .text_color(move || current_palette().text_primary)
                                        .placeholder("替换为…")
                                        .value(move || app_signal().with(|s| s.code_replace_text.clone()))
                                        .on_focus(|| state::set_text_input_focused(true))
                                        .on_blur(|| state::set_text_input_focused(false))
                                        .on_change(|v: &str| {
                                            app_signal().update(|s| s.code_replace_text = v.to_string());
                                        }),
                                )
                                .child(
                                    view()
                                        .test_id("code-replace-one")
                                        .h(px(22.0))
                                        .px(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(move || current_palette().surface2)
                                        .items_center()
                                        .justify_center()
                                        .cursor(Cursor::Pointer)
                                        .on_press(move || {
                                            let repl = app_signal().with_untracked(|s| s.code_replace_text.clone());
                                            if let Some(api) = state::active_editor_api(FocusZone::CodeEditor) {
                                                api.replace(repl);
                                            }
                                        })
                                        .child(
                                            text("替换")
                                                .text_size(px(10.5))
                                                .text_color(move || current_palette().text_control),
                                        ),
                                )
                                .child(
                                    view()
                                        .test_id("code-replace-all")
                                        .h(px(22.0))
                                        .px(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(move || current_palette().accent_soft)
                                        .items_center()
                                        .justify_center()
                                        .cursor(Cursor::Pointer)
                                        .on_press(move || {
                                            let repl = app_signal().with_untracked(|s| s.code_replace_text.clone());
                                            if let Some(api) = state::active_editor_api(FocusZone::CodeEditor) {
                                                api.replace_all(repl);
                                            }
                                        })
                                        .child(
                                            text("全部替换")
                                                .text_size(px(10.5))
                                                .text_color(move || current_palette().accent_soft_text),
                                        ),
                                )
                        },
                    ))
            },
        ))
        // 主编辑器或拦截提示（仅在拦截态变化时切换，保持 Editor 稳定）
        .child(dynamic(move || {
            let p = current_palette();
            if split.get() {
                return super::split_code_view::split_code_view().into_element();
            }
            let block = block_memo.get();
            if let Some(msg) = block {
                return view()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child(text(msg).text_size(px(12.5)).text_color(p.text_muted))
                    .into_element();
            }

            let initial = app_signal()
                .with_untracked(|s| {
                    s.code_tabs
                        .get(s.active_code_tab_idx)
                        .map(|t| t.text.clone())
                        .unwrap_or_default()
                });

            let auto_focus = app_signal().with_untracked(|s| s.focus_zone == FocusZone::CodeEditor);
            super::blame_view::editor_body(editor()
                .styles(super::review_editor_styles)
                .component_ref(&ed_ref)
                .test_id("code-editor-view")
                .flex_1()
                .auto_focus(auto_focus)
                .default_value(Some(SharedString::from(initial)))
                .language(move || {
                    app_signal().with(|s| {
                        s.code_tabs
                            .get(s.active_code_tab_idx)
                            .map(|t| SharedString::from(t.file.rel_path.clone()))
                    })
                })
                .read_only(move || {
                    app_signal().with(|s| {
                        s.code_tabs
                            .get(s.active_code_tab_idx)
                            .map(|t| !t.editable)
                            .unwrap_or(true)
                    })
                })
                .soft_wrap(move || app_signal().with(|s| s.cfg.editor_soft_wrap))
                .font_size(move || px(app_signal().with(|s| s.cfg.editor_font_size)))
                .search(move || {
                    app_signal().with(|s| {
                        if s.code_find_open && !s.code_find_query.is_empty() {
                            Some(EditorSearch::new(s.code_find_query.clone()))
                        } else {
                            None
                        }
                    })
                })
                .decorations(move || crate::ui::bookmarks::editor_decorations())
                .on_focus(|| {
                    state::set_text_input_focused(false);
                    app_signal().update(|s| s.focus_zone = FocusZone::CodeEditor);
                    state::request_shortcut_rebind();
                })
                .on_change(|value: &SharedString| {
                    let id = app_signal().with_untracked(|s| s.code_tabs.get(s.active_code_tab_idx)
                        .map(|tab| tab.id.clone()));
                    if let Some(id) = id {
                        crate::code_split_runtime::record_editor_change(&id, value.as_ref());
                    }
                })
                .on_selection_change(|sels: &Vec<EditorSelection>| {
                    if let Some(last) = sels.last() {
                        let head = last.head;
                        app_signal().update(|s| {
                            if let Some(tab) = s.code_tabs.get_mut(s.active_code_tab_idx) {
                                tab.cursor = head;
                            }
                        });
                    }
                })
                .on_search_change(|res: &EditorSearchResult| {
                    let next = (res.count, res.current);
                    if app_signal().with_untracked(|s| s.code_find_matches != next) {
                        app_signal().update(|s| s.code_find_matches = next);
                    }
                })
                .on_text_press(|_: &nsh_base::EditorTextPressEvent| super::blame_view::note_text_press())
                .on_context_menu(move |ev: &EditorContextMenuEvent| {
                    let info = app_signal().with_untracked(|s| {
                        s.code_tabs.get(s.active_code_tab_idx).map(|t| t.file.clone())
                    });
                    if let Some(f) = info {
                        let mut items = vec![
                            MenuItem::new("复制").on_select(state::copy_for_current_focus),
                            MenuItem::new("文件结构…").on_select(crate::ui::editor_tools::open_structure),
                            MenuItem::new("书签…").on_select(crate::ui::bookmarks::open_modal),
                            MenuItem::new("重新格式化代码").on_select(crate::format_runtime::format_active),
                            MenuItem::new("与剪贴板比较").on_select(super::comparison::open_clipboard),
                            MenuItem::new("回滚选中块").on_select(super::git_review::rollback_code),
                            MenuItem::new("保存时自动格式化")
                                .checked(app_signal().with_untracked(|s| s.cfg.format_on_save))
                                .on_select(|| {
                                    app_signal().update(|s| s.cfg.format_on_save = !s.cfg.format_on_save);
                                    state::save_config_quiet();
                                }),
                            MenuItem::separator(),
                        ];
                        let history = f.clone();
                        let revision = f.clone();
                        items.push(MenuItem::submenu("Git", [
                            MenuItem::new("查看文件历史").on_select(move || super::revision_picker::show_history(history.clone())),
                            MenuItem::new("与修订版本比较…").on_select(move || super::revision_picker::open(revision.clone())),
                        ]));
                        items.extend(super::blame_view::menu_items(ev.position.line + 1));
                        let breakpoint_file = f.clone();
                        let line = ev.position.line + 1;
                        items.push(MenuItem::new("切换断点").on_select(move || super::debug_panel::toggle_breakpoint(&breakpoint_file,line)));
                        items.push(super::language_tools::menu());
                        items.push(MenuItem::separator());
                        items.extend(build_location_menu_items(
                            f.repo_id,
                            f.repo_path,
                            f.rel_path,
                            f.abs_path,
                            ev.position.line + 1,
                            false,
                        ));
                        crate::state::show_popup_menu_at(items, point(ev.x, ev.y));
                    }
                })
                .into_element(), ed_ref, None).into_element()
        }))
}
