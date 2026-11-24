use std::collections::HashSet;

use nsh_base::input;
use nsh_core::prelude::*;

use crate::paths::basename;
use crate::state::{
    self, AppState, EditorTabKind, FocusZone, LeftTab, NavTarget, app_signal, current_palette,
};

const MAX_VISIBLE: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Everywhere,
    Files,
    Actions,
    RecentFiles,
    Switcher,
}

impl Mode {
    fn accepts_text(self) -> bool {
        !matches!(self, Mode::Switcher)
    }

    fn is_category(self) -> bool {
        matches!(self, Mode::Everywhere | Mode::Files | Mode::Actions)
    }

    fn title(self) -> &'static str {
        match self {
            Mode::Everywhere => "随处搜索",
            Mode::Files => "随处搜索 · 文件",
            Mode::Actions => "查找操作",
            Mode::RecentFiles => "最近文件",
            Mode::Switcher => "切换器",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionId {
    FocusCommit,
    FocusProject,
    FocusGit,
    FindInFiles,
    ReplaceInFiles,
    Structure,
    Todos,
    Bookmarks,
    GoToFile,
    RecentLocations,
    LocalHistory,
    Refresh,
    FormatCode,
    FindResults,
    Terminal,
    Run,
    RunConfig,
    Logs,
    GitHub,
}

struct ActionDef {
    id: ActionId,
    title: &'static str,
    hint: &'static str,
    keywords: &'static str,
}

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: ActionId::FocusCommit,
        title: "聚焦提交工具窗口",
        hint: "⌘0",
        keywords: "commit 提交 focus tool window",
    },
    ActionDef {
        id: ActionId::FocusProject,
        title: "聚焦项目工具窗口",
        hint: "⌘1",
        keywords: "project 项目 focus tool window",
    },
    ActionDef {
        id: ActionId::FocusGit,
        title: "聚焦 Git 工具窗口",
        hint: "⌘9",
        keywords: "git focus tool window 提交历史",
    },
    ActionDef {
        id: ActionId::FindInFiles,
        title: "在文件中查找",
        hint: "find in files",
        keywords: "find search 查找 files 文件",
    },
    ActionDef {
        id: ActionId::ReplaceInFiles,
        title: "在文件中替换",
        hint: "replace in files",
        keywords: "replace 替换 files 文件",
    },
    ActionDef {
        id: ActionId::Structure,
        title: "编辑器结构",
        hint: "structure",
        keywords: "structure outline symbols 结构 大纲 符号",
    },
    ActionDef {
        id: ActionId::Todos,
        title: "TODO",
        hint: "todo comments",
        keywords: "todo comments 待办 注释",
    },
    ActionDef {
        id: ActionId::Bookmarks,
        title: "行书签",
        hint: "line bookmarks",
        keywords: "bookmarks lines 书签 行",
    },
    ActionDef {
        id: ActionId::GoToFile,
        title: "打开文件…",
        hint: "⌘⇧O",
        keywords: "open file goto 打开 文件",
    },
    ActionDef {
        id: ActionId::RecentLocations,
        title: "最近位置…",
        hint: "⌘⇧E",
        keywords: "recent locations 最近 位置",
    },
    ActionDef {
        id: ActionId::LocalHistory,
        title: "本地历史",
        hint: "local history",
        keywords: "local history 本地 历史",
    },
    ActionDef {
        id: ActionId::Refresh,
        title: "刷新工作区",
        hint: "refresh",
        keywords: "refresh reload workspace 刷新 重载 工作区",
    },
    ActionDef {
        id: ActionId::FormatCode,
        title: "格式化代码",
        hint: "format code",
        keywords: "format code 格式化 代码",
    },
    ActionDef { id: ActionId::Terminal, title: "终端", hint: "", keywords: "terminal shell PTY" },
    ActionDef { id: ActionId::Run, title: "运行 / 调试 / 测试", hint: "", keywords: "run test debug DAP" },
    ActionDef { id: ActionId::RunConfig, title: "新建运行配置", hint: "", keywords: "run config" },
    ActionDef { id: ActionId::Logs, title: "日志", hint: "", keywords: "logs command runtime" },
    ActionDef { id: ActionId::GitHub, title: "GitHub PR / CI", hint: "", keywords: "pull request checks" },
    ActionDef {
        id: ActionId::FindResults,
        title: "查找结果",
        hint: "find results",
        keywords: "find results 查找 结果",
    },
];

#[derive(Clone, Default)]
struct Picker {
    mode: Option<Mode>,
    workspace: String,
    query: String,
    selected: usize,
}

#[derive(Clone)]
enum Destination {
    Tab(String),
    File { repo: String, path: String },
    Location(NavTarget),
    Action(ActionId),
}

#[derive(Clone)]
struct Row {
    title: String,
    detail: String,
    destination: Destination,
}

fn signal() -> Signal<Picker> {
    global::<Picker>()
}

pub fn open(mode: Mode) {
    let (workspace, focus) =
        app_signal().with_untracked(|s| (s.cfg.workspace_path.clone(), s.focus_zone));
    if focus == FocusZone::Modal {
        return;
    }
    let query = if mode == Mode::Everywhere {
        state::active_editor_api(focus)
            .map(|editor| editor.selected_text())
            .filter(|text| !text.contains(['\r', '\n']) && text.chars().count() <= 200)
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    signal().set(Picker {
        mode: Some(mode),
        workspace,
        query,
        selected: 0,
    });
    app_signal().update(state::enter_modal_focus);
    state::set_text_input_focused(mode.accepts_text());
}

pub fn start_or_move_switcher(reverse: bool) {
    let delta = if reverse { -1 } else { 1 };
    if is_switcher() {
        move_selection(delta);
        return;
    }
    open(Mode::Switcher);
    if is_switcher() && app_signal().with_untracked(|s| s.editor_tab == EditorTabKind::File) {
        let files = current_rows()
            .iter()
            .take_while(|row| !matches!(row.destination, Destination::Action(_)))
            .count();
        if files > 1 {
            signal().update(|s| s.selected = if reverse { files - 1 } else { 1 });
        }
    }
}

pub fn cycle_tabs(delta: isize) {
    let destination = app_signal().with_untracked(|s| {
        let active = s.code_tabs.get(s.active_code_tab_idx)?;
        let ids: Vec<_> = if let Some(split) = &s.code_split {
            split
                .pane(split.focused)
                .tabs()
                .iter()
                .map(String::as_str)
                .collect()
        } else {
            s.code_tabs.iter().map(|tab| tab.id.as_str()).collect()
        };
        let index = ids.iter().position(|id| *id == active.id)? as isize;
        Some(Destination::Tab(
            ids[(index + delta).rem_euclid(ids.len() as isize) as usize].to_string(),
        ))
    });
    if let Some(destination) = destination {
        activate(destination);
    }
}

pub fn release_control() {
    if is_switcher() {
        accept();
    }
}

pub fn is_open() -> bool {
    signal().with(|s| s.mode.is_some())
}

pub fn is_switcher() -> bool {
    signal().with(|s| s.mode == Some(Mode::Switcher))
}

pub fn close_active() -> bool {
    let state = signal();
    if state.with_untracked(|s| s.mode.is_none()) {
        return false;
    }
    state.set(Picker::default());
    true
}

pub fn accept() {
    let selected = signal().with_untracked(|s| s.selected);
    if let Some(row) = current_rows().into_iter().nth(selected) {
        activate(row.destination);
    }
}

pub fn move_selection(delta: isize) {
    let count = current_rows().len();
    if count != 0 {
        signal().update(|s| {
            s.selected = (s.selected as isize + delta).rem_euclid(count as isize) as usize;
        });
    }
}

fn matches(q: &str, fields: &[&str]) -> bool {
    if q.is_empty() {
        return true;
    }
    let haystack = fields.join(" ").to_lowercase();
    q.split_whitespace().all(|word| haystack.contains(word))
}

fn current_rows() -> Vec<Row> {
    let (mode, workspace, query) =
        signal().with(|s| (s.mode, s.workspace.clone(), s.query.trim().to_lowercase()));
    let Some(mode) = mode else { return Vec::new() };
    app_signal().with(|s| {
        if s.cfg.workspace_path != workspace {
            return Vec::new();
        }
        let mut rows = Vec::new();
        if matches!(mode, Mode::Switcher | Mode::RecentFiles) {
            push_recent(s, mode, &query, &mut rows);
        }
        if matches!(mode, Mode::Everywhere | Mode::Actions | Mode::Switcher) {
            let has_find_results = super::content_search::find_panel_available_in(&workspace);
            for action in ACTIONS {
                if mode == Mode::Switcher
                    && !matches!(
                        action.id,
                        ActionId::FocusCommit
                            | ActionId::FocusProject
                            | ActionId::FocusGit
                            | ActionId::FindResults
                    )
                {
                    continue;
                }
                if (action.id != ActionId::FindResults || has_find_results)
                    && matches(&query, &[action.title, action.hint, action.keywords])
                {
                    rows.push(Row {
                        title: action.title.into(),
                        detail: action.hint.into(),
                        destination: Destination::Action(action.id),
                    });
                }
            }
        }
        if matches!(mode, Mode::Everywhere | Mode::Files) {
            rows.extend(
                s.workspace_files
                    .iter()
                    .filter(|file| matches(&query, &[&file.rel_path, &file.repo_name]))
                    .take(MAX_VISIBLE.saturating_sub(rows.len()))
                    .map(|file| Row {
                        title: basename(&file.rel_path),
                        detail: format!("{} / {}", file.repo_name, file.rel_path),
                        destination: Destination::File {
                            repo: file.repo_id.clone(),
                            path: file.rel_path.clone(),
                        },
                    }),
            );
        }
        rows.truncate(MAX_VISIBLE);
        rows
    })
}

fn push_recent(s: &AppState, mode: Mode, query: &str, rows: &mut Vec<Row>) {
    let tab_row = |tab: &state::CodeTab| Row {
        title: tab.title(),
        detail: tab.file.rel_path.clone(),
        destination: Destination::Tab(tab.id.clone()),
    };
    let mut seen = HashSet::new();
    if mode == Mode::Switcher
        && s.editor_tab == EditorTabKind::File
        && let Some(tab) = s.code_tabs.get(s.active_code_tab_idx)
    {
        seen.insert(tab.id.clone());
        rows.push(tab_row(tab));
    }
    for nav in &s.recent_locations {
        let NavTarget::CodeFile {
            repo_id, rel_path, revision, ..
        } = nav
        else {
            continue;
        };
        let id = state::code_tab_id(repo_id, rel_path, revision);
        if !matches(query, &[repo_id, rel_path, revision]) || !seen.insert(id) {
            continue;
        }
        let exists = if revision.is_empty() {
            s.workspace_files.iter().any(|file| file.repo_id == *repo_id && file.rel_path == *rel_path)
        } else {
            s.repos.iter().any(|repo| repo.id == *repo_id)
        };
        if !exists {
            continue;
        }
        rows.push(Row {
            title: if revision.is_empty() { basename(rel_path) } else {
                format!("{} ({})", basename(rel_path), revision.get(..8).unwrap_or(revision))
            },
            detail: format!("{repo_id} / {rel_path}"),
            destination: Destination::Location(nav.clone()),
        });
    }
    if mode == Mode::Switcher {
        rows.extend(
            s.code_tabs
                .iter()
                .filter(|tab| seen.insert(tab.id.clone()))
                .map(tab_row),
        );
        rows.truncate(MAX_VISIBLE - 4);
    }
}

fn activate(destination: Destination) {
    let workspace = signal().with_untracked(|s| s.workspace.clone());
    if is_open() {
        state::close_active_modal();
        if !app_signal().with_untracked(|s| s.cfg.workspace_path == workspace) {
            return;
        }
    }
    match destination {
        Destination::Tab(id) => {
            state::cancel_diff_read();
            state::capture_editor_views();
            app_signal().update(|s| {
                if let Some(index) = s.code_tabs.iter().position(|tab| tab.id == id) {
                    crate::code_split_runtime::select_tab_in_model(s, index);
                    state::sync_dirty_working_diff_into_code_tab(s, index);
                    s.editor_tab = EditorTabKind::File;
                    s.focus_zone = FocusZone::CodeEditor;
                }
            });
            state::focus_active_editor_ref();
        }
        Destination::File { repo, path } => {
            let file = app_signal().with_untracked(|s| {
                s.workspace_files
                    .iter()
                    .find(|file| file.repo_id == repo && file.rel_path == path)
                    .cloned()
            });
            if let Some(file) = file {
                state::open_code_file(file, None, true);
            }
        }
        Destination::Location(nav) => state::restore_nav_target(nav),
        Destination::Action(action) => execute(action),
    }
}

fn open_legacy_picker(kind: state::PickerKind) {
    app_signal().update(|s| {
        s.picker_open = Some(kind);
        s.picker_query.clear();
        state::enter_modal_focus(s);
    });
}

fn execute(action: ActionId) {
    match action {
        ActionId::FocusCommit => super::focus_tool_window(LeftTab::Commit, false),
        ActionId::FocusProject => super::focus_tool_window(LeftTab::Project, false),
        ActionId::FocusGit => {
            super::content_search::hide_find_panel();
            app_signal().update(|s| {
                s.show_git_panel = true;
                s.focus_zone = FocusZone::GitFiles;
            });
        }
        ActionId::FindInFiles => super::content_search::open_workspace(),
        ActionId::ReplaceInFiles => super::content_search::open_replace(),
        ActionId::Structure => super::editor_tools::open_structure(),
        ActionId::Todos => super::editor_tools::open_todos(),
        ActionId::Bookmarks => super::bookmarks::open_modal(),
        ActionId::GoToFile => open_legacy_picker(state::PickerKind::GoToFile),
        ActionId::RecentLocations => open_legacy_picker(state::PickerKind::RecentLocations),
        ActionId::LocalHistory => state::open_local_history_for_active(),
        ActionId::Refresh => state::refresh_workspace_async(None),
        ActionId::FormatCode => crate::format_runtime::format_active(),
        ActionId::FindResults => super::content_search::show_find_panel(),
        ActionId::Terminal => super::terminal_panel::toggle(),
        ActionId::Run => super::run_panel::open(),
        ActionId::RunConfig => { super::run_panel::open(); super::run_panel::edit(true); }
        ActionId::Logs => super::workbench::reveal(super::workbench::Panel::Logs),
        ActionId::GitHub => super::github_panel::open(),
    }
}

pub fn modal_layer() -> impl IntoElement {
    let kind = computed(|| signal().with(|s| s.mode.map(|mode| mode == Mode::Switcher)));
    dynamic(move || match kind.get() {
        Some(switcher) => picker_modal(switcher).into_element(),
        None => view().into_element(),
    })
}

fn picker_modal(switcher: bool) -> impl IntoElement {
    let p = current_palette();
    let frame = element_ref();
    if switcher {
        after_answer(move || frame.focus());
    }
    view()
        .test_id("navigation-picker-modal")
        .absolute()
        .inset_0()
        .p(px(14.0))
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .element_ref(&frame)
                .focusable(switcher)
                .w_full()
                .max_w(px(if switcher { 560.0 } else { 720.0 }))
                .h_full()
                .max_h(px(if switcher { 420.0 } else { 560.0 }))
                .rounded(px(12.0))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .flex_col()
                .overflow_hidden()
                .child(
                    view()
                        .h(px(38.0))
                        .px(px(14.0))
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            text(|| {
                                signal().with(|s| {
                                    s.mode.map(Mode::title).unwrap_or_default().to_string()
                                })
                            })
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.text_primary),
                        )
                        .child(
                            text("关闭")
                                .text_size(px(11.0))
                                .text_color(p.text_control)
                                .on_press(state::close_active_modal),
                        ),
                )
                .child((!switcher).then(query_bar))
                .child(dynamic(row_list)),
        )
}

fn query_bar() -> impl IntoElement {
    let p = current_palette();
    let categories = computed(|| signal().with(|s| s.mode.is_some_and(Mode::is_category)));
    view()
        .px(px(14.0))
        .gap(px(6.0))
        .flex_col()
        .child(
            input()
                .test_id("navigation-picker-input")
                .auto_focus(true)
                .h(px(30.0))
                .px(px(8.0))
                .bg(p.rail_bg)
                .border_1()
                .border_color(p.border)
                .rounded(px(5.0))
                .text_color(p.text_primary)
                .placeholder("输入名称或路径…")
                .value(|| signal().with(|s| s.query.clone()))
                .on_change(|value: &str| {
                    signal().update(|s| {
                        s.query = value.to_string();
                        s.selected = 0;
                    })
                })
                .on_submit(|_: &str| accept()),
        )
        .child(when(
            move || categories.get(),
            move || {
                view().flex_row().gap(px(6.0)).children(
                    [
                        ("全部", Mode::Everywhere),
                        ("文件", Mode::Files),
                        ("操作", Mode::Actions),
                    ]
                    .into_iter()
                    .map(|(label, mode)| {
                        view()
                            .h(px(24.0))
                            .px(px(8.0))
                            .rounded(px(4.0))
                            .cursor(Cursor::Pointer)
                            .bg(move || {
                                if signal().with(|s| s.mode == Some(mode)) {
                                    p.accent_soft
                                } else {
                                    p.rail_bg
                                }
                            })
                            .on_press(move || {
                                signal().update(|s| {
                                    s.mode = Some(mode);
                                    s.selected = 0;
                                })
                            })
                            .child(text(label).text_size(px(11.0)).text_color(p.text_control))
                    }),
                )
            },
        ))
}

fn row_list() -> impl IntoElement {
    let rows = current_rows();
    let selected = signal().with(|s| s.selected);
    let p = current_palette();
    scroll().flex_1().child(
        view()
            .py(px(6.0))
            .flex_col()
            .child(rows.is_empty().then(|| {
                view()
                    .py(px(18.0))
                    .items_center()
                    .child(text("无匹配结果").text_color(p.text_muted))
            }))
            .children(rows.into_iter().enumerate().map(move |(index, row)| {
                view()
                    .test_id(&format!("navigation-picker-row-{index}"))
                    .h(px(31.0))
                    .px(px(14.0))
                    .gap(px(8.0))
                    .flex_row()
                    .items_center()
                    .cursor(Cursor::Pointer)
                    .bg(if index == selected {
                        p.accent_soft
                    } else {
                        p.surface
                    })
                    .hover(move |style| style.bg(p.surface_hover))
                    .on_press(move || activate(row.destination.clone()))
                    .child(
                        text(row.title)
                            .text_size(px(12.0))
                            .text_color(p.text_primary),
                    )
                    .child(view().flex_1())
                    .child(
                        text(row.detail)
                            .text_size(px(10.5))
                            .text_color(p.text_muted)
                            .number_of_lines(1u32),
                    )
            })),
    )
}
