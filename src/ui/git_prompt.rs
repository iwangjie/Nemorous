use std::rc::Rc;

use anyhow::Result;
use nsh_base::input;
use nsh_core::prelude::*;

use crate::state::{self, app_signal, current_palette};

type Submit = Rc<dyn Fn(&str) -> Result<()>>;

#[derive(Clone)]
struct Prompt {
    title: String,
    initial: Option<String>,
    submit: Submit,
}

#[derive(Default)]
struct GitPrompt(Option<Prompt>);

pub(super) fn open(
    title: impl Into<String>,
    initial: String,
    submit: impl Fn(&str) -> Result<()> + 'static,
) {
    global::<GitPrompt>().set(GitPrompt(Some(Prompt {
        title: title.into(),
        initial: Some(initial),
        submit: Rc::new(submit),
    })));
    app_signal().update(state::enter_modal_focus);
}

pub(super) fn confirm(title: impl Into<String>, action: impl Fn() + 'static) {
    global::<GitPrompt>().set(GitPrompt(Some(Prompt {
        title: title.into(),
        initial: None,
        submit: Rc::new(move |_| {
            action();
            Ok(())
        }),
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

fn submit(prompt: &Prompt, value: &str) {
    match (prompt.submit)(value.trim()) {
        Ok(()) => state::close_active_modal(),
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
        let input_prompt = prompt.clone();
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
                        input()
                            .test_id("git-text-prompt-input")
                            .h(px(30.0))
                            .px(px(8.0))
                            .bg(p.surface2)
                            .rounded(px(4.0))
                            .text_color(p.text_primary)
                            .auto_focus(true)
                            .value(move || value.get())
                            .on_change(move |new: &str| value.set(new.to_owned()))
                            .on_submit(move |entered: &str| submit(&input_prompt, entered))
                    }))
                    .child(
                        view()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                view()
                                    .label("取消")
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(p.surface2)
                                    .cursor(Cursor::Pointer)
                                    .on_press(state::close_active_modal)
                                    .child(text("取消").text_color(p.text_primary)),
                            )
                            .child(
                                view()
                                    .test_id("git-text-prompt-submit")
                                    .label("确定")
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(p.accent)
                                    .cursor(Cursor::Pointer)
                                    .on_press(move || submit(&prompt, &value.get()))
                                    .child(text("确定").text_color(p.accent_text)),
                            ),
                    ),
            )
            .into_element()
    })
}
