use super::diff_open::{DiffReadRequest, prepare_working_diff};
use super::source_open::reveal_source_after_render;
use super::*;

fn history(s: &AppState, forward: bool) -> &[NavTarget] {
    if forward { &s.nav_forward } else { &s.nav_back }
}

pub(super) fn navigate_history(forward: bool) {
    if app_signal().with_untracked(|s| history(s, forward).is_empty()) {
        cancel_diff_read();
        return;
    }
    Navigation::start(Intent::History(forward), None);
}

pub fn restore_nav_target(nav: NavTarget) {
    Navigation::start(Intent::Recent, Some(nav));
}

pub fn open_revision_code_file(commit: &CommitEntry, rel_path: &str) {
    let id = code_tab_id(&commit.repo_id, rel_path, &commit.hash);
    let (pos, scroll_y) = app_signal().with_untracked(|s| {
        s.code_tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| (tab.cursor, tab.scroll_y.round() as i32))
            .unwrap_or_else(|| (EditorPosition::new(0, 0), 0))
    });
    Navigation::start(
        Intent::OpenRevision,
        Some(NavTarget::CodeFile {
            repo_id: commit.repo_id.clone(),
            rel_path: rel_path.to_owned(),
            revision: commit.hash.clone(),
            pos,
            scroll_y,
        }),
    );
}

#[derive(Clone, Copy)]
enum Intent {
    History(bool),
    Recent,
    OpenRevision,
}

#[derive(Clone)]
struct Navigation {
    read: DiffReadRequest,
    intent: Intent,
}

enum Restore {
    Restored(Option<NavTarget>),
    Missing,
    Stopped,
}

impl Navigation {
    fn start(intent: Intent, location: Option<NavTarget>) {
        let navigation = Self {
            read: DiffReadRequest::start(),
            intent,
        };
        spawn_from_root(navigation.run(location));
    }

    fn matches(&self, target: &NavTarget, s: &AppState) -> bool {
        self.read.matches(s)
            && match self.intent {
                Intent::History(forward) => history(s, forward).last() == Some(target),
                _ => true,
            }
    }

    fn valid(&self, target: &NavTarget) -> bool {
        self.read.pending() && app_signal().with_untracked(|s| self.matches(target, s))
    }

    async fn run(self, mut location: Option<NavTarget>) {
        loop {
            let target = if let Intent::History(forward) = self.intent {
                app_signal().with_untracked(|s| history(s, forward).last().cloned())
            } else {
                location.take()
            };
            let Some(target) = target else { break };
            if !self.valid(&target) {
                break;
            }
            match self.restore(&target).await {
                Ok(Restore::Restored(previous)) => {
                    if let Intent::History(forward) = self.intent {
                        app_signal().update(|s| {
                            let (from, to) = if forward {
                                (&mut s.nav_forward, &mut s.nav_back)
                            } else {
                                (&mut s.nav_back, &mut s.nav_forward)
                            };
                            from.pop();
                            if let Some(previous) = previous {
                                to.push(previous);
                            }
                        });
                    } else if matches!(self.intent, Intent::OpenRevision) {
                        if let Some(previous) = previous.filter(|previous| previous != &target) {
                            app_signal().update(|s| push_nav_back(s, previous));
                        }
                        record_recent_location(target);
                    }
                    break;
                }
                Ok(Restore::Missing) if self.valid(&target) => {
                    if let Intent::History(forward) = self.intent {
                        app_signal().update(|s| {
                            if forward {
                                s.nav_forward.pop();
                            } else {
                                s.nav_back.pop();
                            }
                        });
                    } else if matches!(self.intent, Intent::OpenRevision) {
                        show_toast("info", "该提交中没有这个文件");
                    } else {
                        show_toast("info", "该审阅位置已不可用");
                    }
                }
                Err(err) if self.valid(&target) => {
                    show_toast("err", format!("无法打开审阅位置：{err:#}"));
                    break;
                }
                _ => break,
            }
        }
        self.read.finish();
    }

    async fn restore(&self, target: &NavTarget) -> anyhow::Result<Restore> {
        match target {
            NavTarget::CodeFile {
                repo_id,
                rel_path,
                revision,
                pos,
                scroll_y,
            } => {
                let Some(file) =
                    app_signal().with_untracked(|s| find_source_file(s, repo_id, rel_path))
                else {
                    return Ok(Restore::Missing);
                };
                let id = code_tab_id(repo_id, rel_path, revision);
                let existing =
                    app_signal().with_untracked(|s| s.code_tabs.iter().position(|t| t.id == id));
                let opened = if existing.is_none() {
                    let target_file = file.clone();
                    let revision = revision.clone();
                    let data = blocking(move || read_source(&target_file, &revision)).await??;
                    if !self.valid(target) {
                        return Ok(Restore::Stopped);
                    }
                    if data.is_none()
                        || !app_signal().with_untracked(|s| {
                            s.contains_project_root(repo_id, &file.repo_path)
                        })
                    {
                        return Ok(Restore::Missing);
                    }
                    data
                } else {
                    None
                };
                let previous = capture_origin();
                let expected_tab = id.clone();
                app_signal().update(|s| {
                    let idx = if let Some(idx) = existing {
                        idx
                    } else if let Some(wt) = opened {
                        s.code_tabs.push(CodeTab::from_read(
                            file,
                            revision.clone(),
                            wt,
                            *pos,
                            false,
                        ));
                        s.code_tabs.len() - 1
                    } else {
                        return;
                    };
                    crate::code_split_runtime::select_tab_in_model(s, idx);
                    sync_dirty_working_diff_into_code_tab(s, idx);
                    crate::code_split_runtime::set_active_position(s, *pos);
                    s.editor_tab = EditorTabKind::File;
                    s.focus_zone = FocusZone::CodeEditor;
                });
                reveal_source_after_render(self.read.clone(), expected_tab, *pos, *scroll_y as f32);
                focus_active_editor_ref();
                Ok(Restore::Restored(previous))
            }
            NavTarget::WorkingDiff {
                file_key,
                staged,
                side,
                pos,
                viewport,
            } => {
                let source = || {
                    self.valid(target)
                        .then(|| {
                            app_signal().with_untracked(|s| {
                                s.statuses
                                    .iter()
                                    .flat_map(|status| status.files.iter())
                                    .find(|file| {
                                        file.key() == *file_key && change_in_section(file, *staged)
                                    })
                                    .map(|file| (s.refresh_gen, file.clone()))
                            })
                        })
                        .flatten()
                };
                if source().is_none() {
                    return Ok(Restore::Missing);
                }
                let already_open = app_signal().with_untracked(|s| {
                    s.working_diff.as_ref().is_some_and(|diff| {
                        diff.file.key() == *file_key && diff.data.staged_only == *staged
                    })
                });
                let opened = if already_open {
                    None
                } else {
                    if !self.save_working_before_return(target, file_key, *staged) {
                        return Ok(Restore::Stopped);
                    }
                    loop {
                        let Some(prepared) = prepare_working_diff(*staged, source).await? else {
                            return Ok(Restore::Missing);
                        };
                        if !self.save_working_before_return(target, file_key, *staged) {
                            return Ok(Restore::Stopped);
                        }
                        if source().as_ref() == Some(&prepared.source) {
                            break Some(prepared);
                        }
                    }
                };
                let previous = capture_origin();
                app_signal().update(|s| {
                    if let Some(prepared) = opened {
                        let rev = s
                            .working_diff
                            .as_ref()
                            .map_or(1, |diff| diff.content_rev + 1);
                        let saved_new_text = prepared.data.new_text.clone();
                        let current_new_text = prepared.draft.unwrap_or_else(|| saved_new_text.clone());
                        let undo_history = s.working_diff_undo_for(
                            &prepared.source.1,
                            &prepared.data,
                            &current_new_text,
                        );
                        s.working_diff = Some(WorkingDiffSession {
                            file: prepared.source.1,
                            disk_state: ReviewDiskState::for_diff(&prepared.data),
                            git_baseline_issue: None,
                            data: prepared.data,
                            current_new_text,
                            saved_new_text,
                            computed: prepared.computed,
                            active_side: *side,
                            old_cursor: EditorPosition::new(0, 0),
                            new_cursor: EditorPosition::new(0, 0),
                            viewport: None,
                            boundary_hint: None,
                            content_rev: rev,
                            undo_history,
                        });
                    } else {
                        sync_dirty_code_tab_into_working_diff(s);
                    }
                    if let Some(diff) = &mut s.working_diff {
                        let available = if *staged {
                            diff.file.staged
                        } else {
                            diff.file.unstaged
                        };
                        s.change_selection = available.then(|| ChangeSelection {
                            file: diff.file.clone(),
                            staged: *staged,
                        });
                        diff.active_side = *side;
                        diff.boundary_hint = None;
                        diff.viewport = *viewport;
                        match side {
                            DiffSide::Old => diff.old_cursor = *pos,
                            DiffSide::New => diff.new_cursor = *pos,
                        }
                    }
                    s.editor_tab = EditorTabKind::WorkingDiff;
                    s.focus_zone = editor_focus_zone(s);
                });
                restore_diff_view(EditorTabKind::WorkingDiff);
                Ok(Restore::Restored(previous))
            }
            NavTarget::CommitDiff {
                repo_id,
                commit_hash,
                file_path,
                scope,
                side,
                pos,
                viewport,
            } => {
                let already_open = app_signal().with_untracked(|s| {
                    s.commit_diff.as_ref().is_some_and(|diff| {
                        diff.commit.repo_id == *repo_id
                            && diff.commit.hash == *commit_hash
                            && diff.file.path == *file_path
                    })
                });
                let opened = if already_open {
                    None
                } else {
                    let repo = app_signal().with_untracked(|s| {
                        s.repos.iter().find(|repo| repo.id == *repo_id).cloned()
                    });
                    let Some(repo) = repo else {
                        return Ok(Restore::Missing);
                    };
                    let path = repo.path.clone();
                    let (hash, file_path) = (commit_hash.clone(), file_path.clone());
                    let result =
                        blocking(move || read_commit_location(&repo, &hash, &file_path)).await?;
                    if !self.valid(target) {
                        return Ok(Restore::Stopped);
                    }
                    let Some((commit, file, mut data)) = result? else {
                        return Ok(Restore::Missing);
                    };
                    if !app_signal().with_untracked(|s| {
                        s.repos
                            .iter()
                            .any(|repo| repo.id == *repo_id && repo.path == path)
                    }) {
                        return Ok(Restore::Missing);
                    }
                    let computed = loop {
                        let mode = whitespace_mode();
                        let (read, computed) = blocking(move || {
                            let computed = compute_diff(&data.old_text, &data.new_text, mode);
                            (data, computed)
                        })
                        .await?;
                        data = read;
                        if !self.valid(target) {
                            return Ok(Restore::Stopped);
                        }
                        if mode == whitespace_mode() {
                            break computed;
                        }
                    };
                    Some((commit, file, data, computed))
                };
                let previous = capture_origin();
                if let Some((commit, file, data, computed)) = opened {
                    install_commit_diff(commit, file, data, computed, CommitDiffOpenOptions {
                        scope: *scope,
                        select_log_file: true,
                        ..CommitDiffOpenOptions::default()
                    });
                }
                app_signal().update(|s| {
                    if let Some(diff) = &mut s.commit_diff {
                        diff.scope = *scope;
                        diff.active_side = *side;
                        diff.boundary_hint = None;
                        diff.viewport = *viewport;
                        match side {
                            DiffSide::Old => diff.old_cursor = *pos,
                            DiffSide::New => diff.new_cursor = *pos,
                        }
                    }
                    s.editor_tab = EditorTabKind::CommitDiff;
                    s.focus_zone = editor_focus_zone(s);
                });
                restore_diff_view(EditorTabKind::CommitDiff);
                Ok(Restore::Restored(previous))
            }
        }
    }

    fn save_working_before_return(&self, target: &NavTarget, file: &str, staged: bool) -> bool {
        let retry = target.clone();
        let current = self.clone();
        save_before_switching_working_diff(file, staged, move || {
            if current.read.latest() && app_signal().with_untracked(|s| current.matches(&retry, s))
            {
                Self::start(current.intent, Some(retry));
            }
        })
    }
}

fn capture_origin() -> Option<NavTarget> {
    capture_editor_views();
    current_nav_target()
}

fn restore_diff_view(tab: EditorTabKind) {
    set_text_input_focused(false);
    restore_diff_view_after_render(tab, true);
}

fn whitespace_mode() -> WhitespaceMode {
    app_signal().with_untracked(|s| WhitespaceMode::from_config(&s.cfg.diff_whitespace_mode))
}

fn find_source_file(s: &AppState, repo_id: &str, rel_path: &str) -> Option<WorkspaceFile> {
    if let Some(file) = s
        .workspace_files
        .iter()
        .find(|f| f.repo_id == repo_id && f.rel_path == rel_path)
    {
        return Some(file.clone());
    }
    let repo = s.repos.iter().find(|repo| repo.id == repo_id)?;
    Some(WorkspaceFile {
        repo_id: repo.id.clone(),
        repo_name: repo.label().to_owned(),
        repo_path: repo.path.clone(),
        rel_path: rel_path.to_owned(),
        abs_path: Path::new(&repo.path)
            .join(rel_path)
            .to_string_lossy()
            .to_string(),
        color_idx: repo.color_idx,
    })
}

fn read_source(file: &WorkspaceFile, revision: &str) -> anyhow::Result<Option<git::WorktreeRead>> {
    if !revision.is_empty() {
        let spec = format!("{revision}:{}", file.rel_path);
        let object = git::read_optional_git_object_text(Path::new(&file.repo_path), &spec)?;
        return Ok(object.map(|(text, binary, too_large)| git::WorktreeRead {
            text,
            line_ending: LineEnding::Lf,
            version: ReviewVersion::default(),
            editable: false,
            readonly_reason: Some(format!(
                "仓库版本 {} 为只读视图",
                revision.get(..8).unwrap_or(revision)
            )),
            binary,
            too_large,
        }));
    }
    git::read_worktree_source(Path::new(&file.abs_path))
}

fn read_commit_location(
    repo: &RepoInfo,
    hash: &str,
    path: &str,
) -> anyhow::Result<Option<(CommitEntry, CommitFileEntry, CommitDiffData)>> {
    let commit = match crate::git_log_query::lookup(repo, hash) {
        Ok(commit) => commit,
        Err(err) => {
            let rev = format!("{hash}^{{commit}}");
            if matches!(git::run_git_limited(Path::new(&repo.path), &["rev-parse", "--verify", "--quiet", &rev], 128, None),
                Ok(git::BoundedGitOutput::Failed { exit: Some(1), stderr }) if stderr.is_empty())
            {
                return Ok(None);
            }
            return Err(err);
        }
    };
    let Some(file) = git::commit_files(&repo.path, hash)?
        .into_iter()
        .find(|file| file.path == path)
    else {
        return Ok(None);
    };
    let data = git::read_commit_diff(&commit, &file)?;
    Ok(Some((commit, file, data)))
}
