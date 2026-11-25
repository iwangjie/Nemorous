use nsh_base::{Editor, editor};
use nsh_core::prelude::*;

use crate::paths::basename;
use crate::state::{
    self, EditorTabKind, FocusZone, NavTarget, PickerKind, accept_disk_version_in_conflict,
    app_signal, current_palette, open_from_file_picker_input, resolve_dirty_close,
    restore_nav_target, submit_goto_line,
};
use crate::theme::Palette;
use crate::ui::icons::{self, *};

fn modal_backdrop(content: impl IntoElement) -> impl IntoElement {
    view()
        .absolute()
        .inset_0()
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(content)
}

fn action_btn(
    label: String,
    primary: bool,
    danger: bool,
    p: Palette,
    tid: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    let (bg, fg) = if primary {
        (p.accent, p.accent_text)
    } else if danger {
        (p.danger, p.accent_text)
    } else {
        (p.surface2, p.text_primary)
    };
    super::controls::button()
        .test_id(tid)
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(6.0))
        .bg(bg)
        .cursor(Cursor::Pointer)
        .hover(move |s| s.opacity(0.9))
        .label(label.clone())
        .on_press(on_click)
        .child(
            text(label)
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(fg),
        )
}

pub fn modals_layer() -> impl IntoElement {
    // 仅在弹窗类型或冲突警告/历史选中项变化时重建弹窗容器，避免输入框打字时丢失焦点！
    let modal_gate = computed(|| {
        app_signal().with(|s| {
            (
                s.save_conflict
                    .as_ref()
                    .map(|c| (c.disk_version.sha256.clone(), c.disk_deleted, c.warning_message.clone())),
                s.dirty_close.as_ref().map(|d| d.dirty_titles.clone()),
                s.goto_line_open,
                s.picker_open,
                s.stash_modal_open,
            )
        })
    });

    dynamic(move || {
        let _gate = modal_gate.get();
        let p = current_palette();
        let (conflict, dirty_close, goto_open, picker, stash_open) =
            app_signal().with_untracked(|s| {
                (
                    s.save_conflict.clone(),
                    s.dirty_close.clone(),
                    s.goto_line_open,
                    s.picker_open,
                    s.stash_modal_open,
                )
            });

        // 1. 保存冲突双栏对比弹窗（最高优先级，保护数据安全）
        if let Some(m) = conflict {
            let disk_ed_ref = component_ref::<Editor>();
            let mine_ed_ref = component_ref::<Editor>();
            let left_title = if m.disk_deleted {
                "磁盘文件已删除"
            } else {
                "磁盘版本"
            };
            let accept_label = if m.disk_deleted {
                "放弃编辑并关闭文件"
            } else {
                "使用磁盘版本"
            };
            let edited_copy = m.edited_text.clone();
            let lang = Some(SharedString::from(m.rel_path.clone()));

            return modal_backdrop(
                view()
                    .test_id("save-conflict-modal")
                    .w(px(860.0))
                    .h(px(520.0))
                    .rounded(px(10.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        view()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(p.border)
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(icons::icon(ICO_ALERT, 16.0, p.amber))
                            .child(
                                text("磁盘文件已变化，编辑仍保留")
                                    .test_id("save-conflict-title")
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary),
                            )
                            .child(
                                text(m.rel_path.clone())
                                    .text_size(px(11.5))
                                    .text_color(p.text_muted),
                            ),
                    )
                    .child(m.warning_message.clone().map(|warn| {
                        view()
                            .test_id("save-conflict-warning")
                            .px(px(16.0))
                            .py(px(6.0))
                            .bg(p.amber_soft)
                            .border_b_1()
                            .border_color(p.border)
                            .child(
                                text(warn)
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.amber),
                            )
                    }))
                    .child(
                        view()
                            .flex_1()
                            .flex_row()
                            .overflow_hidden()
                            .child(
                                view()
                                    .flex_1()
                                    .flex_col()
                                    .border_r_1()
                                    .border_color(p.border)
                                    .child(
                                        view()
                                            .h(px(26.0))
                                            .px(px(10.0))
                                            .bg(p.surface2)
                                            .border_b_1()
                                            .border_color(p.border)
                                            .items_center()
                                            .flex_row()
                                            .child(
                                                text(left_title)
                                                    .text_size(px(11.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(if m.disk_deleted {
                                                        p.danger
                                                    } else {
                                                        p.text_primary
                                                    }),
                                            ),
                                    )
                                    .child(if m.disk_deleted {
                                        view()
                                            .flex_1()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                text("磁盘上的目标文件已被删除，保存已被拦截，不会静默重建")
                                                    .text_size(px(12.0))
                                                    .text_color(p.danger),
                                            )
                                            .into_element()
                                    } else {
                                        editor()
                                            .component_ref(&disk_ed_ref)
                                            .flex_1()
                                            .value(Some(SharedString::from(m.disk_text.clone())))
                                            .language(lang.clone())
                                            .read_only(true)
                                            .on_focus(move || {
                                                state::set_text_input_focused(false);
                                                state::editor_refs_signal()
                                                    .update(|r| r.modal_active = Some(disk_ed_ref));
                                            })
                                            .into_element()
                                    }),
                            )
                            .child(
                                view()
                                    .flex_1()
                                    .flex_col()
                                    .child(
                                        view()
                                            .h(px(26.0))
                                            .px(px(10.0))
                                            .bg(p.surface2)
                                            .border_b_1()
                                            .border_color(p.border)
                                            .items_center()
                                            .flex_row()
                                            .child(
                                                text("你的编辑")
                                                    .text_size(px(11.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(p.text_primary),
                                            ),
                                    )
                                    .child(
                                        editor()
                                            .component_ref(&mine_ed_ref)
                                            .flex_1()
                                            .value(Some(SharedString::from(m.edited_text.clone())))
                                            .language(lang)
                                            .read_only(true)
                                            .on_focus(move || {
                                                state::set_text_input_focused(false);
                                                state::editor_refs_signal()
                                                    .update(|r| r.modal_active = Some(mine_ed_ref));
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        view()
                            .px(px(16.0))
                            .py(px(10.0))
                            .border_t_1()
                            .border_color(p.border)
                            .bg(p.rail_bg)
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(action_btn(
                                "复制我的编辑".to_string(),
                                false,
                                false,
                                p,
                                "conflict-copy-mine-btn",
                                move || {
                                    set_clipboard(edited_copy.clone());
                                    state::show_toast("ok", "已复制你的编辑到剪贴板");
                                },
                            ))
                            .child(
                                view()
                                    .flex_row()
                                    .gap(px(8.0))
                                    .child(action_btn(
                                        accept_label.to_string(),
                                        false,
                                        true,
                                        p,
                                        "conflict-accept-disk-btn",
                                        accept_disk_version_in_conflict,
                                    ))
                                    .child(action_btn(
                                        "继续编辑".to_string(),
                                        true,
                                        false,
                                        p,
                                        "conflict-keep-editing-btn",
                                        || {
                                            app_signal().update(|s| {
                                                s.save_conflict = None;
                                                s.focus_zone = match s.editor_tab {
                                                    EditorTabKind::WorkingDiff => {
                                                        FocusZone::WorkingDiffNew
                                                    }
                                                    _ => FocusZone::CodeEditor,
                                                };
                                            });
                                        },
                                    )),
                            ),
                    ),
            )
            .into_element();
        }

        // 2. 关闭单个 / 多个脏标签保护弹窗
        if let Some(dc) = dirty_close {
            let count = dc.dirty_titles.len();
            let title = if count > 1 {
                format!("有 {count} 个未保存文件")
            } else {
                format!(
                    "“{}”有未保存的更改",
                    dc.dirty_titles.first().cloned().unwrap_or_default()
                )
            };
            let list_text = dc.dirty_titles.join("、");
            return modal_backdrop(
                view()
                    .test_id("dirty-close-modal")
                    .w(px(420.0))
                    .p(px(18.0))
                    .gap(px(12.0))
                    .rounded(px(10.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .flex_col()
                    .child(
                        text(title)
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_primary),
                    )
                    .child(
                        text(format!("关闭前是否保存：{list_text}？"))
                            .text_size(px(12.0))
                            .text_color(p.text_secondary),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(action_btn(
                                "取消".to_string(),
                                false,
                                false,
                                p,
                                "dirty-close-cancel",
                                state::close_active_modal,
                            ))
                            .child(action_btn(
                                if count > 1 { "全部放弃" } else { "放弃更改" }.to_string(),
                                false,
                                true,
                                p,
                                "dirty-close-discard",
                                || resolve_dirty_close(false),
                            ))
                            .child(action_btn(
                                if count > 1 { "保存全部" } else { "保存" }.to_string(),
                                true,
                                false,
                                p,
                                "dirty-close-save",
                                || resolve_dirty_close(true),
                            )),
                    ),
            )
            .into_element();
        }

        // 3. 转到行:列弹窗（Cmd+L）
        if goto_open {
            return modal_backdrop(
                view()
                    .test_id("goto-line-modal")
                    .w(px(340.0))
                    .p(px(14.0))
                    .gap(px(10.0))
                    .rounded(px(10.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .flex_col()
                    .child(
                        text("转到行:列 (⌘L)")
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_primary),
                    )
                    .child(
                        super::controls::text_field(30.0, 12.5, 10.0)
                            .test_id("goto-line-input")
                            .rounded(px(6.0))
                            .bg(p.rail_bg)
                            .border_1()
                            .border_color(p.accent)
                            .text_color(p.text_primary)
                            .placeholder("输入行号或 行:列，例如 12:5")
                            .value(move || app_signal().with(|s| s.goto_line_input.clone()))
                            .on_focus(|| state::set_text_input_focused(true))
                            .on_blur(|| state::set_text_input_focused(false))
                            .on_change(|v: &str| {
                                app_signal().update(|s| s.goto_line_input = v.to_string());
                            })
                            .on_submit(|_: &str| submit_goto_line()),
                    )
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(action_btn(
                                "取消".to_string(),
                                false,
                                false,
                                p,
                                "goto-line-cancel",
                                state::close_active_modal,
                            ))
                            .child(action_btn(
                                "跳转".to_string(),
                                true,
                                false,
                                p,
                                "goto-line-submit",
                                submit_goto_line,
                            )),
                    ),
            )
            .into_element();
        }

        // 4. 转到文件 (Cmd+Shift+O) / 最近位置 (Cmd+Shift+E)
        if let Some(kind) = picker {
            let title = match kind {
                PickerKind::GoToFile => "转到文件 (⌘⇧O)",
                PickerKind::RecentLocations => "最近位置 (⌘⇧E)",
            };
            let list_view = dynamic(move || {
                let p = current_palette();
                let (query, files, recents) = app_signal().with(|s| {
                    (
                        s.picker_query.clone(),
                        s.workspace_files.clone(),
                        s.recent_locations.clone(),
                    )
                });
                let q_low = query.trim().to_ascii_lowercase();
                let mut rows: Vec<AnyElement> = Vec::new();
                match kind {
                    PickerKind::GoToFile => {
                        for wf in files
                            .iter()
                            .filter(|f| {
                                q_low.is_empty()
                                    || f.rel_path.to_ascii_lowercase().contains(&q_low)
                                    || f.repo_name.to_ascii_lowercase().contains(&q_low)
                            })
                            .take(30)
                        {
                            let wf_click = wf.clone();
                            rows.push(
                                view()
                                    .h(px(28.0))
                                    .px(px(10.0))
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .cursor(Cursor::Pointer)
                                    .hover(move |s| s.bg(p.surface_hover))
                                    .on_press(move || {
                                        state::set_text_input_focused(false);
                                        app_signal().update(|s| {
                                            s.picker_open = None;
                                            state::restore_focus_after_modal(s);
                                        });
                                        state::open_code_file(wf_click.clone(), None, true);
                                    })
                                    .child(
                                        text(wf.rel_path.clone())
                                            .text_size(px(12.0))
                                            .text_color(p.text_primary)
                                            .number_of_lines(1u32),
                                    )
                                    .child(
                                        text(wf.repo_name.clone())
                                            .text_size(px(10.5))
                                            .text_color(p.text_muted),
                                    )
                                    .into_element(),
                            );
                        }
                    }
                    PickerKind::RecentLocations => {
                        for nav in recents.into_iter().take(24) {
                            let label = match &nav {
                                NavTarget::CodeFile { rel_path, revision, pos, .. } => {
                                    if revision.is_empty() {
                                        format!("源码 · {}:{}", rel_path, pos.line + 1)
                                    } else {
                                        let short = revision.get(..8).unwrap_or(revision);
                                        format!("仓库版本 · {}:{} ({})", rel_path, pos.line + 1, short)
                                    }
                                }
                                NavTarget::WorkingDiff { file_key, pos, .. } => {
                                    format!("差异 · {}:{}", basename(file_key), pos.line + 1)
                                }
                                NavTarget::CommitDiff { file_path, pos, .. } => {
                                    format!("提交差异 · {}:{}", file_path, pos.line + 1)
                                }
                            };
                            if !q_low.is_empty() && !label.to_ascii_lowercase().contains(&q_low) {
                                continue;
                            }
                            let nav_click = nav.clone();
                            rows.push(
                                view()
                                    .h(px(28.0))
                                    .px(px(10.0))
                                    .flex_row()
                                    .items_center()
                                    .cursor(Cursor::Pointer)
                                    .hover(move |s| s.bg(p.surface_hover))
                                    .on_press(move || {
                                        state::set_text_input_focused(false);
                                        app_signal().update(|s| {
                                            s.picker_open = None;
                                            state::restore_focus_after_modal(s);
                                        });
                                        restore_nav_target(nav_click.clone());
                                    })
                                    .child(
                                        text(label)
                                            .text_size(px(12.0))
                                            .text_color(p.text_primary)
                                            .number_of_lines(1u32),
                                    )
                                    .into_element(),
                            );
                        }
                    }
                }
                view().flex_col().py(px(4.0)).children(rows)
            });

            return modal_backdrop(
                view()
                    .test_id("review-picker-modal")
                    .w(px(520.0))
                    .max_h(px(440.0))
                    .rounded(px(10.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        view()
                            .px(px(12.0))
                            .py(px(10.0))
                            .border_b_1()
                            .border_color(p.border)
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                view()
                                    .flex_row()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        text(title)
                                            .text_size(px(12.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.text_primary),
                                    )
                                    .child(
                                        super::controls::button()
                                            .cursor(Cursor::Pointer)
                                            .on_press(state::close_active_modal)
                                            .child(icons::icon(ICO_CLOSE, 12.0, p.text_muted)),
                                    ),
                            )
                            .child(
                                super::controls::text_field(28.0, 12.0, 8.0)
                                    .test_id("review-picker-input")
                                    .rounded(px(6.0))
                                    .bg(p.rail_bg)
                                    .border_1()
                                    .border_color(p.accent)
                                    .text_color(p.text_primary)
                                    .placeholder(match kind {
                                        PickerKind::GoToFile => "输入路径或 ~/path:line:col 后回车…",
                                        PickerKind::RecentLocations => "筛选最近位置…",
                                    })
                                    .value(move || app_signal().with(|s| s.picker_query.clone()))
                                    .on_focus(|| state::set_text_input_focused(true))
                                    .on_blur(|| state::set_text_input_focused(false))
                                    .on_change(|v: &str| {
                                        app_signal().update(|s| s.picker_query = v.to_string());
                                    })
                                    .on_submit(move |val: &str| {
                                        if kind == PickerKind::GoToFile {
                                            open_from_file_picker_input(val);
                                        }
                                    }),
                            ),
                    )
                    .child(scroll().flex_1().child(list_view)),
            )
            .into_element();
        }

        // 6. Stash 管理弹窗（确保窄栏 More -> Stash -> 管理 Stash… 可达真实弹窗）
        if stash_open {
            let stashes = app_signal().with(|s| s.stashes.clone());
            let mut rows: Vec<AnyElement> = Vec::new();
            if stashes.is_empty() {
                rows.push(
                    view()
                        .p(px(16.0))
                        .child(text("当前工作区暂无 Stash 记录").text_size(px(12.0)).text_color(p.text_muted))
                        .into_element(),
                );
            } else {
                for st in stashes {
                    rows.push(
                        view()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(p.border)
                            .flex_row()
                            .gap(px(8.0))
                            .child(text(st.repo_name).text_size(px(11.0)).text_color(p.accent_soft_text))
                            .child(text(st.selector).text_size(px(11.0)).text_color(p.text_muted))
                            .child(text(st.message).text_size(px(11.5)).text_color(p.text_primary))
                            .into_element(),
                    );
                }
            }
            return modal_backdrop(
                view()
                    .test_id("stash-modal")
                    .w(px(520.0))
                    .max_h(px(400.0))
                    .rounded(px(10.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        view()
                            .px(px(14.0))
                            .py(px(10.0))
                            .border_b_1()
                            .border_color(p.border)
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .child(
                                text("管理 Stash")
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_primary),
                            )
                            .child(
                                super::controls::button()
                                    .cursor(Cursor::Pointer)
                                    .on_press(state::close_active_modal)
                                    .child(icons::icon(ICO_CLOSE, 12.0, p.text_muted)),
                            ),
                    )
                    .child(scroll().flex_1().child(view().flex_col().children(rows))),
            )
            .into_element();
        }

        view().w(px(0.0)).h(px(0.0)).into_element()
    })
}
