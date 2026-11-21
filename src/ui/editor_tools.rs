use std::collections::HashMap;
use std::sync::OnceLock;

use nsh_base::{EditorPosition, input};
use nsh_core::prelude::*;
use regex::Regex;

use crate::git::WorkspaceFile;
use crate::search::{CancelToken, SearchOptions, search_files};
use crate::state::{self, EditorTabKind, FocusZone, app_signal, current_palette};

pub const DEFAULT_TODO_PATTERN: &str = "TODO|FIXME|HACK|XXX";
const MAX_STRUCTURE_ITEMS: usize = 800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureItem {
    pub name: String,
    pub kind: String,
    pub line: usize,
    pub depth: usize,
}

#[derive(Debug, Clone)]
struct StructureView {
    tab_id: String,
    title: String,
    items: Vec<StructureItem>,
}

#[derive(Debug, Clone)]
struct TodoItem {
    file: WorkspaceFile,
    repo_name: String,
    rel_path: String,
    line: usize,

    text: String,
}

#[derive(Clone, Default)]
struct EditorToolsState {
    structure: Option<StructureView>,
    todos_open: bool,
    todos_loading: bool,
    todos_error: Option<String>,
    todos: Vec<TodoItem>,
    todo_query: String,
    todo_generation: u64,
    todo_cancel: Option<CancelToken>,
    todo_workspace: String,
}

fn tools_signal() -> Signal<EditorToolsState> {
    global::<EditorToolsState>()
}

fn go_structure() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\s*)(func|type)\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)")
            .expect("valid Go structure regex")
    })
}

fn js_structure() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\s*)(?:export\s+)?(?:async\s+)?(function|class|interface|enum|const|let|var)\s+([A-Za-z_$][\w$]*)")
            .expect("valid JS structure regex")
    })
}

fn py_structure() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\s*)(?:async\s+)?(def|class)\s+([A-Za-z_]\w*)")
            .expect("valid Python structure regex")
    })
}

fn java_structure() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\s*)(?:(?:public|protected|private|static|final|abstract|synchronized|native|default)\s+)*(class|interface|enum|record|[A-Za-z_][\w<>\[\], ?]*)\s+([A-Za-z_]\w*)\s*[({;]")
            .expect("valid Java structure regex")
    })
}

fn rs_structure() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\s*)(?:pub\s+)?(fn|struct|enum|trait|impl|mod)\s+([A-Za-z_]\w*)")
            .expect("valid Rust structure regex")
    })
}

fn structure_regex(path: &str) -> Option<&'static Regex> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "go" => Some(go_structure()),
        "js" | "jsx" | "ts" | "tsx" => Some(js_structure()),
        "py" => Some(py_structure()),
        "java" | "kt" | "kts" | "cs" | "scala" => Some(java_structure()),
        "rs" => Some(rs_structure()),
        _ => None,
    }
}

pub fn parse_structure(path: &str, content: &str) -> Vec<StructureItem> {
    let Some(re) = structure_regex(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, line) in content.split('\n').enumerate() {
        if out.len() >= MAX_STRUCTURE_ITEMS {
            break;
        }
        let Some(caps) = re.captures(line) else {
            continue;
        };
        let kind = caps.get(2).map(|value| value.as_str()).unwrap_or_default();
        let name = caps.get(3).map(|value| value.as_str()).unwrap_or_default();
        if kind.is_empty() || name.is_empty() {
            continue;
        }
        if matches!(kind, "const" | "let" | "var")
            && !line.contains("function")
            && !line.contains("=>")
        {
            continue;
        }
        let indent = caps.get(1).map(|value| value.as_str()).unwrap_or_default();
        let mut width = 0usize;
        for ch in indent.chars() {
            width += if ch == '\t' { 4 } else { 1 };
        }
        out.push(StructureItem {
            name: name.to_string(),
            kind: kind.to_string(),
            line: index + 1,
            depth: width / 2,
        });
    }
    out
}

pub fn open_structure() {
    let active = app_signal().with_untracked(|s| {
        if s.editor_tab != EditorTabKind::File {
            return None;
        }
        s.code_tabs
            .get(s.active_code_tab_idx)
            .map(|tab| (tab.id.clone(), tab.file.rel_path.clone(), tab.text.clone()))
    });
    let Some((tab_id, path, text)) = active else {
        state::show_toast("warn", "先打开一个代码文件");
        return;
    };
    let items = parse_structure(&path, &text);
    let title = crate::paths::basename(&path).to_string();
    tools_signal().update(|s| {
        s.structure = Some(StructureView {
            tab_id,
            title,
            items,
        });
    });
    app_signal().update(|s| state::enter_modal_focus(s));
}

pub fn open_todos() {
    tools_signal().update(|s| {
        if s.todo_query.trim().is_empty() {
            s.todo_query = DEFAULT_TODO_PATTERN.to_string();
        }
        s.todos_open = true;
        s.todos_error = None;
    });
    app_signal().update(|s| state::enter_modal_focus(s));
    refresh_todos();
}

fn refresh_todos() {
    let query = tools_signal().with_untracked(|s| s.todo_query.trim().to_string());
    let (workspace, files) =
        app_signal().with_untracked(|s| (s.cfg.workspace_path.clone(), s.workspace_files.clone()));
    let cancel = CancelToken::new();
    let mut generation = 0;
    tools_signal().update(|s| {
        if let Some(old) = s.todo_cancel.replace(cancel.clone()) {
            old.cancel();
        }
        s.todo_generation = s.todo_generation.wrapping_add(1);
        generation = s.todo_generation;
        s.todo_workspace.clone_from(&workspace);
        s.todos_loading = true;
        s.todos_error = None;
        s.todos.clear();
    });
    let Some(owner) = global::<state::RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    owner.run(|| {
        spawn(async move {
            let results = blocking(move || {
                let options = SearchOptions {
                    regex: true,
                    ..Default::default()
                };
                let hits = search_files(&files, None, &query, &options, &cancel)?.hits;
                if cancel.is_cancelled() {
                    return Ok(Vec::new());
                }
                let by_key: HashMap<_, _> =
                    files.into_iter().map(|file| (file.key(), file)).collect();
                Ok::<_, crate::search::SearchError>(
                    hits.into_iter()
                        .filter_map(|hit| {
                            let key = format!("{}::{}", hit.repo_id, hit.rel_path);
                            Some(TodoItem {
                                file: by_key.get(&key)?.clone(),
                                repo_name: hit.repo_name,
                                rel_path: hit.rel_path,
                                line: hit.line,
                                text: hit.snippet.trim().to_string(),
                            })
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .await;
            let current = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
            if current != workspace {
                return;
            }
            tools_signal().update(|s| {
                if s.todo_generation != generation || !s.todos_open {
                    return;
                }
                s.todos_loading = false;
                s.todo_cancel = None;
                match results {
                    Ok(Ok(items)) => s.todos = items,
                    Ok(Err(err)) => s.todos_error = Some(format!("无法扫描待办：{err}")),
                    Err(err) => s.todos_error = Some(format!("扫描任务中止：{err}")),
                }
            });
        })
    });
}

pub fn close_active() -> bool {
    let mut closed = false;
    tools_signal().update(|s| {
        if s.structure.take().is_some() {
            closed = true;
        }
        if s.todos_open {
            s.todos_open = false;
            s.todos_loading = false;
            s.todo_generation = s.todo_generation.wrapping_add(1);
            if let Some(cancel) = s.todo_cancel.take() {
                cancel.cancel();
            }
            closed = true;
        }
    });
    closed
}

fn activate_structure(line: usize) {
    let target =
        tools_signal().with_untracked(|s| s.structure.as_ref().map(|view| view.tab_id.clone()));
    state::close_active_modal();
    let active = app_signal().with_untracked(|s| {
        s.code_tabs
            .get(s.active_code_tab_idx)
            .map(|tab| tab.id.clone())
    });
    if target.is_some() && active == target {
        let pos = EditorPosition::new(line.saturating_sub(1) as u32, 0);
        app_signal().update(|s| s.focus_zone = FocusZone::CodeEditor);
        state::reveal_in_code_editor(pos);
        state::focus_active_editor_ref();
    }
}

fn activate_todo(index: usize) {
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let target = tools_signal().with_untracked(|s| {
        (s.todo_workspace == workspace)
            .then(|| s.todos.get(index).cloned())
            .flatten()
    });
    if let Some(item) = target {
        state::close_active_modal();
        let row = item.line.saturating_sub(1) as u32;
        state::open_code_file(item.file, Some(EditorPosition::new(row, 0)), true);
    }
}

pub fn modal_layer() -> impl IntoElement {
    let modal = computed(|| tools_signal().with(|s| (s.structure.is_some(), s.todos_open)));
    dynamic(move || match modal.get() {
        (true, _) => structure_modal().into_element(),
        (false, true) => todos_modal().into_element(),
        (false, false) => view().into_element(),
    })
}

fn structure_modal() -> impl IntoElement {
    let p = current_palette();
    let snapshot = tools_signal().with(|s| s.structure.clone());
    let (title, items) = match snapshot {
        Some(view) => (view.title, view.items),
        None => ("结构".to_string(), Vec::new()),
    };
    let mut list = view().flex_1().flex_col().py(px(6.0));
    if items.is_empty() {
        list = list.child(
            view()
                .items_center()
                .justify_center()
                .py(px(24.0))
                .child(text("没有可识别的结构").text_color(p.text_muted)),
        );
    } else {
        for item in items {
            list = list.child(structure_row(item));
        }
    }
    view()
        .test_id("structure-modal")
        .absolute()
        .inset_0()
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .w(px(480.0))
                .h(px(440.0))
                .rounded(px(12.0))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .flex_col()
                .overflow_hidden()
                .child(structure_header(title))
                .child(scroll().flex_1().child(list)),
        )
}

fn structure_header(title: String) -> impl IntoElement {
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
                .child(text("结构").text_color(p.text_primary))
                .child(view().w(px(8.0)))
                .child(text(title).text_color(p.text_muted)),
        )
        .child(close_button())
}

fn structure_row(item: StructureItem) -> impl IntoElement {
    let p = current_palette();
    let line = item.line;
    let indent = "  ".repeat(item.depth.min(8));
    view()
        .test_id(&format!("structure-row-{line}"))
        .flex_row()
        .items_center()
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(3.0))
        .on_press(move || activate_structure(line))
        .child(
            view()
                .w(px(56.0))
                .child(text(item.kind).text_color(p.text_muted)),
        )
        .child(
            view()
                .flex_1()
                .child(text(format!("{indent}{}", item.name)).text_color(p.text_primary)),
        )
        .child(
            view()
                .w(px(48.0))
                .child(text(line.to_string()).text_color(p.text_muted)),
        )
}

fn todos_modal() -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id("todos-modal")
        .absolute()
        .inset_0()
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .w(px(740.0))
                .h(px(600.0))
                .rounded(px(12.0))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .flex_col()
                .overflow_hidden()
                .child(dynamic(move || todo_header()))
                .child(todo_query_row())
                .child(dynamic(move || todo_body())),
        )
}

fn todo_header() -> impl IntoElement {
    let p = current_palette();
    let (count, loading) = tools_signal().with(|s| (s.todos.len(), s.todos_loading));
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
                .child(text("TODO").text_color(p.text_primary))
                .child(view().w(px(8.0)))
                .child(text(count.to_string()).text_color(p.text_muted)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .child(when(move || loading, scan_indicator))
                .child(when(move || !loading, scan_button))
                .child(view().w(px(6.0)))
                .child(close_button()),
        )
}

fn scan_indicator() -> impl IntoElement {
    let p = current_palette();
    text("扫描中…").text_color(p.text_muted)
}

fn scan_button() -> impl IntoElement {
    let p = current_palette();
    view()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .test_id("todo-scan")
        .on_press(refresh_todos)
        .child(text("重新扫描").text_color(p.text_primary))
}

fn todo_query_row() -> impl IntoElement {
    view().px(px(14.0)).pb(px(8.0)).child(
        input()
            .test_id("todo-query")
            .value(move || tools_signal().with(|s| s.todo_query.clone()))
            .placeholder(DEFAULT_TODO_PATTERN)
            .on_focus(|| state::set_text_input_focused(true))
            .on_blur(|| state::set_text_input_focused(false))
            .on_change(|value: &str| tools_signal().update(|s| s.todo_query = value.into()))
            .on_submit(|_: &str| refresh_todos()),
    )
}

fn todo_body() -> impl IntoElement {
    let p = current_palette();
    let (items, loading, error) =
        tools_signal().with(|s| (s.todos.clone(), s.todos_loading, s.todos_error.clone()));
    let mut body = view().flex_1().flex_col().px(px(8.0)).pb(px(8.0));
    if items.is_empty() {
        if let Some(message) = error {
            body = body.child(
                view()
                    .px(px(14.0))
                    .py(px(6.0))
                    .child(text(message).text_color(p.danger)),
            );
        } else if loading {
            body = body.child(
                view()
                    .items_center()
                    .justify_center()
                    .py(px(24.0))
                    .child(text("扫描中…").text_color(p.text_muted)),
            );
        } else {
            body = body.child(
                view()
                    .items_center()
                    .justify_center()
                    .py(px(24.0))
                    .child(text("没有匹配项").text_color(p.text_muted)),
            );
        }
    }
    for (index, item) in items.iter().enumerate() {
        body = body.child(todo_row(index, item.clone()));
    }
    scroll().flex_1().child(body)
}

fn todo_row(index: usize, item: TodoItem) -> impl IntoElement {
    let p = current_palette();
    view()
        .test_id(&format!("todo-row-{index}"))
        .flex_row()
        .items_center()
        .px(px(6.0))
        .py(px(4.0))
        .rounded(px(3.0))
        .on_press(move || activate_todo(index))
        .child(
            view()
                .w(px(90.0))
                .child(text(item.repo_name).text_color(p.text_muted)),
        )
        .child(
            view().w(px(240.0)).child(
                text(format!("{}:{}", item.rel_path, item.line)).text_color(p.text_secondary),
            ),
        )
        .child(
            view()
                .flex_1()
                .child(text(item.text).text_color(p.text_primary)),
        )
}

fn close_button() -> impl IntoElement {
    let p = current_palette();
    view()
        .w(px(22.0))
        .h(px(22.0))
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .on_press(state::close_active_modal)
        .child(text("×").text_color(p.text_muted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_structure_shapes() {
        let rust = parse_structure(
            "src/main.rs",
            "pub fn start() {}\nimpl App {\n    fn run(&self) {}\n}\n",
        );
        assert_eq!(rust.len(), 3);
        assert_eq!(rust[0].name, "start");
        assert_eq!(rust[0].line, 1);
        assert_eq!(rust[1].kind, "impl");
        assert_eq!(rust[2].line, 3);

        let js = parse_structure(
            "app.ts",
            "export const value = 1\nexport const fn = () => 1\n",
        );
        assert_eq!(js.len(), 1);
        assert_eq!(js[0].name, "fn");
    }

    #[test]
    fn default_todo_pattern_is_valid() {
        assert!(Regex::new(DEFAULT_TODO_PATTERN).is_ok());
    }
}
