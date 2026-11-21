use nsh_core::element::desktop::{MenuItem, popup_menu};
use nsh_core::prelude::*;

use crate::state::{self, LeftTab, app_signal, current_palette};
use crate::theme::Palette;

use super::icons::{self, ICO_MINUS, ICO_MORE_VERTICAL};

#[derive(Clone, Copy)]
pub(super) struct ToolWindow {
    tab: LeftTab,
    hovered: Signal<bool>,
    menu_open: Signal<bool>,
    pub(super) width: Signal<f32>,
}

impl ToolWindow {
    pub(super) fn new(tab: LeftTab) -> Self {
        let initial_width = app_signal().with_untracked(|s| match tab {
            LeftTab::Commit => s.cfg.commit_left_width,
            LeftTab::Project => s.cfg.code_left_width,
        });
        Self {
            tab,
            hovered: signal(false),
            menu_open: signal(false),
            width: signal(initial_width),
        }
    }

    pub(super) fn panel(self, id: &'static str, content: impl IntoElement) -> impl IntoElement {
        view()
            .test_id(id)
            .flex_1()
            .flex_col()
            .overflow_hidden()
            .bg(move || current_palette().rail_bg)
            .on_mouse_enter(move |_: &MouseEvent| self.hovered.set(true))
            .on_mouse_leave(move |_: &MouseEvent| self.hovered.set(false))
            .on_layout(move |bounds: &Bounds<Points>| {
                let width = bounds.size.width.0;
                if width > 0.0 && width != self.width.get_untracked() {
                    self.width.set(width);
                }
            })
            .child(content)
    }

    pub(super) fn header(
        self,
        caption: impl IntoElement,
        extra: impl Fn() -> Option<AnyElement> + 'static,
        menu: impl Fn() -> Vec<MenuItem> + 'static,
    ) -> impl IntoElement {
        let (title, actions, options, hide, name) = match self.tab {
            LeftTab::Commit => (
                "changes-header-title-row",
                "changes-title-actions",
                "changes-title-options",
                "changes-hide",
                "提交",
            ),
            LeftTab::Project => (
                "project-header-title-row",
                "project-title-actions",
                "project-options-button",
                "project-hide",
                "文件树",
            ),
        };
        let menu = stored(menu);
        let extra = stored(extra);
        view()
            .test_id(title)
            .h(px(32.0))
            .w_full()
            .px(px(6.0))
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .overflow_hidden()
            .border_b_1()
            .border_color(move || current_palette().border)
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(caption),
            )
            .child(when(
                move || {
                    self.hovered.get()
                        || self.menu_open.get()
                        || app_signal().with(|s| {
                            s.show_left_panel
                                && s.left_tab == self.tab
                                && s.focus_zone == super::tool_window_focus(self.tab)
                        })
                },
                move || {
                    dynamic(move || {
                        let p = current_palette();
                        let open = self.menu_open.get();
                        view()
                            .test_id(actions)
                            .flex_row()
                            .flex_none()
                            .items_center()
                            .gap(px(2.0))
                            .child(extra.with(|render| render()))
                            .child(button(
                                options,
                                "工具窗口选项",
                                ICO_MORE_VERTICAL,
                                p,
                                open,
                                true,
                                move || {
                                    if self.menu_open.get_untracked() {
                                        return;
                                    }
                                    let items = menu.with(|make| make());
                                    self.menu_open.set(true);
                                    state::spawn_from_root(async move {
                                        popup_menu(items).await;
                                        self.menu_open.try_set(false);
                                    });
                                },
                            ))
                            .child(button(
                                hide,
                                format!("隐藏{name}"),
                                ICO_MINUS,
                                p,
                                false,
                                true,
                                move || {
                                    self.hovered.set(false);
                                    super::hide_tool_window(self.tab);
                                },
                            ))
                            .into_element()
                    })
                },
            ))
    }
}

pub(super) fn button(
    id: &'static str,
    label: impl Into<String>,
    icon: &'static str,
    p: Palette,
    active: bool,
    enabled: bool,
    action: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let color = if !enabled {
        p.text_muted.opacity(0.45)
    } else if active {
        p.accent_soft_text
    } else {
        p.text_control
    };
    view()
        .test_id(id)
        .w(px(22.0))
        .h(px(22.0))
        .flex_none()
        .rounded(px(4.0))
        .items_center()
        .justify_center()
        .bg(if active {
            p.accent_soft
        } else {
            transparent_black()
        })
        .cursor(if enabled {
            Cursor::Pointer
        } else {
            Cursor::Default
        })
        .hover(move |s| if enabled { s.bg(p.surface_hover) } else { s })
        .label(label.clone())
        .tooltip(super::tooltip_label(label, p))
        .on_press(move || {
            if enabled {
                action();
            }
        })
        .child(icons::icon(icon, 13.0, color))
}
