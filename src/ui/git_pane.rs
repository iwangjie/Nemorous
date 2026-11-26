use std::path::Path;

use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::{self, CommitEntry, CommitFileEntry, WorkspaceFile};
use crate::paths::basename;
use crate::state::{
    AppState, FocusZone, RootOwnerHolder, app_signal, current_palette, open_code_file, open_commit_diff,
    open_file_revision_history, open_revision_code_file,
};
use crate::theme::status_text_color;
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

use super::git_forest;

pub(super) fn local_author_date(date: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(date)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| date.to_owned())
}

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

pub(crate) fn clear_commit_selection() {
    selection().update(|s| {
        s.generation = s.generation.wrapping_add(1);
        s.loading = false;
        s.error = None;
        s.files_focused = false;
    });
    app_signal().update(|s| {
        s.selected_commit = None;
        s.selected_commit_files.clear();
        s.selected_commit_file = None;
    });
}

pub(crate) fn select_commit(commit: CommitEntry) {
    let loaded = selection().with_untracked(|s| !s.loading && s.error.is_none());
    if loaded {
        let current_file = app_signal().with_untracked(|s| {
            s.selected_commit.as_ref()
                .filter(|selected| git_forest::same_commit(selected, &commit))
                .map(|_| s.selected_commit_file.clone())
        });
        if let Some(file) = current_file {
            if let Some(file) = file {
                crate::state::preview_commit_diff(commit, file);
            }
            return;
        }
    }
    load_commit_selection(commit, true);
}

pub(crate) fn load_commit_selection(commit: CommitEntry, preview: bool) {
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
            if preview && app_signal().with_untracked(|s| s.show_git_panel && s.cfg.git_log_diff_preview)
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
            let files = visible_files(s, &commit);
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

pub(crate) fn visible_files(s: &AppState, commit: &CommitEntry) -> Vec<CommitFileEntry> {
    let mut files = s.selected_commit_files.clone();
    files.retain(|file| in_file_history_scope(s, commit, file));
    order_files(&mut files, s.cfg.git_log_group_files);
    files
}

pub(crate) fn file_is_visible(s: &AppState, commit: &CommitEntry, file: &CommitFileEntry) -> bool {
    s.selected_commit.as_ref().is_some_and(|selected| git_forest::same_commit(selected, commit))
        && in_file_history_scope(s, commit, file)
        && s.selected_commit_files.contains(file)
}

fn in_file_history_scope(s: &AppState, commit: &CommitEntry, file: &CommitFileEntry) -> bool {
    s.file_history.as_ref().filter(|history| !history.show_affected)
        .is_none_or(|history| history.paths.get(&commit.hash) == Some(&file.path))
}

pub(crate) fn order_files(files: &mut [CommitFileEntry], group_by_directory: bool) {
    if group_by_directory {
        files.sort_by(|a, b| directory(&a.path).cmp(directory(&b.path)).then(a.path.cmp(&b.path)));
    }
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
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

fn toggle_preview() {
    let enabled = app_signal().with_untracked(|s| !s.cfg.git_log_diff_preview);
    crate::state::set_commit_diff_preview(enabled);
}

pub(super) fn show_log_options() {
    use crate::config::GitLogColumns;
    let columns = app_signal().with_untracked(|s| s.cfg.git_log_columns);
    let toggle = |label, checked, field: fn(&mut GitLogColumns) -> &mut bool| {
        MenuItem::new(label).checked(checked).on_select(move || {
            app_signal().update(|s| {
                let visible = field(&mut s.cfg.git_log_columns);
                *visible = !*visible;
            });
            crate::state::save_config_quiet();
        })
    };
    crate::state::show_popup_menu(vec![
        MenuItem::submenu("列", vec![
            toggle("作者", columns.author, |c| &mut c.author),
            toggle("日期", columns.date, |c| &mut c.date),
            toggle("提交哈希", columns.hash, |c| &mut c.hash),
        ]),
        toggle("显示分支和标签", columns.refs, |c| &mut c.refs),
    ]);
}

fn show_file_options() {
    let (preview, grouped, details, affected) = app_signal().with_untracked(|s| {
        (s.cfg.git_log_diff_preview, s.cfg.git_log_group_files, s.cfg.git_log_show_details,
            s.file_history.as_ref().map(|h| h.show_affected))
    });
    let mut items = vec![
        MenuItem::new("差异预览").checked(preview).on_select(toggle_preview),
        MenuItem::new("按目录分组").checked(grouped).on_select(|| {
            app_signal().update(|s| s.cfg.git_log_group_files = !s.cfg.git_log_group_files);
            crate::state::save_config_quiet();
        }),
        MenuItem::new("显示提交详情").checked(details).on_select(|| {
            app_signal().update(|s| s.cfg.git_log_show_details = !s.cfg.git_log_show_details);
            crate::state::save_config_quiet();
        }),
    ];
    if let Some(affected) = affected {
        items.push(MenuItem::new("显示所有受影响的文件").checked(affected).on_select(|| {
            let mut preview = None;
            app_signal().update(|s| {
                if let Some(history) = s.file_history.as_mut() {
                    history.show_affected = !history.show_affected;
                }
                if let Some(commit) = &s.selected_commit {
                    let visible = visible_files(s, commit);
                    if s.selected_commit_file.as_ref().is_none_or(|file| !visible.contains(file)) {
                        s.selected_commit_file = visible.into_iter().next();
                        if s.cfg.git_log_diff_preview {
                            preview = Some(commit.clone()).zip(s.selected_commit_file.clone());
                        }
                    }
                }
            });
            if let Some((commit, file)) = preview {
                crate::state::preview_commit_diff(commit, file);
            }
        }));
    }
    crate::state::show_popup_menu(items);
}

fn history_items(commit: CommitEntry, path: Option<String>) -> Vec<MenuItem> {
    let mut items = super::git_advanced::commit_items(&commit);
    let Some(path) = path else { return items; };
    let working = app_signal().with_untracked(|s| {
        let history = s.file_history.as_ref()?;
        s.workspace_files
            .iter()
            .find(|file| file.repo_id == history.repo.id && file.rel_path == history.rel_path)
            .cloned()
    });
    let base = selection().with_untracked(|s| s.base.clone());
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
    let selected_commit_row = element_ref();
    let selected_file_row = element_ref();
    let selected_commit = computed(|| app_signal().with(|s| {
        s.selected_commit.as_ref().map(|commit| (commit.repo_id.clone(), commit.hash.clone()))
    }));
    let selected_file = computed(move || {
        Some((selected_commit.get()?, app_signal().with(|s| s.selected_commit_file.clone())?))
    });
    let file_options = computed(|| app_signal().with(|s| {
        (s.cfg.git_log_group_files, s.cfg.git_log_show_details, s.file_history.as_ref().map(|h| h.show_affected))
    }));
    let file_list = computed(|| app_signal().with(|s| {
        s.selected_commit.as_ref().map(|commit| {
            (commit.clone(), s.selected_commit_file.clone(), visible_files(s, commit))
        })
    }));
    let file_status = computed(|| selection().with(|s| (s.loading, s.error.clone())));
    effect(move || {
        if selected_commit.get().is_some() {
            super::reveal_selected_row(selected_commit_row);
        }
    });
    effect(move || {
        file_options.get();
        if selected_file.get().is_some() {
            super::reveal_selected_row(selected_file_row);
        }
    });
    view()
        .test_id("git-bottom-pane")
        .h_full()
        .w_full()
        .flex_col()
        .border_t_1()
        .border_color(move || current_palette().border)
        .bg(move || current_palette().rail_bg)
        // 顶栏
        .child(dynamic(move || {
            let p = current_palette();
            let file_history = app_signal().with(|s| {
                s.file_history.as_ref().map(|h| basename(&h.rel_path))
            });
            let title = file_history.as_ref().map_or_else(
                || "Git".to_owned(),
                |name| format!("文件历史：{name}"),
            );

            view()
                .h(px(32.0))
                .px(px(10.0))
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .border_b_1()
                .border_color(p.border)
                .child(icons::icon(ICO_BRANCH, 13.0, p.accent))
                .child(
                    text(title)
                        .test_id(if file_history.is_some() { "git-file-history" } else { "git-log-title" })
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_primary)
                        .number_of_lines(1u32),
                )
                .child(file_history.is_some().then(|| {
                    super::controls::button()
                        .w(px(22.0))
                        .h(px(22.0))
                        .rounded(px(4.0))
                        .cursor(Cursor::Pointer)
                        .hover(move |s| s.bg(p.surface_hover))
                        .label("关闭文件历史")
                        .tooltip(super::tooltip_label("关闭文件历史".into(), p))
                        .on_press(|| {
                            app_signal().update(|s| s.file_history = None);
                            selection().update(|s| s.base = None);
                        })
                        .child(icons::icon(ICO_CLOSE, 10.0, p.text_muted))
                }))
                .child(view().flex_1())
                .child(
                    super::controls::button()
                        .w(px(22.0))
                        .h(px(22.0))
                        .rounded(px(4.0))
                        .cursor(Cursor::Pointer)
                        .hover(move |s| s.bg(p.surface_hover))
                        .label("关闭 Git")
                        .on_press(|| {
                            app_signal().update(|s| s.show_git_panel = false);
                        })
                        .child(icons::icon(ICO_CLOSE, 11.0, p.text_muted)),
                )
        }))
        // 主体：左提交列表，右选中提交的变更文件列表
        .child(
            view()
                .flex_1()
                .flex_row()
                .overflow_hidden()
                .child(
                    // 提交列表
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .border_r_1()
                        .border_color(move || current_palette().border)
                        .child(when(move || !is_history.get(), || dynamic(git_forest::controls)))
                        .child(scroll().test_id("git-commit-list-scroll").flex_1().child(dynamic(move || {
                            let p = current_palette();
                            let (selected, history, columns) = app_signal().with(|s| {
                                (s.selected_commit.clone(), s.file_history.clone(), s.cfg.git_log_columns)
                            });
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
                                let date = local_author_date(&c.date);
                                if !is_history {
                                    let group = results.grouping.key(&c, &date);
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
                                let refs = (columns.refs && !is_history && !c.refs.is_empty())
                                    .then(|| c.refs.replace(" -> ", " → ").replace(", ", " · "));
                                let subject_hint = format!("{}\n{} · {}", c.subject, c.repo_name, c.short_hash);
                                rows.push(
                                    view()
                                        .test_id(&format!("git-commit-row-{}", c.short_hash))
                                        .when(is_sel, |row| row.element_ref(&selected_commit_row))
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
                                        .child(columns.author.then(|| {
                                            text(c.author)
                                                .test_id("git-commit-author")
                                                .w(px(100.0))
                                                .min_w(px(48.0))
                                                .text_size(px(11.0))
                                                .text_color(p.text_primary)
                                                .number_of_lines(1u32)
                                        }))
                                        .child(columns.date.then(|| {
                                            text(date)
                                                .test_id("git-commit-date")
                                                .w(px(105.0))
                                                .min_w(px(76.0))
                                                .text_size(px(10.5))
                                                .text_color(p.text_muted)
                                                .number_of_lines(1u32)
                                        }))
                                        .child(graph.as_ref().map(|graph| graph.row(index, p)))
                                        .child(columns.hash.then(|| {
                                            text(c.short_hash)
                                                .test_id("git-commit-hash")
                                                .flex_none()
                                                .text_size(px(10.5))
                                                .text_color(p.text_muted)
                                        }))
                                        .child(
                                            view()
                                                .test_id("git-commit-subject")
                                                .flex_1()
                                                .min_w(px(64.0))
                                                .overflow_hidden()
                                                .tooltip(super::tooltip_label(subject_hint, p).max_w(px(360.0)))
                                                .child(
                                                    text(c.subject)
                                                        .text_size(px(11.5))
                                                        .text_color(p.text_primary)
                                                        .number_of_lines(1u32),
                                                ),
                                        )
                                        .child(refs.map(|refs| {
                                            text(refs.clone())
                                                .test_id("git-commit-refs")
                                                .min_w(px(0.0))
                                                .max_w(relative(0.28))
                                                .tooltip(super::tooltip_label(refs, p))
                                                .text_size(px(10.5))
                                                .text_color(p.accent_soft_text)
                                                .number_of_lines(1u32)
                                        }))
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
                        }))),
                )
                .child(
                    // 选中提交的文件列表
                    view()
                        .w(relative(0.30))
                        .max_w(px(360.0))
                        .flex_none()
                        .flex_col()
                        .overflow_hidden()
                        .child(
                            view()
                                .h(px(30.0))
                                .px(px(5.0))
                                .flex_row()
                                .items_center()
                                .border_b_1()
                                .border_color(move || current_palette().border)
                                .child(dynamic(move || {
                                    git_forest::icon_action(
                                        "git-show-diff", "查看差异", ICO_COLUMNS, false, selected_file.get().is_some(),
                                        || {
                                            let selected = app_signal().with_untracked(|s| {
                                                s.selected_commit.clone().zip(s.selected_commit_file.clone())
                                            });
                                            if let Some((commit, file)) = selected {
                                                open_commit_diff(commit, file, true);
                                            }
                                        },
                                    )
                                }))
                                .child(git_forest::icon_action(
                                    "git-file-view-options", "变更文件视图选项", ICO_EYE, false, true, show_file_options,
                                )),
                        )
                        .child(dynamic(move || {
                            let p = current_palette();
                            let (grouped, show_details, _) = file_options.get();
                            let Some((commit, sel_file, files)) = file_list.get() else {
                                return view()
                                    .p(px(12.0))
                                    .child(
                                        text("选择一个提交以查看变更文件")
                                            .text_size(px(11.5))
                                            .text_color(p.text_muted),
                                    )
                                    .into_element();
                            };

                            let (loading, error) = file_status.get();
                            if loading || error.is_some() {
                                return view()
                                    .p(px(12.0))
                                    .child(
                                        text(
                                            error.unwrap_or_else(|| "正在读取变更文件…".into()),
                                        )
                                        .text_size(px(11.5))
                                        .text_color(p.text_muted),
                                    )
                                    .into_element();
                            }
                            let details = show_details.then(|| view()
                                .test_id("git-commit-details")
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
                                ));

                            let mut rows = Vec::with_capacity(files.len());
                            let mut last_directory = None;
                            for cf in files {
                                let is_sel = sel_file.as_ref().is_some_and(|s| s.path == cf.path);
                                let col = status_text_color(&cf.status, p);
                                let (directory, name) =
                                    cf.path.rsplit_once('/').unwrap_or(("", cf.path.as_str()));
                                if grouped && last_directory.as_deref() != Some(directory) {
                                    last_directory = Some(directory.to_owned());
                                    rows.push(view()
                                        .test_id(&format!("git-file-directory-{directory}"))
                                        .h(px(24.0))
                                        .px(px(8.0))
                                        .gap(px(5.0))
                                        .flex_row()
                                        .items_center()
                                        .child(icons::icon(ICO_FOLDER, 12.0, p.text_muted))
                                        .child(text(if directory.is_empty() { "仓库根目录" } else { directory }.to_owned())
                                            .text_size(px(10.5))
                                            .text_color(p.text_muted))
                                        .into_element());
                                }
                                let original_name = cf.orig_path.rsplit('/').next().unwrap_or_default();
                                let name = if !original_name.is_empty() && original_name != name {
                                    format!("{original_name} → {name}")
                                } else {
                                    name.to_owned()
                                };
                                let location = if cf.orig_path.is_empty() {
                                    cf.path.clone()
                                } else {
                                    format!("{} → {}", cf.orig_path, cf.path)
                                };
                                let c_click = commit.clone();
                                let cf_click = cf.clone();
                                let c_cm = commit.clone();
                                let cf_cm = cf.clone();

                                rows.push(
                                    view()
                                        .test_id(&format!("git-commit-file-{}", cf.path))
                                        .when(is_sel, |row| row.element_ref(&selected_file_row))
                                        .h(px(24.0))
                                        .flex_none()
                                        .px(px(if grouped { 24.0 } else { 10.0 }))
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
                                                MenuItem::new("打开仓库版本").disabled(file_c.status == "D").on_select({
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
                                            text(name)
                                                .flex_none()
                                                .text_size(px(11.5))
                                                .text_color(col)
                                                .number_of_lines(1u32),
                                        )
                                        .child((!grouped && !directory.is_empty()).then(|| {
                                            text(directory.to_owned())
                                                .flex_none()
                                                .text_size(px(10.0))
                                                .text_color(p.text_muted)
                                                .number_of_lines(1u32)
                                        }))
                                        .tooltip(super::tooltip_label(location, p))
                                        .into_element(),
                                );
                            }

                            view()
                                .flex_1()
                                .flex_col()
                                .overflow_hidden()
                                .child(details)
                                .child(
                                    scroll()
                                        .horizontal()
                                        .test_id("git-commit-files-scroll")
                                        .flex_1()
                                        .child(
                                            scroll().test_id("git-commit-file-list-scroll").flex_none().h_full().min_w(relative(1.0))
                                                .child(view().flex_col().children(rows)),
                                        ),
                                )
                                .into_element()
                        })),
                ),
        )
}
