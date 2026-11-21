use std::path::Path;

use nsh_base::input;
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::git::{self, CommitEntry, WorkspaceFile};
use crate::paths::basename;
use crate::state::{
    FocusZone, app_signal, current_palette, open_code_file, open_commit_diff,
    open_file_revision_history, open_revision_code_file,
};
use crate::theme::status_text_color;
use crate::ui::commit_pane::build_location_menu_items;
use crate::ui::icons::{self, *};

fn select_commit(commit: CommitEntry) {
    let files = git::commit_files(&commit.repo_path, &commit.hash).unwrap_or_default();
    app_signal().update(|s| {
        s.selected_commit_file = files.first().cloned();
        s.selected_commit_files = files;
        s.selected_commit = Some(commit);
    });
}

pub fn git_pane() -> impl IntoElement {
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
                    s.git_commits.len(),
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
                                })
                                .child(icons::icon(ICO_CLOSE, 10.0, p.accent_soft_text)),
                        )
                }))
                .child(view().flex_1())
                .child(
                    input()
                        .test_id("git-filter-input")
                        .w(px(200.0))
                        .h(px(24.0))
                        .px(px(8.0))
                        .rounded(px(5.0))
                        .bg(p.surface)
                        .border_1()
                        .border_color(p.border)
                        .text_size(px(11.0))
                        .text_color(p.text_primary)
                        .placeholder("过滤提交信息 / 哈希…")
                        .value(move || app_signal().with(|s| s.git_filter_text.clone()))
                        .on_focus(|| crate::state::set_text_input_focused(true))
                        .on_blur(|| crate::state::set_text_input_focused(false))
                        .on_change(|v: &str| {
                            app_signal().update(|s| s.git_filter_text = v.to_string());
                        }),
                )
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
                            let (commits, filter_q, selected_hash, fh) = app_signal().with(|s| {
                                (
                                    s.git_commits.clone(),
                                    s.git_filter_text.trim().to_ascii_lowercase(),
                                    s.selected_commit.as_ref().map(|c| c.hash.clone()),
                                    s.file_history.clone(),
                                )
                            });
                            let source = if let Some(fh) = fh {
                                fh.commits
                            } else {
                                commits
                            };

                            let filtered: Vec<CommitEntry> = source
                                .into_iter()
                                .filter(|c| {
                                    filter_q.is_empty()
                                        || c.subject.to_ascii_lowercase().contains(&filter_q)
                                        || c.short_hash.to_ascii_lowercase().contains(&filter_q)
                                        || c.author.to_ascii_lowercase().contains(&filter_q)
                                })
                                .collect();

                            let mut rows: Vec<AnyElement> = Vec::new();
                            for c in filtered {
                                let is_sel = selected_hash.as_deref() == Some(c.hash.as_str());
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
                                        .hover(move |s| {
                                            if is_sel {
                                                s
                                            } else {
                                                s.bg(p.surface_hover)
                                            }
                                        })
                                        .on_press(move || select_commit(c_click.clone()))
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
                                        .hover(move |s| {
                                            if is_sel {
                                                s
                                            } else {
                                                s.bg(p.surface_hover)
                                            }
                                        })
                                        .on_press(move |e: &PressEvent| {
                                            app_signal().update(|s| {
                                                s.selected_commit_file = Some(cf_click.clone());
                                                s.focus_zone = FocusZone::GitFiles;
                                            });
                                            if e.clicks >= 2 {
                                                open_commit_diff(c_click.clone(), cf_click.clone(), true);
                                            }
                                        })
                                        .on_context_menu(move |e: &ContextMenuEvent| {
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
                                                        move || open_commit_diff(c1.clone(), f1.clone(), true)
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
                                                        if let Some(repo) = app_signal().with_untracked(|s| {
                                                            s.repos.iter().find(|r| r.id == c1.repo_id).cloned()
                                                        }) {
                                                            open_file_revision_history(repo, c1.hash.clone(), p1.clone());
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
                                            crate::state::show_popup_menu_at(items, e.page_position);
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
