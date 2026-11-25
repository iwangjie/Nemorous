use nsh_base::{Input, input};
use nsh_core::element::View;
use nsh_core::prelude::*;

/// Frame for a single-line action. Content is centered on both axes even
/// when the caller gives the control a compact height or an explicit width.
pub(super) fn button() -> View {
    view().flex_row().items_center().justify_center()
}

/// Keep the native single-line field centered within a symmetric content box.
/// Form callers retain their compact heights, IME, and editing modes.
pub(super) fn text_field(height: f32, size: f32, inset: f32) -> Input {
    input()
        .h(px(height))
        .px(px(inset))
        .py(px(0.0))
        .text_size(px(size))
        .line_height(px((size * 1.32).round()))
}
