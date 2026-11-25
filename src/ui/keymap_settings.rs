use nsh_core::prelude::*;

use crate::keymap::{self, ACTIONS, Keymap};
use crate::state::{self, app_signal, current_palette};

#[derive(Clone, Default)]
struct Settings {
    open: bool,
    filter: String,
    selected: Option<&'static str>,
    draft: String,
    message: Option<String>,
}

fn signal() -> Signal<Settings> {
    global::<Settings>()
}

pub fn open() {
    signal().set(Settings {
        open: true,
        ..Settings::default()
    });
    app_signal().update(state::enter_modal_focus);
}

pub fn close_active() -> bool {
    let settings = signal();
    if settings.with_untracked(|s| s.open) {
        settings.set(Settings::default());
        true
    } else {
        false
    }
}

fn select(id: &'static str) {
    let keymap = super::configured_keymap();
    signal().update(|s| {
        s.selected = Some(id);
        s.draft = keymap.keys(id).first().cloned().unwrap_or_default();
        s.message = None;
    });
}

fn save() {
    let Some((id, value)) = signal().with_untracked(|s| Some((s.selected?, s.draft.clone())))
    else {
        return;
    };
    let mut result = Ok(());
    app_signal().update(|s| {
        result = keymap::set_binding(&mut s.cfg, id, &value, apple_keys(), os());
    });
    match result {
        Ok(()) => {
            state::save_config_quiet();
            select(id);
            signal().update(|s| s.message = Some("已保存快捷键".into()));
        }
        Err(error) => signal().update(|s| s.message = Some(error)),
    }
}

fn reset(only_selected: bool) {
    let id = signal().with_untracked(|s| s.selected);
    if only_selected && id.is_none() {
        return;
    }
    app_signal().update(|s| keymap::reset(&mut s.cfg, if only_selected { id } else { None }));
    state::save_config_quiet();
    if let Some(id) = id {
        select(id);
    }
    signal().update(|s| s.message = Some("已恢复默认快捷键".into()));
}

pub fn modal_layer() -> impl IntoElement {
    let open = computed(|| signal().with(|s| s.open));
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
    let keys = computed(super::configured_keymap);
    view()
        .test_id("keymap-settings")
        .absolute()
        .inset_0()
        .p(px(16.0))
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .w_full()
                .max_w(px(840.0))
                .h_full()
                .max_h(px(555.0))
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
                            text("快捷键设置")
                                .text_size(px(14.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_primary),
                        )
                        .child(view().flex_1())
                        .child(button("keymap-reset-all", "全部恢复默认", || {
                            reset(false)
                        }))
                        .child(button("keymap-close", "关闭", state::close_active_modal)),
                )
                .child(
                    view()
                        .flex_1()
                        .flex_row()
                        .overflow_hidden()
                        .child(
                            view()
                                .flex_1()
                                .flex_col()
                                .p(px(10.0))
                                .gap(px(6.0))
                                .border_r_1()
                                .border_color(p.border)
                                .child(
                                    super::controls::text_field(28.0, 12.0, 8.0)
                                        .test_id("keymap-filter")
                                        .auto_focus(true)
                                        .bg(p.rail_bg)
                                        .text_color(p.text_primary)
                                        .placeholder("查找操作…")
                                        .value(|| signal().with(|s| s.filter.clone()))
                                        .on_change(|value: &str| {
                                            signal().update(|s| s.filter = value.to_string())
                                        }),
                                )
                                .child(dynamic(move || rows(keys.get()))),
                        )
                        .child(
                            view()
                                .w(px(280.0))
                                .p(px(12.0))
                                .flex_col()
                                .gap(px(10.0))
                                .child(dynamic(|| editor_panel())),
                        ),
                ),
        )
}

fn rows(keys: Keymap) -> impl IntoElement {
    let p = current_palette();
    let (query, selected) = signal().with(|s| (s.filter.trim().to_lowercase(), s.selected));
    scroll().flex_1().child(
        view().flex_col().children(
            ACTIONS
                .iter()
                .filter(move |action| {
                    query.is_empty()
                        || action.label.to_lowercase().contains(&query)
                        || action.id.to_lowercase().contains(&query)
                        || action.group.contains(&query)
                })
                .map(move |action| {
                    let keys = keys
                        .keys(action.id)
                        .iter()
                        .map(|key| {
                            if key == keymap::DOUBLE_SHIFT {
                                "双击 Shift"
                            } else {
                                key.as_str()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" / ");
                    view()
                        .test_id(&format!("keymap-action-{}", action.id))
                        .h(px(28.0))
                        .px(px(6.0))
                        .flex_row()
                        .gap(px(8.0))
                        .items_center()
                        .cursor(Cursor::Pointer)
                        .bg(if selected == Some(action.id) {
                            p.accent_soft
                        } else {
                            p.surface
                        })
                        .hover(move |style| style.bg(p.surface_hover))
                        .on_press(move || select(action.id))
                        .child(
                            text(action.label)
                                .text_size(px(12.0))
                                .text_color(p.text_primary),
                        )
                        .child(view().flex_1())
                        .child(text(keys).text_size(px(10.5)).text_color(p.text_muted))
                }),
        ),
    )
}

fn editor_panel() -> impl IntoElement {
    let p = current_palette();
    let selected = computed(|| signal().with(|s| s.selected));
    view().flex_col().gap(px(8.0)).child(dynamic(move || {
        if let Some(action) = selected
            .get()
            .and_then(|id| ACTIONS.iter().find(|action| action.id == id))
        {
            view()
                .flex_col()
                .gap(px(8.0))
                .child(
                    text(action.label)
                        .text_size(px(13.0))
                        .text_color(p.text_primary),
                )
                .child(
                    super::controls::text_field(30.0, 12.0, 8.0)
                        .test_id("keymap-draft")
                        .bg(p.rail_bg)
                        .border_1()
                        .border_color(p.border)
                        .rounded(px(4.0))
                        .text_color(p.text_primary)
                        .placeholder("例如 mod+alt+l")
                        .value(|| signal().with(|s| s.draft.clone()))
                        .on_change(|value: &str| {
                            signal().update(|s| {
                                s.draft = value.to_string();
                                s.message = None;
                            })
                        })
                        .on_submit(|_: &str| save()),
                )
                .child(
                    view()
                        .flex_row()
                        .gap(px(6.0))
                        .child(button("keymap-save", "保存快捷键", save))
                        .child(button("keymap-reset", "恢复默认", || reset(true))),
                )
                .child(
                    text(|| signal().with(|s| s.message.clone().unwrap_or_default()))
                        .text_size(px(11.0))
                        .text_color(p.text_control),
                )
                .into_element()
        } else {
            view()
                .child(
                    text("选择要修改的操作")
                        .text_size(px(12.0))
                        .text_color(p.text_muted),
                )
                .into_element()
        }
    }))
}

fn button(id: &'static str, label: &'static str, handler: impl Fn() + 'static) -> impl IntoElement {
    let p = current_palette();
    super::controls::button()
        .test_id(id)
        .h(px(25.0))
        .px(px(7.0))
        .rounded(px(4.0))
        .cursor(Cursor::Pointer)
        .bg(p.surface2)
        .hover(move |style| style.bg(p.surface_hover))
        .on_press(move || handler())
        .child(text(label).text_size(px(11.0)).text_color(p.text_control))
}
