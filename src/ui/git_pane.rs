use std::path::Path;

use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::{self, CommitEntry, WorkspaceFile};
use crate::paths::basename;
use crate::state::{
    FocusZone, RootOwnerHolder, app_signal, current_palette, open_code_file, open_commit_diff,
    open_file_revision_history, open_revision_code_file,
};
use crate::theme::status_text_color;
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

use super::git_forest;

#[derive(Clone, Default)]
struct Selection {
    generation: u64,
    loading: bool,
    error: Option<String>,
    base: Option<(CommitEntry, String)>,
    files_focused: bool,
}

fn selection() -> Signal<Selection> {
    global::<Selection>()
}

pub(crate) fn select_commit(commit: CommitEntry) {
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let (workspace, desired_path, history) = app_signal().with_untracked(|s| {
        let history = s.file_history.as_ref();
        (
            s.cfg.workspace_path.clone(),
            history.and_then(|history| history.paths.get(&commit.hash).cloned()),
            history.map(|history| {
                (
                    history.repo.id.clone(),
                    history.rel_path.clone(),
                    history.rev.clone(),
                )
            }),
        )
    });
    let generation = selection().with_untracked(|s| s.generation.wrapping_add(1));
    selection().update(|s| {
        s.generation = generation;
        s.loading = true;
        s.error = None;
    });
    app_signal().update(|s| {
        s.selected_commit = Some(commit.clone());
        s.selected_commit_files.clear();
        s.selected_commit_file = None;
    });
    owner.run(|| {
        spawn(async move {
            let path = commit.repo_path.clone();
            let hash = commit.hash.clone();
            let outcome = blocking(move || git::commit_files(&path, &hash)).await;
            if !selection().with_untracked(|s| s.generation == generation)
                || !app_signal().with_untracked(|s| {
                    s.cfg.workspace_path == workspace
                        && s.show_git_panel
                        && s.selected_commit
                            .as_ref()
                            .is_some_and(|entry| git_forest::same_commit(entry, &commit))
                        && s.file_history.as_ref().map(|history| {
                            (
                                history.repo.id.clone(),
                                history.rel_path.clone(),
                                history.rev.clone(),
                            )
                        }) == history
                })
            {
                return;
            }
            selection().update(|s| s.loading = false);
            let files = match outcome {
                Ok(Ok(files)) => files,
                Ok(Err(err)) => {
                    selection().update(|s| s.error = Some(err.to_string()));
                    return;
                }
                Err(err) => {
                    selection().update(|s| s.error = Some(err.to_string()));
                    return;
                }
            };
            let target = if history.is_some() {
                desired_path
                    .as_ref()
                    .and_then(|path| files.iter().find(|file| &file.path == path))
                    .cloned()
            } else {
                files.first().cloned()
            };
            app_signal().update(|s| {
                s.selected_commit_files = files;
                s.selected_commit_file = target.clone();
            });
            if app_signal().with_untracked(|s| s.cfg.git_log_diff_preview)
                && let Some(file) = target
            {
                crate::state::preview_commit_diff(commit, file);
            }
        })
    });
}

pub(crate) fn focus_history() {
    selection().update(|s| {
        s.base = None;
        s.files_focused = false;
    });
    app_signal().update(|s| s.focus_zone = FocusZone::GitFiles);
}

pub(super) fn move_selection(delta: isize) {
    if selection().with_untracked(|s| s.files_focused) {
        let next = app_signal().with_untracked(|s| {
            let commit = s.selected_commit.clone()?;
            let mut files = s.selected_commit_files.clone();
            if let Some(history) = s
                .file_history
                .as_ref()
                .filter(|history| !history.show_affected)
            {
                files.retain(|file| history.paths.get(&commit.hash) == Some(&file.path));
            }
            let index = next_index(
                &files,
                s.selected_commit_file.as_ref(),
                delta,
                PartialEq::eq,
            )?;
            Some((commit, files[index].clone(), s.cfg.git_log_diff_preview))
        });
        if let Some((commit, file, preview)) = next {
            app_signal().update(|s| s.selected_commit_file = Some(file.clone()));
            if preview {
                crate::state::preview_commit_diff(commit, file);
            }
        }
    } else {
        let (selected, history) = app_signal().with_untracked(|s| {
            (
                s.selected_commit.clone(),
                s.file_history
                    .as_ref()
                    .map(|history| history.commits.clone()),
            )
        });
        let mut rows = history.unwrap_or_else(|| {
            let results = git_forest::rows();
            let mut commits = results.commits.as_ref().clone();
            if results.grouping == git_forest::Grouping::Repository {
                commits.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));
            }
            commits
        });
        if let Some(index) = next_index(&rows, selected.as_ref(), delta, git_forest::same_commit) {
            select_commit(rows.remove(index));
        }
    }
}

fn next_index<T>(
    items: &[T],
    selected: Option<&T>,
    delta: isize,
    equal: impl Fn(&T, &T) -> bool,
) -> Option<usize> {
    let last = items.len().checked_sub(1)?;
    Some(
        selected
            .and_then(|selected| items.iter().position(|item| equal(item, selected)))
            .map_or(if delta < 0 { last } else { 0 }, |index| {
                index.saturating_add_signed(delta).min(last)
            }),
    )
}

fn preview_toggle() -> impl IntoElement {
    dynamic(|| {
        let p = current_palette();
        let active = app_signal().with(|s| s.cfg.git_log_diff_preview);
        view()
            .test_id("git-diff-preview")
            .label("差异预览")
            .h(px(23.0))
            .px(px(7.0))
            .rounded(px(4.0))
            .items_center()
            .justify_center()
            .cursor(Cursor::Pointer)
            .bg(if active { p.accent_soft } else { p.surface })
            .child(text("差异预览").text_size(px(10.5)).text_color(if active {
                p.accent_soft_text
            } else {
                p.text_primary
            }))
            .on_press(|| {
                let mut preview = None;
                app_signal().update(|s| {
                    s.cfg.git_log_diff_preview = !s.cfg.git_log_diff_preview;
                    if s.cfg.git_log_diff_preview {
                        preview = s
                            .selected_commit
                            .clone()
                            .zip(s.selected_commit_file.clone());
                    }
                });
                crate::state::save_config_quiet();
                if let Some((commit, file)) = preview {
                    crate::state::preview_commit_diff(commit, file);
                }
            })
    })
}

fn history_items(commit: CommitEntry, path: Option<String>) -> Vec<MenuItem> {
    let Some(path) = path else { return Vec::new() };
    let working = app_signal().with_untracked(|s| {
        let history = s.file_history.as_ref()?;
        s.workspace_files
            .iter()
            .find(|file| file.repo_id == history.repo.id && file.rel_path == history.rel_path)
            .cloned()
    });
    let base = selection().with_untracked(|s| s.base.clone());
    let mut items = Vec::new();
    if let Some(file) = working {
        let (revision, name) = (commit.clone(), path.clone());
        items.push(MenuItem::new("与当前文件比较…").on_select(move || {
            super::comparison::open_historical_file(file.clone(), revision.clone(), name.clone());
        }));
    }
    let (revision, name) = (commit.clone(), path.clone());
    items.push(MenuItem::new("设为比较基准").on_select(move || {
        selection().update(|s| s.base = Some((revision.clone(), name.clone())));
        crate::state::show_toast("info", format!("已选择修订 {}", revision.short_hash));
    }));
    if let Some((base, base_path)) = base.filter(|(base, _)| {
        base.repo_id == commit.repo_id && !git_forest::same_commit(base, &commit)
    }) {
        items.push(
            MenuItem::new(format!("与修订 {} 比较…", base.short_hash)).on_select(move || {
                super::comparison::open_two_revisions(
                    base.clone(),
                    base_path.clone(),
                    commit.clone(),
                    path.clone(),
                );
            }),
        );
    }
    items
}

pub fn git_pane() -> impl IntoElement {
    let is_history = computed(|| app_signal().with(|s| s.file_history.is_some()));
    view()
        .test_id("git-bottom-pane")
        .h(move || px(app_signal().with(|s| s.cfg.git_panel_height)))
        .w_full()
        .flex_col()
        .border_t_1()
        .border_color(move || current_palette().border)
        .bg(move || current_palette().rail_bg)
        // 顶栏
        .child(dynamic(move || {
            let p = current_palette();
            let (total, fh_name) = app_signal().with(|s| {
                (
                    s.file_history.as_ref().map_or_else(
                        || git_forest::rows().commits.len(),
                        |history| history.commits.len(),
                    ),
                    s.file_history.as_ref().map(|fh| basename(&fh.rel_path)),
                )
            });

            view()
                .h(px(34.0))
                .px(px(10.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .border_b_1()
                .border_color(p.border)
                .child(icons::icon(ICO_BRANCH, 13.0, p.accent))
                .child(
                    text("Git 提交历史")
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_primary),
                )
                .child(
                    text(format!("已加载 {total} 个提交"))
                        .text_size(px(10.5))
                        .text_color(p.text_muted),
                )
                .child(fh_name.map(|name| {
                    view()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(p.accent_soft)
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            text(format!("文件历史：{name}"))
                                .text_size(px(11.0))
                                .text_color(p.accent_soft_text),
                        )
                        .child(
                            view()
                                .cursor(Cursor::Pointer)
                                .on_press(|| {
                                    app_signal().update(|s| s.file_history = None);
                                    selection().update(|s| s.base = None);
                                })
                                .child(icons::icon(ICO_CLOSE, 10.0, p.accent_soft_text)),
                        )
                }))
                .child(view().flex_1())
                .child(preview_toggle())
                .child(when(
                    move || is_history.get(),
                    || {
                        dynamic(|| {
                            let p = current_palette();
                            let active = app_signal()
                                .with(|s| s.file_history.as_ref().is_some_and(|h| h.show_affected));
                            view()
                                .test_id("git-history-affected")
                                .label("显示所有受影响的文件")
                                .px(px(6.0))
                                .h(px(23.0))
                                .rounded(px(4.0))
                                .items_center()
                                .justify_center()
                                .cursor(Cursor::Pointer)
                                .bg(if active { p.accent_soft } else { p.surface })
                                .child(text("受影响的文件").text_size(px(10.5)).text_color(
                                    if active {
                                        p.accent_soft_text
                                    } else {
                                        p.text_primary
                                    },
                                ))
                                .on_press(|| {
                                    app_signal().update(|s| {
                                        if let Some(history) = s.file_history.as_mut() {
                                            history.show_affected = !history.show_affected;
                                        }
                                    })
                                })
                        })
                    },
                ))
                .child(
                    view()
                        .w(px(22.0))
                        .h(px(22.0))
                        .rounded(px(4.0))
                        .items_center()
                        .justify_center()
                        .cursor(Cursor::Pointer)
                        .hover(move |s| s.bg(p.surface_hover))
                        .label("关闭 Git")
                        .on_press(|| {
                            app_signal().update(|s| s.show_git_panel = false);
                        })
                        .child(icons::icon(ICO_CLOSE, 11.0, p.text_muted)),
                )
        }))
        .child(when(move || !is_history.get(), git_forest::controls))
        // 主体：左提交列表，右选中提交的变更文件列表
        .child(
            view()
                .flex_1()
                .flex_row()
                .overflow_hidden()
                .child(
                    // 提交列表
                    scroll()
                        .flex_1()
                        .border_r_1()
                        .border_color(move || current_palette().border)
                        .child(dynamic(move || {
                            let p = current_palette();
                            let (selected, history) = app_signal()
                                .with(|s| (s.selected_commit.clone(), s.file_history.clone()));
                            let results = git_forest::rows();
                            let is_history = history.is_some();
                            let mut filtered = history.as_ref().map_or_else(
                                || results.commits.as_ref().clone(),
                                |history| history.commits.clone(),
                            );
                            if !is_history && results.grouping == git_forest::Grouping::Repository {
                                filtered.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));
                            }
                            let mut rows: Vec<AnyElement> = Vec::new();
                            let mut label = None::<String>;
                            if let Some(status) = history
                                .as_ref()
                                .and_then(|history| {
                                    history.error.clone().or_else(|| {
                                        history.loading.then(|| "正在读取文件历史…".into())
                                    })
                                })
                                .or_else(|| {
                                    (!is_history && results.loading).then(|| "正在读取提交…".into())
                                })
                            {
                                rows.push(
                                    text(status)
                                        .text_size(px(11.0))
                                        .text_color(p.text_muted)
                                        .into_element(),
                                );
                            }
                            if !is_history {
                                for error in results.errors.iter() {
                                    rows.push(
                                        text(error.clone())
                                            .text_size(px(11.0))
                                            .text_color(p.danger)
                                            .into_element(),
                                    );
                                }
                            }
                            if filtered.is_empty()
                                && history
                                    .as_ref()
                                    .map_or(!results.loading, |history| !history.loading)
                            {
                                rows.push(
                                    text("无匹配的提交")
                                        .text_size(px(11.0))
                                        .text_color(p.text_muted)
                                        .into_element(),
                                );
                            }
                            let graph = (!is_history
                                && results.grouping == git_forest::Grouping::Graph)
                                .then(|| super::git_graph::layout(&filtered));
                            for (index, c) in filtered.into_iter().enumerate() {
                                if !is_history {
                                    let group = results.grouping.key(&c);
                                    if group != label.as_deref() {
                                        label = group.map(str::to_owned);
                                        if let Some(label) = &label {
                                            rows.push(
                                                view()
                                                    .h(px(22.0))
                                                    .px(px(8.0))
                                                    .bg(p.surface2)
                                                    .child(
                                                        text(label.clone())
                                                            .text_size(px(10.5))
                                                            .text_color(p.text_muted),
                                                    )
                                                    .into_element(),
                                            );
                                        }
                                    }
                                }
                                let is_sel = selected
                                    .as_ref()
                                    .is_some_and(|selected| git_forest::same_commit(selected, &c));
                                let history_path =
                                    history.as_ref().and_then(|h| h.paths.get(&c.hash).cloned());
                                let history_commit = c.clone();
                                let c_click = c.clone();
                                let date_short = c.date.get(..10).unwrap_or(&c.date).to_string();
                                rows.push(
                                    view()
                                        .test_id(&format!("git-commit-row-{}", c.short_hash))
                                        .h(px(26.0))
                                        .px(px(10.0))
                                        .flex_row()
                                        .items_center()
                                        .gap(px(8.0))
                                        .bg(if is_sel {
                                            p.active_row_bg
                                        } else {
                                            transparent_black()
                                        })
                                        .cursor(Cursor::Pointer)
                                        .hover(
                                            move |s| {
                                                if is_sel { s } else { s.bg(p.surface_hover) }
                                            },
                                        )
                                        .on_press(move || {
                                            selection().update(|s| s.files_focused = false);
                                            app_signal()
                                                .update(|s| s.focus_zone = FocusZone::GitFiles);
                                            select_commit(c_click.clone());
                                        })
                                        .on_context_menu(move |ev: &ContextMenuEvent| {
                                            let menu = history_items(
                                                history_commit.clone(),
                                                history_path.clone(),
                                            );
                                            if !menu.is_empty() {
                                                crate::state::show_popup_menu_at(
                                                    menu,
                                                    ev.page_position,
                                                );
                                            }
                                        })
                                        .child(graph.as_ref().map(|graph| graph.row(index, p)))
                                        .child(
                                            text(c.short_hash.clone())
                                                .text_size(px(11.0))
                                                .text_color(p.accent_soft_text),
                                        )
                                        .child(
                                            view().flex_1().overflow_hidden().child(
                                                text(c.subject.clone())
                                                    .text_size(px(11.5))
                                                    .text_color(p.text_primary)
                                                    .number_of_lines(1u32),
                                            ),
                                        )
                                        .child(
                                            text(c.author.clone())
                                                .text_size(px(10.5))
                                                .text_color(p.text_muted)
                                                .number_of_lines(1u32),
                                        )
                                        .child(
                                            text(date_short)
                                                .text_size(px(10.5))
                                                .text_color(p.text_muted)
                                                .number_of_lines(1u32),
                                        )
                                        .into_element(),
                                );
                            }
                            if !is_history && results.more {
                                rows.push(git_forest::more_button().into_element());
                            } else if !is_history && results.scan_limited {
                                rows.push(
                                    text("已搜索每个仓库最近 2000 个符合范围的提交")
                                        .text_size(px(11.0))
                                        .text_color(p.text_muted)
                                        .into_element(),
                                );
                            }
                            view().flex_col().py(px(2.0)).children(rows)
                        })),
                )
                .child(
                    // 选中提交的文件列表
                    view()
                        .w(px(360.0))
                        .flex_none()
                        .flex_col()
                        .overflow_hidden()
                        .child(dynamic(move || {
                            let p = current_palette();
                            let (commit, files, sel_file) = app_signal().with(|s| {
                                (
                                    s.selected_commit.clone(),
                                    s.selected_commit_files.clone(),
                                    s.selected_commit_file.clone(),
                                )
                            });
                            let Some(commit) = commit else {
                                return view()
                                    .p(px(12.0))
                                    .child(
                                        text("选择一个提交以查看变更文件")
                                            .text_size(px(11.5))
                                            .text_color(p.text_muted),
                                    )
                                    .into_element();
                            };

                            let detail = selection().with(|s| (s.loading, s.error.clone()));
                            if detail.0 || detail.1.is_some() {
                                return view()
                                    .p(px(12.0))
                                    .child(
                                        text(
                                            detail.1.unwrap_or_else(|| "正在读取变更文件…".into()),
                                        )
                                        .text_size(px(11.5))
                                        .text_color(p.text_muted),
                                    )
                                    .into_element();
                            }
                            let files = app_signal().with(|s| {
                                let path = s
                                    .file_history
                                    .as_ref()
                                    .filter(|history| !history.show_affected)
                                    .map(|history| history.paths.get(&commit.hash));
                                match path {
                                    Some(Some(path)) => files
                                        .into_iter()
                                        .filter(|file| &file.path == path)
                                        .collect(),
                                    Some(None) => Vec::new(),
                                    None => files,
                                }
                            });
                            let mut rows: Vec<AnyElement> = Vec::new();
                            rows.push(
                                view()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .border_b_1()
                                    .border_color(p.border)
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        text(commit.subject.clone())
                                            .text_size(px(11.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.text_primary)
                                            .number_of_lines(1u32),
                                    )
                                    .child(
                                        text(format!(
                                            "{} · {} · {} 个文件",
                                            commit.short_hash,
                                            commit.author,
                                            files.len()
                                        ))
                                        .text_size(px(10.5))
                                        .text_color(p.text_muted)
                                        .number_of_lines(1u32),
                                    )
                                    .into_element(),
                            );

                            for cf in files {
                                let is_sel = sel_file.as_ref().is_some_and(|s| s.path == cf.path);
                                let col = status_text_color(&cf.status, p);
                                let label = if !cf.orig_path.is_empty() {
                                    format!("{} → {}", cf.orig_path, cf.path)
                                } else {
                                    cf.path.clone()
                                };
                                let c_click = commit.clone();
                                let cf_click = cf.clone();
                                let c_cm = commit.clone();
                                let cf_cm = cf.clone();

                                rows.push(
                                    view()
                                        .test_id(&format!("git-commit-file-{}", cf.path))
                                        .h(px(24.0))
                                        .px(px(10.0))
                                        .flex_row()
                                        .items_center()
                                        .gap(px(6.0))
                                        .bg(if is_sel {
                                            p.active_row_bg
                                        } else {
                                            transparent_black()
                                        })
                                        .cursor(Cursor::Pointer)
                                        .hover(
                                            move |s| {
                                                if is_sel { s } else { s.bg(p.surface_hover) }
                                            },
                                        )
                                        .on_press(move |e: &PressEvent| {
                                            selection().update(|s| s.files_focused = true);
                                            app_signal().update(|s| {
                                                s.selected_commit_file = Some(cf_click.clone());
                                                s.focus_zone = FocusZone::GitFiles;
                                            });
                                            if e.clicks >= 2 {
                                                open_commit_diff(
                                                    c_click.clone(),
                                                    cf_click.clone(),
                                                    true,
                                                );
                                            } else if app_signal()
                                                .with_untracked(|s| s.cfg.git_log_diff_preview)
                                            {
                                                crate::state::preview_commit_diff(
                                                    c_click.clone(),
                                                    cf_click.clone(),
                                                );
                                            }
                                        })
                                        .on_context_menu(move |e: &ContextMenuEvent| {
                                            selection().update(|s| s.files_focused = true);
                                            let commit_c = c_cm.clone();
                                            let file_c = cf_cm.clone();
                                            app_signal().update(|s| {
                                                s.selected_commit_file = Some(file_c.clone());
                                                s.focus_zone = FocusZone::GitFiles;
                                            });
                                            let abs = Path::new(&commit_c.repo_path)
                                                .join(&file_c.path)
                                                .to_string_lossy()
                                                .to_string();
                                            let mut items = vec![
                                                MenuItem::new("查看差异")
                                                    .shortcut("mod+d")
                                                    .on_select({
                                                        let c1 = commit_c.clone();
                                                        let f1 = file_c.clone();
                                                        move || {
                                                            open_commit_diff(
                                                                c1.clone(),
                                                                f1.clone(),
                                                                true,
                                                            )
                                                        }
                                                    }),
                                                MenuItem::new("打开仓库版本").on_select({
                                                    let c1 = commit_c.clone();
                                                    let p1 = file_c.path.clone();
                                                    move || open_revision_code_file(&c1, &p1)
                                                }),
                                                MenuItem::new("打开本地源码").on_select({
                                                    let c1 = commit_c.clone();
                                                    let p1 = file_c.path.clone();
                                                    let a1 = abs.clone();
                                                    move || {
                                                        open_code_file(
                                                            WorkspaceFile {
                                                                repo_id: c1.repo_id.clone(),
                                                                repo_name: c1.repo_name.clone(),
                                                                repo_path: c1.repo_path.clone(),
                                                                rel_path: p1.clone(),
                                                                abs_path: a1.clone(),
                                                                color_idx: 0,
                                                            },
                                                            None,
                                                            true,
                                                        );
                                                    }
                                                }),
                                                MenuItem::new("查看截至此处的历史").on_select({
                                                    let c1 = commit_c.clone();
                                                    let p1 = file_c.path.clone();
                                                    move || {
                                                        if let Some(repo) = app_signal()
                                                            .with_untracked(|s| {
                                                                s.repos
                                                                    .iter()
                                                                    .find(|r| r.id == c1.repo_id)
                                                                    .cloned()
                                                            })
                                                        {
                                                            open_file_revision_history(
                                                                repo,
                                                                c1.hash.clone(),
                                                                p1.clone(),
                                                            );
                                                        }
                                                    }
                                                }),
                                                MenuItem::separator(),
                                            ];
                                            items.extend(build_location_menu_items(
                                                commit_c.repo_id.clone(),
                                                commit_c.repo_path.clone(),
                                                file_c.path.clone(),
                                                abs,
                                                1,
                                                false,
                                            ));
                                            crate::state::show_popup_menu_at(
                                                items,
                                                e.page_position,
                                            );
                                        })
                                        .child(
                                            text(cf.status.clone())
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(col),
                                        )
                                        .child(
                                            view().flex_1().overflow_hidden().child(
                                                text(label)
                                                    .text_size(px(11.5))
                                                    .text_color(col)
                                                    .number_of_lines(1u32),
                                            ),
                                        )
                                        .into_element(),
                                );
                            }

                            scroll()
                                .flex_1()
                                .child(view().flex_col().children(rows))
                                .into_element()
                        })),
                ),
        )
}
