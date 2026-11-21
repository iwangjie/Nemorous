pub mod config;
pub mod diff_model;
pub mod git;
pub mod local_history;
pub mod paths;
pub mod search;
pub mod state;
pub mod theme;
pub mod ui;
pub mod watcher;

use nsh_core::prelude::*;

#[native_module("nemorous/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

#[component]
pub fn App() -> impl IntoElement {
    ui::shell()
}
