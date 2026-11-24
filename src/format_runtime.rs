use std::path::Path;

use nsh_core::prelude::*;

use crate::formatter::format_source;
use crate::git::ReviewVersion;
use crate::state::{self, AppState, EditorTabKind, RootOwnerHolder, app_signal};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Code(String),
    WorkingDiff(String),
}

#[derive(Clone, PartialEq, Eq)]
struct Source {
    target: Target,
    repo: String,
    relative: String,
    version: ReviewVersion,
    content_revision: u64,
    text: String,
}

impl Target {
    fn source(&self, app: &AppState) -> Option<Source> {
        match self {
            Self::Code(id) => {
                let tab = app
                    .code_tabs
                    .iter()
                    .find(|tab| &tab.id == id && tab.editor_editable() && tab.revision.is_empty())?;
                Some(Source {
                    target: self.clone(),
                    repo: tab.file.repo_path.clone(),
                    relative: tab.file.rel_path.clone(),
                    version: tab.version.clone(),
                    content_revision: tab.content_rev,
                    text: tab.text.clone(),
                })
            }
            Self::WorkingDiff(id) => {
                let diff = app.working_diff.as_ref().filter(|diff| {
                    diff.file.key() == *id && diff.editor_editable()
                })?;
                Some(Source {
                    target: self.clone(),
                    repo: diff.file.repo_path.clone(),
                    relative: diff.file.path.clone(),
                    version: diff.data.version.clone(),
                    content_revision: diff.content_rev,
                    text: diff.current_new_text.clone(),
                })
            }
        }
    }
}

#[derive(Clone, Default)]
struct Formatting {
    active: bool,
}

pub(crate) fn working_target() -> Option<Target> {
    app_signal().with_untracked(|s| {
        let diff = s.working_diff.as_ref()?;
        Some(Target::WorkingDiff(diff.file.key()))
    })
}

pub(crate) fn format_active() {
    let target = app_signal().with_untracked(|s| {
        if s.editor_tab != EditorTabKind::File {
            return None;
        }
        s.code_tabs
            .get(s.active_code_tab_idx)
            .map(|tab| Target::Code(tab.id.clone()))
    });
    if let Some(target) = target {
        start(vec![target], true, || {});
    } else {
        state::show_toast("warn", "先打开一个代码文件");
    }
}

/// An enabled request owns the save and its continuation even when the worker is busy.
pub(crate) fn save_if_enabled(targets: Vec<Target>, after: impl FnOnce() + 'static) -> bool {
    if !app_signal().with_untracked(|s| s.cfg.format_on_save) {
        return false;
    }
    start(targets, false, after);
    true
}

fn start(targets: Vec<Target>, manual: bool, after: impl FnOnce() + 'static) {
    let job = global::<Formatting>();
    if job.with_untracked(|s| s.active) {
        state::show_toast("info", "正在格式化，请稍候");
        return;
    }
    let (workspace, command, sources) = app_signal().with_untracked(|s| {
        let sources = targets
            .iter()
            .map(|target| target.source(s))
            .collect::<Option<Vec<_>>>();
        (
            s.cfg.workspace_path.clone(),
            s.cfg.format_command.clone(),
            sources,
        )
    });
    let Some(sources) = sources else {
        state::show_toast("warn", "当前文件为只读，无法格式化");
        return;
    };
    let Some(root) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    job.update(|s| s.active = true);
    root.run(|| {
        spawn(async move {
            let mut ok = true;
            for source in sources {
                if !format_and_save(&workspace, &command, source, manual).await {
                    ok = false;
                    break;
                }
            }
            job.update(|s| s.active = false);
            if ok {
                after();
            }
        })
    });
}

async fn format_and_save(workspace: &str, command: &str, source: Source, manual: bool) -> bool {
    let unchanged = || {
        app_signal().with_untracked(|s| {
            s.cfg.workspace_path == workspace && source.target.source(s).as_ref() == Some(&source)
        })
    };
    if !unchanged() {
        return false;
    }
    let (repo, relative, text, command) = (
        source.repo.clone(),
        source.relative.clone(),
        source.text.clone(),
        command.to_string(),
    );
    let result =
        blocking(move || format_source(Path::new(&repo), &relative, &command, &text)).await;
    if !unchanged() {
        state::show_toast("warn", "格式化期间文件或缓冲区已变化");
        return false;
    }

    let mut failure = None;
    let formatted = match result {
        Ok(Ok(text)) => text.filter(|text| text != &source.text),
        Ok(Err(err)) => {
            failure = Some(format!("格式化失败：{err:#}"));
            None
        }
        Err(err) => {
            failure = Some(format!("格式化任务中断：{err}"));
            None
        }
    };
    if manual && formatted.is_none() {
        state::show_toast(
            "info",
            failure.unwrap_or_else(|| "文件已格式化或没有可用的格式化器".into()),
        );
        return false;
    }
    if let Some(text) = &formatted {
        match &source.target {
            Target::Code(id) => crate::code_split_runtime::record_editor_change(id, text),
            Target::WorkingDiff(key) => state::edit_working_diff_text(key, text),
        }
    }
    let saved = match &source.target {
        Target::Code(id) => state::save_code_tab_by_id(id),
        Target::WorkingDiff(_) => state::save_working_diff_now(),
    };
    if saved {
        if let Some(failure) = failure {
            state::show_toast("warn", format!("已保存。{failure}"));
        } else if formatted.is_some() {
            state::show_toast("ok", format!("已格式化 {}", source.relative));
        }
    }
    saved
}
