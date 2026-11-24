mod blame;
mod code_split_runtime;
mod commit_message;
mod conflict_text;
mod conflict_files;
mod editor_split;
pub mod config;
pub mod diff_model;
mod file_revisions;
mod format_runtime;
mod formatter;
pub mod git;
mod git_branches;
mod git_commit;
mod git_credentials;
mod git_review_actions;
mod git_rebase_plan;
mod git_remotes;
mod git_repo_admin;
mod git_worktrees;
mod git_history_actions;
mod git_index;
mod git_index_transaction;
mod git_log_query;
mod keymap;
#[cfg(target_os = "macos")]
pub mod macos_gestures;
pub mod local_history;
mod local_history_legacy;
mod merge_tasks;
mod merge_ai;
mod history_restore;
mod history_runtime;
pub mod paths;
pub mod replace_in_files;
pub mod search;
mod selected_changes;
pub mod session;
mod session_runtime;
pub mod state;
pub mod theme;
mod tool_command;
pub mod ui;
pub mod watcher;
mod update_manifest;
mod update_runtime;
mod workbench_github;
mod workbench_lsp;
mod workbench_mani;
mod workbench_process;
mod workbench_protocol;
#[cfg(unix)]
mod workbench_terminal_unix;
#[cfg(windows)]
mod workbench_terminal_windows;
mod workbench_run;
mod workspace_ops;

use nsh_core::prelude::*;

#[native_module("nemorous/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

#[component]
pub fn App() -> impl IntoElement {
    ui::shell()
}
