use anyhow::{Context, ensure};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;
use serde_json::{Value, json};

use super::font_zoom::FontArea;
use super::workbench::button;
use crate::config::Config;
use crate::state::{self, app_signal, current_palette};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Appearance,
    General,
    Updates,
}

#[derive(Clone, Default)]
struct Settings {
    open: bool,
    page: Page,
}

fn settings() -> Signal<Settings> {
    global::<Settings>()
}

pub(crate) fn open() {
    settings().set(Settings {
        open: true,
        page: Page::Appearance,
    });
    app_signal().update(state::enter_modal_focus);
}

pub(crate) fn close_active() -> bool {
    let open = settings().with_untracked(|settings| settings.open);
    if open {
        settings().set(Settings::default());
    }
    open
}

fn change(change: impl FnOnce(&mut Config)) {
    app_signal().update(|s| change(&mut s.cfg));
    state::save_config_quiet();
}

pub(super) fn zoom(delta: f32) {
    change(|config| {
        let value = if delta == 0.0 {
            1.0
        } else {
            ((config.zoom() + delta) * 100.0).round() / 100.0
        };
        config
            .extra
            .insert("zoom".into(), json!(value.clamp(0.75, 2.0)));
    });
}

fn page(value: Page) {
    settings().update(|settings| settings.page = value);
}

fn open_keymap() {
    state::close_active_modal();
    super::keymap_settings::open();
}

fn reopen(page: Page) {
    settings().set(Settings { open: true, page });
    app_signal().update(state::enter_modal_focus);
}

fn row_shell(
    title: &'static str,
    id: &'static str,
    action: impl Fn() + 'static,
    right: impl IntoElement,
) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(id)
        .label(title)
        .w_full()
        .min_h(px(36.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .border_b_1()
        .border_color(p.border)
        .cursor(Cursor::Pointer)
        .on_press(action)
        .hover(move |style| style.bg(p.surface_hover))
        .child(
            text(title)
                .flex_1()
                .text_color(p.text_primary)
                .text_size(px(12.5)),
        )
        .child(right)
}

fn setting_row(
    title: &'static str,
    id: &'static str,
    read: impl Fn() -> String + 'static,
    action: impl Fn() + 'static,
) -> impl IntoElement {
    let p = current_palette();
    row_shell(
        title,
        id,
        action,
        text(read).text_color(p.text_secondary).text_size(px(12.0)),
    )
}

fn switch(on: impl Fn() -> bool + 'static) -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let on = on();
        view()
            .w(px(30.0))
            .h(px(17.0))
            .flex_none()
            .rounded(px(9.0))
            .p(px(2.0))
            .flex_row()
            .items_center()
            .bg(if on { p.accent } else { p.surface3 })
            .when(on, |track| track.justify_end())
            .child(
                view()
                    .w(px(13.0))
                    .h(px(13.0))
                    .rounded(px(7.0))
                    .bg(if on { p.accent_text } else { p.text_muted }),
            )
            .into_element()
    })
}

pub(super) fn section(title: &'static str, rows: Vec<AnyElement>) -> AnyElement {
    let p = current_palette();
    view()
        .w_full()
        .flex_col()
        .mb(px(18.0))
        .child(
            text(title)
                .text_color(p.text_muted)
                .text_size(px(11.0))
                .px(px(8.0))
                .pb(px(5.0)),
        )
        .children(rows)
        .into_element()
}

fn toggle(
    label: &'static str,
    id: &'static str,
    read: impl Fn(&Config) -> bool + Copy + 'static,
    write: impl Fn(&mut Config, bool) + 'static,
) -> impl IntoElement {
    row_shell(
        label,
        id,
        move || {
            change(|cfg| {
                let new = !read(cfg);
                write(cfg, new);
            })
        },
        switch(move || app_signal().with(|s| read(&s.cfg))),
    )
}

pub(super) fn extra_toggle(
    label: &'static str,
    key: &'static str,
    default: bool,
) -> impl IntoElement {
    toggle(
        label,
        key,
        move |cfg| cfg.extra_bool(key, default),
        move |cfg, value| {
            cfg.extra.insert(key.into(), json!(value));
        },
    )
}

pub(super) fn extra_text(
    label: &'static str,
    key: &'static str,
    default: &'static str,
) -> impl IntoElement {
    setting_row(
        label,
        key,
        move || app_signal().with(|s| s.cfg.extra_str(key, default).to_owned()),
        move || {
            let initial = app_signal().with_untracked(|s| s.cfg.extra_str(key, default).to_owned());
            let page = settings().with_untracked(|settings| settings.page);
            state::close_active_modal();
            super::git_prompt::open(label, initial, move |value| {
                if key == "update_mirror" && !value.trim().is_empty() {
                    crate::update_manifest::validate_https_url(value)?;
                }
                change(|cfg| {
                    cfg.extra.insert(key.into(), json!(value));
                });
                after_answer(move || reopen(page));
                Ok(())
            });
        },
    )
}

pub(super) fn extra_number(
    label: &'static str,
    key: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> impl IntoElement {
    let read = move || {
        app_signal().with(|s| {
            s.cfg
                .extra
                .get(key)
                .and_then(Value::as_u64)
                .unwrap_or(default)
                .to_string()
        })
    };
    setting_row(label, key, read, move || {
        let initial = read();
        let page = settings().with_untracked(|settings| settings.page);
        state::close_active_modal();
        super::git_prompt::open(label, initial, move |value| {
            let n: u64 = value.parse().context("请输入整数")?;
            ensure!((min..=max).contains(&n), "允许范围 {min}–{max}");
            change(|cfg| {
                cfg.extra.insert(key.into(), json!(n));
            });
            after_answer(move || reopen(page));
            Ok(())
        });
    })
}

fn options(
    label: &'static str,
    id: &'static str,
    read: impl Fn() -> String + 'static,
    choices: &'static [(&'static str, &'static str)],
    write: fn(&'static str),
) -> impl IntoElement {
    setting_row(label, id, read, move || {
        state::show_popup_menu(
            choices
                .iter()
                .map(|(key, label)| MenuItem::new(*label).on_select(move || write(key)))
                .collect(),
        )
    })
}

fn appearance() -> impl IntoElement {
    const THEMES: &[(&str, &str)] = &[
        ("light", "浅色"),
        ("dark", "深色"),
        ("darcula", "Darcula"),
        ("system", "跟随系统"),
    ];
    const ICONS: &[(&str, &str)] = &[
        ("aperture", "Aperture"),
        ("orbit", "Orbit"),
        ("prism", "Prism"),
        ("slash", "Slash"),
        ("aero", "Aero"),
    ];
    let p = current_palette();
    view()
        .flex_col()
        .child(section(
            "主题",
            vec![
                options(
                    "主题",
                    "settings-theme",
                    || {
                        app_signal().with(|s| {
                            THEMES
                                .iter()
                                .find(|(value, _)| *value == s.cfg.theme)
                                .map(|(_, label)| *label)
                                .unwrap_or("跟随系统")
                                .into()
                        })
                    },
                    THEMES,
                    |value| state::set_theme_name(value.into()),
                )
                .into_element(),
                options(
                    "应用图标",
                    "settings-icon",
                    || app_signal().with(|s| s.cfg.extra_str("app_icon", "aperture").to_owned()),
                    ICONS,
                    |value| {
                        change(|cfg| {
                            cfg.extra.insert("app_icon".into(), json!(value));
                        })
                    },
                )
                .into_element(),
            ],
        ))
        .child(section(
            "字号与缩放",
            vec![
                stepper_row(
                    "界面缩放",
                    || app_signal().with(|s| format!("{:.0}%", s.cfg.zoom() * 100.0)),
                    ["缩小", "放大", "重置"],
                    ["zoom-decrease", "zoom-increase", "zoom-reset"],
                    [|| zoom(-0.1), || zoom(0.1), || zoom(0.0)],
                    Some("zoom-value"),
                    p,
                ),
                font_row("源码字号", FontArea::Source, ["font-decrease", "font-increase", "font-reset"], p),
                font_row("Diff 字号", FontArea::Diff, ["diff-font-decrease", "diff-font-increase", "diff-font-reset"], p),
                font_row("文件树字号", FontArea::Tree, ["tree-font-decrease", "tree-font-increase", "tree-font-reset"], p),
                font_row("查找结果字号", FontArea::Search, ["search-font-decrease", "search-font-increase", "search-font-reset"], p),
            ],
        ))
        .child(section(
            "编辑器与界面",
            vec![
                extra_toggle("工具按钮文字标签", "nav_show_labels", false).into_element(),
                toggle(
                    "编辑器软换行",
                    "settings-soft-wrap",
                    |cfg| cfg.editor_soft_wrap,
                    |cfg, v| cfg.editor_soft_wrap = v,
                )
                .into_element(),
                toggle(
                    "显示差异空白字符",
                    "settings-whitespace",
                    |cfg| cfg.diff_show_whitespaces,
                    |cfg, v| cfg.diff_show_whitespaces = v,
                )
                .into_element(),
            ],
        ))
}

fn stepper_row(
    label: &'static str,
    value: impl Fn() -> String + 'static,
    names: [&'static str; 3],
    ids: [&'static str; 3],
    actions: [fn(); 3],
    value_id: Option<&'static str>,
    p: crate::theme::Palette,
) -> AnyElement {
    let [n0, n1, n2] = names;
    let [i0, i1, i2] = ids;
    let [a0, a1, a2] = actions;
    let value = text(value).w(px(44.0)).text_right().text_color(p.text_secondary).text_size(px(12.0));
    let value = match value_id {
        Some(id) => value.test_id(id),
        None => value,
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .min_h(px(36.0))
        .px(px(8.0))
        .border_b_1()
        .border_color(p.border)
        .child(text(label).flex_1().text_left().text_color(p.text_primary).text_size(px(12.5)))
        .child(value)
        .child(button(n0, i0, a0))
        .child(button(n1, i1, a1))
        .child(button(n2, i2, a2))
        .into_element()
}

fn font_row(
    label: &'static str,
    area: FontArea,
    [decrease, increase, reset]: [&'static str; 3],
    p: crate::theme::Palette,
) -> AnyElement {
    let [d, i, r] = [decrease, increase, reset];
    let value = text(move || app_signal().with(|s| format!("{}", area.size(&s.cfg))))
        .w(px(44.0))
        .text_right()
        .text_color(p.text_secondary)
        .text_size(px(12.0));
    view()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .min_h(px(36.0))
        .px(px(8.0))
        .border_b_1()
        .border_color(p.border)
        .child(text(label).flex_1().text_left().text_color(p.text_primary).text_size(px(12.5)))
        .child(value)
        .child(button("减小", d, move || area.change(-1.0)))
        .child(button("增大", i, move || area.change(1.0)))
        .child(button("重置", r, move || area.change(0.0)))
        .into_element()
}

fn protected_branches() -> String {
    app_signal().with(|s| {
        s.cfg
            .extra
            .get("protected_branches")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .unwrap_or_else(|| "main, master, test, dev, prod, production".into())
    })
}

fn general() -> impl IntoElement {
    view()
        .flex_col()
        .child(section(
            "工作区",
            vec![
                toggle(
                    "包括工作区根仓库",
                    "include_root_repo",
                    |cfg| cfg.extra_bool("include_root_repo", true),
                    |cfg, v| {
                        cfg.extra.insert("include_root_repo".into(), json!(v));
                        after_answer(|| state::refresh_workspace_async(None));
                    },
                )
                .into_element(),
                toggle(
                    "单击预览差异",
                    "settings-preview",
                    |cfg| cfg.change_diff_preview,
                    |cfg, v| cfg.change_diff_preview = v,
                )
                .into_element(),
                toggle(
                    "项目视图跟随编辑器",
                    "settings-follow",
                    |cfg| cfg.project_follow_editor,
                    |cfg, v| cfg.project_follow_editor = v,
                )
                .into_element(),
                extra_toggle("启用 Mani", "mani_enabled", true).into_element(),
                extra_number("并发仓库数", "concurrency", 8, 1, 32).into_element(),
            ],
        ))
        .child(section(
            "编辑",
            vec![
                toggle(
                    "保存时自动格式化",
                    "settings-format",
                    |cfg| cfg.format_on_save,
                    |cfg, v| cfg.format_on_save = v,
                )
                .into_element(),
                setting_row(
                    "格式化命令",
                    "settings-format-command",
                    || app_signal().with(|s| s.cfg.format_command.clone()),
                    || {
                        let initial = app_signal().with_untracked(|s| s.cfg.format_command.clone());
                        state::close_active_modal();
                        super::git_prompt::open("格式化命令", initial, |command| {
                            change(|cfg| cfg.format_command = command.to_owned());
                            after_answer(|| reopen(Page::General));
                            Ok(())
                        });
                    },
                )
                .into_element(),
                extra_toggle("启用语言服务", "lsp_enabled", false).into_element(),
            ],
        ))
        .child(section(
            "Git 与 AI",
            vec![
                extra_text("AI 命令行", "ai_agent", "auto").into_element(),
                extra_text("默认合并目标", "default_target_branch", "test").into_element(),
                setting_row("保护分支", "protected_branches", protected_branches, || {
                    let value = app_signal().with_untracked(|_| protected_branches());
                    state::close_active_modal();
                    super::git_prompt::open("保护分支，逗号分隔", value, |value| {
                        let branches = value
                            .split([',', '，'])
                            .map(str::trim)
                            .filter(|branch| !branch.is_empty())
                            .collect::<Vec<_>>();
                        change(|cfg| {
                            cfg.extra.insert("protected_branches".into(), json!(branches));
                        });
                        after_answer(|| reopen(Page::General));
                        Ok(())
                    });
                })
                .into_element(),
            ],
        ))
}

fn nav_item(label: &'static str, id: &'static str, target: Option<Page>, action: fn()) -> impl IntoElement {
    dynamic(move || {
        let p = current_palette();
        let active = target.is_some_and(|t| settings().with(|s| s.page) == t);
        view()
            .test_id(id)
            .label(label)
            .w_full()
            .h(px(30.0))
            .flex_none()
            .rounded(px(6.0))
            .px(px(10.0))
            .flex_row()
            .items_center()
            .cursor(Cursor::Pointer)
            .bg(if active { p.accent_soft } else { transparent_black() })
            .hover(move |style| style.bg(if active { p.accent_soft } else { p.surface_hover }))
            .on_press(action)
            .child(text(label).text_size(px(12.5)).text_color(if active {
                p.accent_soft_text
            } else {
                p.text_secondary
            }))
            .into_element()
    })
}

pub(super) fn layer() -> impl IntoElement {
    dynamic(|| {
        if !settings().with(|settings| settings.open) {
            return view().into_element();
        }
        let p = current_palette();
        view()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000078))
            .p(px(14.0))
            .items_center()
            .justify_center()
            .child(
                view()
                    .test_id("settings-modal")
                    .w_full()
                    .h_full()
                    .max_w(px(760.0))
                    .max_h(px(600.0))
                    .rounded(px(9.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .flex_col()
                    .child(
                        view()
                            .flex_row()
                            .flex_none()
                            .h(px(44.0))
                            .px(px(16.0))
                            .items_center()
                            .border_b_1()
                            .border_color(p.border)
                            .child(
                                text("设置")
                                    .flex_1()
                                    .text_color(p.text_primary)
                                    .text_size(px(14.0)),
                            )
                            .child(button("关闭", "settings-close", state::close_active_modal)),
                    )
                    .child(
                        view()
                            .flex_row()
                            .flex_1()
                            .min_h(px(0.0))
                            .child(
                                view()
                                    .w(px(150.0))
                                    .flex_none()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .p(px(10.0))
                                    .border_r_1()
                                    .border_color(p.border)
                                    .bg(p.rail_bg)
                                    .child(nav_item("外观", "settings-appearance", Some(Page::Appearance), || {
                                        page(Page::Appearance)
                                    }))
                                    .child(nav_item("通用", "settings-general", Some(Page::General), || {
                                        page(Page::General)
                                    }))
                                    .child(nav_item("更新", "settings-updates", Some(Page::Updates), || {
                                        page(Page::Updates)
                                    }))
                                    .child(nav_item("键盘映射", "settings-keymap", None, open_keymap)),
                            )
                            .child(
                                scroll().flex_1().child(
                                    view().p(px(16.0)).child(dynamic(|| {
                                        match settings().with(|settings| settings.page) {
                                            Page::Appearance => appearance().into_element(),
                                            Page::General => general().into_element(),
                                            Page::Updates => super::update_view::body().into_element(),
                                        }
                                    })),
                                ),
                            ),
                    ),
            )
            .into_element()
    })
}

#[cfg(target_os = "macos")]
pub(super) fn setup_icon() {
    effect(|| {
        let choice = app_signal().with(|s| s.cfg.extra_str("app_icon", "aperture").to_owned());
        let _ = dark();
        let _ = nsh_core::element::desktop::window_state().map(|w| w.focused);
        dispatch2::DispatchQueue::main().exec_async(move || {
            use nsh_platform_macos::objc2::{AnyThread, MainThreadMarker};
            use nsh_platform_macos::objc2_app_kit::{NSApplication, NSImage};
            use nsh_platform_macos::objc2_foundation::{NSData, NSString, NSUserDefaults};
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let dark = NSUserDefaults::standardUserDefaults()
                .stringForKey(&NSString::from_str("AppleInterfaceStyle"))
                .is_some_and(|style| style.to_string() == "Dark");
            let asset: &[u8] = match (choice.as_str(), dark) {
                ("aero", false) => include_bytes!("../../resources/appicons/aero_light.png"),
                ("aero", true) => include_bytes!("../../resources/appicons/aero_dark.png"),
                ("orbit", false) => include_bytes!("../../resources/appicons/orbit_light.png"),
                ("orbit", true) => include_bytes!("../../resources/appicons/orbit_dark.png"),
                ("prism", false) => include_bytes!("../../resources/appicons/prism_light.png"),
                ("prism", true) => include_bytes!("../../resources/appicons/prism_dark.png"),
                ("slash", false) => include_bytes!("../../resources/appicons/slash_light.png"),
                ("slash", true) => include_bytes!("../../resources/appicons/slash_dark.png"),
                (_, false) => include_bytes!("../../resources/appicons/aperture_light.png"),
                (_, true) => include_bytes!("../../resources/appicons/aperture_dark.png"),
            };
            let data = NSData::with_bytes(asset);
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                unsafe {
                    NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
                }
            }
        });
    });
}
