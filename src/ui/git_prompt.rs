use std::rc::Rc;

use anyhow::Result;
use nsh_core::prelude::*;

use crate::state::{self, app_signal, current_palette};

type Submit = Rc<dyn Fn(&str, bool) -> Result<()>>;

#[derive(Clone)]
struct Prompt {
    title: String,
    initial: Option<String>,
    option: Option<&'static str>,
    submit_label: &'static str,
    alternate: Option<(&'static str, Rc<dyn Fn()>)>,
    after_close: Option<Rc<dyn Fn()>>,
    submit: Submit,
}

#[derive(Default)]
struct GitPrompt(Option<Prompt>);

pub(super) fn open(
    title: impl Into<String>,
    initial: String,
    submit: impl Fn(&str) -> Result<()> + 'static,
) {
    open_with_option(title, initial, None, move |value, _| submit(value));
}

pub(super) fn open_with_option(
    title: impl Into<String>,
    initial: String,
    option: Option<&'static str>,
    submit: impl Fn(&str, bool) -> Result<()> + 'static,
) {
    global::<GitPrompt>().set(GitPrompt(Some(Prompt {
        title: title.into(),
        initial: Some(initial),
        option,
        submit_label: "确定",
        alternate: None,
        after_close: None,
        submit: Rc::new(submit),
    })));
    app_signal().update(state::enter_modal_focus);
}

pub(super) fn confirm(title: impl Into<String>, action: impl Fn() + 'static) {
    global::<GitPrompt>().set(GitPrompt(Some(Prompt {
        title: title.into(),
        initial: None,
        option: None,
        submit_label: "确定",
        alternate: None,
        after_close: None,
        submit: Rc::new(move |_, _| {
            action();
            Ok(())
        }),
    })));
    app_signal().update(state::enter_modal_focus);
}

pub(super) fn confirm_after_close(title: impl Into<String>, action: impl Fn() + 'static) {
    global::<GitPrompt>().set(GitPrompt(Some(Prompt {
        title: title.into(),
        initial: None,
        option: None,
        submit_label: "确定",
        alternate: None,
        after_close: Some(Rc::new(action)),
        submit: Rc::new(|_, _| Ok(())),
    })));
    app_signal().update(state::enter_modal_focus);
}

pub(super) fn choose(
    title: impl Into<String>,
    submit_label: &'static str,
    submit: impl Fn() -> Result<()> + 'static,
    alternate_label: &'static str,
    alternate: impl Fn() + 'static,
) {
    global::<GitPrompt>().set(GitPrompt(Some(Prompt {
        title: title.into(),
        initial: None,
        option: None,
        submit_label,
        alternate: Some((alternate_label, Rc::new(alternate))),
        after_close: None,
        submit: Rc::new(move |_, _| submit()),
    })));
    app_signal().update(state::enter_modal_focus);
}

pub(crate) fn close_active() -> bool {
    let prompt = global::<GitPrompt>();
    let open = prompt.with_untracked(|p| p.0.is_some());
    if open {
        prompt.set(GitPrompt::default());
    }
    open
}

fn submit(prompt: &Prompt, value: &str, option: bool) {
    match (prompt.submit)(value.trim(), option) {
        Ok(()) => {
            state::close_active_modal();
            if let Some(action) = &prompt.after_close {
                action();
            }
        }
        Err(err) => state::show_toast("warn", err.to_string()),
    }
}

pub(super) fn layer() -> impl IntoElement {
    dynamic(|| {
        let Some(prompt) = global::<GitPrompt>().with(|p| p.0.clone()) else {
            return view().into_element();
        };
        let p = current_palette();
        let value = signal(prompt.initial.clone().unwrap_or_default());
        let option = signal(false);
        let input_prompt = prompt.clone();
        let submit_label = prompt.submit_label;
        view()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000070))
            .items_center()
            .justify_center()
            .child(
                view()
                    .test_id("git-text-prompt")
                    .w(px(380.0))
                    .px(px(16.0))
                    .py(px(14.0))
                    .flex_col()
                    .gap(px(10.0))
                    .rounded(px(8.0))
                    .bg(p.surface)
                    .border_1()
                    .border_color(p.border_strong)
                    .child(
                        text(prompt.title.clone())
                            .text_color(p.text_primary)
                            .text_size(px(13.0)),
                    )
                    .child(prompt.initial.is_some().then(|| {
                        super::controls::text_field(30.0, 12.0, 8.0)
                            .test_id("git-text-prompt-input")
                            .bg(p.surface2)
                            .rounded(px(4.0))
                            .text_color(p.text_primary)
                            .auto_focus(true)
                            .value(move || value.get())
                            .on_change(move |new: &str| value.set(new.to_owned()))
                            .on_submit(move |entered: &str| submit(&input_prompt, entered, option.get_untracked()))
                    }))
                    .child(prompt.option.map(|label| {
                        nsh_base::checkbox()
                            .checked(move || option.get())
                            .accessibility_label(label)
                            .on_change(move |checked: &bool| option.set(*checked))
                            .child(text(label).text_size(px(12.0)).text_color(p.text_primary))
                    }))
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                super::controls::button()
                                    .label("取消")
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(p.surface2)
                                    .cursor(Cursor::Pointer)
                                    .on_press(state::close_active_modal)
                                    .child(text("取消").text_color(p.text_primary)),
                            )
                            .child(prompt.alternate.clone().map(|(label, action)| {
                                super::controls::button()
                                    .h(px(28.0)).px(px(10.0)).rounded(px(4.0))
                                    .bg(p.surface2).cursor(Cursor::Pointer)
                                    .on_press(move || {
                                        state::close_active_modal();
                                        action();
                                    })
                                    .child(text(label).text_color(p.text_primary))
                            }))
                            .child(
                                super::controls::button()
                                    .test_id("git-text-prompt-submit")
                                    .label(submit_label)
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(p.accent)
                                    .cursor(Cursor::Pointer)
                                    .on_press(move || submit(&prompt, &value.get(), option.get_untracked()))
                                    .child(text(submit_label).text_color(p.accent_text)),
                            ),
                    ),
            )
            .into_element()
    })
}
