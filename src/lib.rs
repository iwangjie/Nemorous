mod code_split_runtime;
mod editor_split;
pub mod config;
pub mod diff_model;
mod file_revisions;
mod format_runtime;
mod formatter;
pub mod git;
mod keymap;
pub mod local_history;
mod local_history_legacy;
mod history_restore;
mod history_runtime;
pub mod paths;
pub mod replace_in_files;
pub mod search;
pub mod session;
mod session_runtime;
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
