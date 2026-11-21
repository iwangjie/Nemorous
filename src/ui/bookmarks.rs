//! 旧版内置行书签：配置持久化、编辑器栏标记与书签弹窗。

use std::collections::HashSet;

use nsh_base::{EditorDecoration, EditorGutterMark, EditorPosition, EditorRange, input};
use nsh_core::prelude::*;

use crate::config::{Bookmark, Config};
use crate::state::{self, EditorTabKind, app_signal, current_palette};

#[derive(Clone, Default)]
struct BookmarksUi {
    open: bool,
    query: String,
}

pub fn open_modal() {
    global::<BookmarksUi>().set(BookmarksUi {
        open: true,
        query: String::new(),
    });
    app_signal().update(state::enter_modal_focus);
    state::set_text_input_focused(true);
}

pub fn close_active() -> bool {
    let ui = global::<BookmarksUi>();
    if !ui.with_untracked(|s| s.open) {
        return false;
    }
    ui.set(BookmarksUi::default());
    true
}

fn active_file_location() -> Option<(String, String, i64)> {
    app_signal().with_untracked(|s| {
        let tab = s.code_tabs.get(s.active_code_tab_idx)?;
        if s.editor_tab != EditorTabKind::File || !tab.revision.is_empty() {
            return None;
        }
        Some((
            tab.file.repo_id.clone(),
            tab.file.rel_path.clone(),
            i64::from(tab.cursor.line) + 1,
        ))
    })
}

fn add_bookmark_to(cfg: &mut Config, repo_id: &str, rel_path: &str, line: i64) -> bool {
    if line <= 0 || rel_path.is_empty() {
        return false;
    }
    if cfg.bookmarks.iter().any(|bookmark| {
        bookmark.repo_id == repo_id && bookmark.rel_path == rel_path && bookmark.line == line
    }) {
        return false;
    }
    cfg.bookmarks.push(Bookmark {
        repo_id: repo_id.to_string(),
        rel_path: rel_path.to_string(),
        line,
        note: None,
    });
    true
}

fn remove_bookmark_from(cfg: &mut Config, repo_id: &str, rel_path: &str, line: i64) -> bool {
    if line <= 0 || rel_path.is_empty() {
        return false;
    }
    let Some(index) = cfg.bookmarks.iter().position(|bookmark| {
        bookmark.repo_id == repo_id && bookmark.rel_path == rel_path && bookmark.line == line
    }) else {
        return false;
    };
    cfg.bookmarks.remove(index);
    true
}

fn toggle_bookmark_in(cfg: &mut Config, repo_id: &str, rel_path: &str, line: i64) -> bool {
    remove_bookmark_from(cfg, repo_id, rel_path, line)
        || add_bookmark_to(cfg, repo_id, rel_path, line)
}

/// Command+Shift+B（非 Apple 为 Ctrl+Shift+B）：切换当前代码行书签。
pub fn toggle_current_line() {
    let Some((repo_id, rel_path, line)) = active_file_location() else {
        return;
    };
    let mut changed = false;
    app_signal().update(|s| {
        changed = toggle_bookmark_in(&mut s.cfg, &repo_id, &rel_path, line);
    });
    if changed {
        state::save_config_quiet();
    }
}

/// 弹窗里的“添加当前行”。
pub fn add_current_line() {
    let Some((repo_id, rel_path, line)) = active_file_location() else {
        return;
    };
    let mut changed = false;
    app_signal().update(|s| {
        changed = add_bookmark_to(&mut s.cfg, &repo_id, &rel_path, line);
    });
    if changed {
        state::save_config_quiet();
    }
}

/// 只删除精确匹配 repo_id + rel_path + line 的条目，绝不清理其他工作区或不可读路径。
pub fn remove_bookmark(repo_id: &str, rel_path: &str, line: i64) {
    let mut removed = false;
    app_signal().update(|s| {
        removed = remove_bookmark_from(&mut s.cfg, repo_id, rel_path, line);
    });
    if removed {
        state::save_config_quiet();
    }
}

fn jump_to_bookmark(bookmark: &Bookmark) {
    let file = app_signal().with_untracked(|s| {
        s.workspace_files
            .iter()
            .find(|file| file.repo_id == bookmark.repo_id && file.rel_path == bookmark.rel_path)
            .cloned()
    });
    if let Some(file) = file {
        state::close_active_modal();
        state::open_code_file(
            file,
            Some(EditorPosition::new(line_index(bookmark.line), 0)),
            true,
        );
    }
}

fn line_index(line: i64) -> u32 {
    u32::try_from(line.saturating_sub(1)).unwrap_or(u32::MAX)
}

pub fn editor_decorations() -> Vec<EditorDecoration> {
    decorations(None)
}

pub(crate) fn editor_decorations_for_split(
    side: crate::editor_split::PaneId,
) -> Vec<EditorDecoration> {
    decorations(Some(side))
}

fn decorations(side: Option<crate::editor_split::PaneId>) -> Vec<EditorDecoration> {
    app_signal().with(|s| {
        let tab = match side {
            Some(side) => s
                .code_split
                .as_ref()
                .and_then(|split| split.pane(side).active())
                .and_then(|id| s.code_tabs.iter().find(|tab| tab.id == id)),
            None => s.code_tabs.get(s.active_code_tab_idx),
        };
        let Some(tab) = tab.filter(|tab| tab.revision.is_empty()) else {
            return Vec::new();
        };
        let mut decorations = s.cfg
            .bookmarks
            .iter()
            .filter(|bm| {
                bm.line > 0 && bm.repo_id == tab.file.repo_id && bm.rel_path == tab.file.rel_path
            })
            .map(|bm| {
                let pos = EditorPosition::new(line_index(bm.line), 0);
                let mut decoration = EditorDecoration::default();
                decoration.range = EditorRange::new(pos, pos);
                let mut gutter = EditorGutterMark::default();
                gutter.class = Some("bg-amber-500".into());
                decoration.gutter = Some(gutter);
                decoration
            })
            .collect::<Vec<_>>();
        if let Some(lines) = s.cfg.extra.get("breakpoints")
            .and_then(|all| all[tab.file.key()].as_array()) {
            decorations.extend(lines.iter().filter_map(|line| {
                let line = line.as_u64()?.checked_sub(1)?.try_into().ok()?;
                let position = EditorPosition::new(line,0);
                let mut decoration = EditorDecoration::default();
                decoration.range = EditorRange::new(position,position);
                let mut gutter = EditorGutterMark::default();
                gutter.class = Some("bg-red-500".into());
                decoration.gutter = Some(gutter);
                Some(decoration)
            }));
        }
        decorations
    })
}

fn bookmark_matches_query(bookmark: &Bookmark, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    bookmark.repo_id.to_lowercase().contains(query)
        || bookmark.rel_path.to_lowercase().contains(query)
        || bookmark
            .note
            .as_deref()
            .map(|note| note.to_lowercase().contains(query))
            .unwrap_or(false)
        || bookmark.line.to_string().contains(query)
}

fn visible_bookmarks() -> Vec<Bookmark> {
    let query = global::<BookmarksUi>().with(|s| s.query.trim().to_lowercase());
    app_signal().with(|s| {
        let paths: HashSet<_> = s
            .workspace_files
            .iter()
            .map(|file| (file.repo_id.as_str(), file.rel_path.as_str()))
            .collect();
        s.cfg
            .bookmarks
            .iter()
            .filter(|bookmark| bookmark.line > 0 && bookmark_matches_query(bookmark, &query))
            .filter(|bookmark| {
                paths.contains(&(bookmark.repo_id.as_str(), bookmark.rel_path.as_str()))
            })
            .cloned()
            .collect()
    })
}

pub fn modal_layer() -> impl IntoElement {
    let open = computed(|| global::<BookmarksUi>().with(|s| s.open));
    dynamic(move || {
        if open.get() {
            bookmarks_modal().into_element()
        } else {
            view().into_element()
        }
    })
}

fn bookmarks_modal() -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id("bookmarks-modal")
        .absolute()
        .inset_0()
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .w(px(620.0))
                .h(px(520.0))
                .rounded(px(12.0))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .flex_col()
                .overflow_hidden()
                .child(dynamic(|| bookmarks_header(visible_bookmarks().len())))
                .child(bookmarks_filter_row())
                .child(dynamic(bookmarks_list)),
        )
}

fn bookmarks_list() -> impl IntoElement {
    let p = current_palette();
    let bookmarks = visible_bookmarks();
    scroll().flex_1().child(
        view()
            .flex_col()
            .py(px(6.0))
            .child(bookmarks.is_empty().then(|| {
                view()
                    .items_center()
                    .py(px(24.0))
                    .child(text("暂无书签").text_color(p.text_muted))
            }))
            .children(bookmarks.into_iter().map(bookmark_row)),
    )
}

fn bookmarks_header(count: usize) -> impl IntoElement {
    let p = current_palette();
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(14.0))
        .py(px(10.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .child(text("书签").text_color(p.text_primary))
                .child(view().w(px(8.0)))
                .child(text(count.to_string()).text_color(p.text_muted)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .child(action_button(
                    "添加当前行",
                    "bookmarks-add-current",
                    add_current_line,
                ))
                .child(view().w(px(6.0)))
                .child(action_button(
                    "关闭",
                    "bookmarks-close",
                    state::close_active_modal,
                )),
        )
}

fn action_button(label: &'static str, test_id: &'static str, on_click: fn()) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(test_id)
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(p.border)
        .on_press(on_click)
        .child(text(label).text_color(p.text_primary))
}

fn bookmarks_filter_row() -> impl IntoElement {
    let p = current_palette();
    view().px(px(14.0)).pb(px(8.0)).child(
        input()
            .test_id("bookmarks-filter")
            .auto_focus(true)
            .h(px(28.0))
            .border_1()
            .border_color(p.border)
            .rounded(px(6.0))
            .px(px(8.0))
            .text_color(p.text_primary)
            .placeholder("搜索路径…")
            .value(|| global::<BookmarksUi>().with(|s| s.query.clone()))
            .on_change(|value: &str| {
                global::<BookmarksUi>().update(|s| s.query = value.to_string())
            })
            .on_focus(|| state::set_text_input_focused(true))
            .on_blur(|| state::set_text_input_focused(false)),
    )
}

fn bookmark_row(bookmark: Bookmark) -> impl IntoElement {
    let p = current_palette();
    let repo_id = bookmark.repo_id.clone();
    let rel_path = bookmark.rel_path.clone();
    let line = bookmark.line;
    let jump_target = bookmark.clone();
    let remove_repo = repo_id.clone();
    let remove_rel = rel_path.clone();
    let location = format!("{}:{}", rel_path, line);
    let subtitle = bookmark.note.clone().unwrap_or_else(|| repo_id.clone());
    view()
        .test_id("bookmark-row")
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .child(
            view()
                .flex_col()
                .flex_1()
                .child(text(location).text_color(p.text_primary))
                .child(text(subtitle).text_color(p.text_muted)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .child(
                    view()
                        .test_id(&format!("bookmark-jump-{repo_id}-{rel_path}-{line}"))
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(p.border)
                        .on_press(move || jump_to_bookmark(&jump_target))
                        .child(text("跳转").text_color(p.text_primary)),
                )
                .child(view().w(px(6.0)))
                .child(
                    view()
                        .test_id(&format!("bookmark-remove-{repo_id}-{rel_path}-{line}"))
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(p.border)
                        .on_press(move || remove_bookmark(&remove_repo, &remove_rel, line))
                        .child(text("移除").text_color(p.text_muted)),
                ),
        )
}
