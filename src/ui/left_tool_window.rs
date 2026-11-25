use nsh_core::element::desktop::{MenuItem, popup_menu};
use nsh_core::prelude::*;

use crate::state::{self, LeftTab, WorkspaceData, WorkspaceNotice, app_signal, current_palette};
use crate::theme::Palette;

use super::icons::{self, ICO_MINUS, ICO_MORE_VERTICAL};

#[derive(Clone, Default)]
struct TreeFilterRefs {
    project: Option<ElementRef>,
    commit: Option<ElementRef>,
}

pub(super) fn open_tree_filter(tab: LeftTab) {
    app_signal().update(|s| {
        if s.tree_filter_open != Some(tab) {
            s.dismiss_tree_filter();
            s.tree_filter_open = Some(tab);
        }
    });
    after_answer(move || {
        if !app_signal().with_untracked(|s| {
            s.show_left_panel && s.left_tab == tab && s.tree_filter_open == Some(tab)
                && s.focus_zone == super::tool_window_focus(tab)
        }) {
            return;
        }
        let input = global::<TreeFilterRefs>().with_untracked(|refs| match tab {
            LeftTab::Project => refs.project,
            LeftTab::Commit => refs.commit,
        });
        if let Some(input) = input {
            input.focus();
        }
    });
}

pub(super) fn tree_filter(
    tab: LeftTab,
    id: &'static str,
    placeholder: &'static str,
) -> impl IntoElement {
    let input_ref = element_ref();
    global::<TreeFilterRefs>().update(|refs| match tab {
        LeftTab::Project => refs.project = Some(input_ref),
        LeftTab::Commit => refs.commit = Some(input_ref),
    });
    let open = computed(move || app_signal().with(|s| s.tree_filter_open == Some(tab)));
    when(move || open.get(), move || {
        let p = current_palette();
        view()
            .px(px(8.0))
            .py(px(5.0))
            .flex_none()
            .border_b_1()
            .border_color(p.border)
            .child(
                super::controls::text_field(26.0, 11.5, 8.0)
                    .element_ref(&input_ref)
                    .test_id(id)
                    .auto_focus(true)
                    .rounded(px(5.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border)
                    .text_color(p.text_primary)
                    .placeholder(placeholder)
                    .value(move || app_signal().with(|s| match tab {
                        LeftTab::Project => s.project_filter.clone(),
                        LeftTab::Commit => s.changes_filter.clone(),
                    }))
                    .on_focus(move || {
                        app_signal().update(|s| s.focus_zone = super::tool_window_focus(tab));
                        state::set_text_input_focused(true);
                    })
                    .on_blur(|| state::set_text_input_focused(false))
                    .on_change(move |value: &str| {
                        app_signal().update(|s| match tab {
                            LeftTab::Project => value.clone_into(&mut s.project_filter),
                            LeftTab::Commit => value.clone_into(&mut s.changes_filter),
                        });
                    }),
            )
    })
}

pub(super) fn tree_scroll(
    viewport: &'static str,
    rows: &'static str,
    content: impl IntoElement,
) -> impl IntoElement {
    scroll()
        .horizontal()
        .test_id(viewport)
        .flex_1()
        .child(
            scroll()
                .test_id(rows)
                .flex_none()
                .h_full()
                .min_w(relative(1.0))
                .child(content),
        )
}

pub(super) fn workspace_notice(data: WorkspaceData) -> impl IntoElement {
    let notice = computed(move || app_signal().with(|s| s.workspace_notice(data)));
    dynamic(move || {
        let Some(notice) = notice.get() else { return None };
        let p = current_palette();
        let row = view().px(px(8.0)).py(px(6.0)).gap(px(5.0))
            .flex_none().border_b_1().border_color(p.border);
        Some(match notice {
            WorkspaceNotice::Loading(message) => row
                .test_id("workspace-loading")
                .child(text(message).text_size(px(11.0)).text_color(p.text_muted))
                .into_element(),
            WorkspaceNotice::Failed(message) => row
                .test_id("workspace-read-error")
                .child(text(message.clone()).text_size(px(11.0)).text_color(p.text_primary).number_of_lines(2u32))
                .tooltip(super::tooltip_label(message, p).max_w(px(360.0)))
                .child(super::controls::button().test_id("workspace-read-retry").px(px(6.0)).h(px(22.0)).self_start().rounded(px(4.0))
                    .label("重试刷新工作区").cursor(Cursor::Pointer)
                    .hover(move |style| style.bg(p.surface_hover))
                    .on_press(|| state::refresh_workspace_async(None))
                    .child(text("重试").text_size(px(11.0)).text_color(p.accent)))
                .into_element(),
        })
    })
}

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
    super::controls::button()
        .test_id(id)
        .w(px(22.0))
        .h(px(22.0))
        .flex_none()
        .rounded(px(4.0))
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
