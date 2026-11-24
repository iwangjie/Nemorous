//! Batch 2「在文件中查找」模态：基于 `crate::search` 的全文搜索 UI。
//!
//! 搜索状态存放在 NSH 运行时全局信号里（`global::<ContentSearchState>()`，每个引擎/运行时一份），
//! 不占用 `AppState`；关闭时由 `state::close_active_modal` 优先关闭并恢复键盘所有权。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nsh_base::{
    Editor, EditorApi, EditorDecoration, EditorPosition, EditorRange, editor, h_resizable, input,
    resizable_panel, v_resizable,
};
use nsh_core::prelude::*;

use crate::git::WorkspaceFile;
use crate::replace_in_files::ReplaceQuery;

use super::replacement_flow::{self, Phase};
use crate::search::{
    CancelToken, SearchError, SearchHit, SearchOptions, SearchOutcome, SearchStatus, search_files,
};
use crate::state::{self, app_signal, current_palette};
use crate::theme::Palette;
use crate::ui::icons::{self, ICO_CLOSE, ICO_COLUMNS};

/// 每次按键后等待多久才真正开始扫描。
const DEBOUNCE: Duration = Duration::from_millis(220);
const MAX_INITIAL_QUERY_CHARS: usize = 200;

#[derive(Clone, Default)]
struct ContentSearchState {
    open: bool,
    /// 每次查询条件变化自增；只有代次相同的任务才能写回结果。
    generation: u64,
    /// 打开时所在的工作区；跨工作区的旧结果与旧命中都不再生效。
    workspace: String,
    query: String,
    multiline: bool,
    replace_mode: bool,
    replacement: String,
    case_sensitive: bool,
    regex: bool,
    whole_word: bool,
    mask_enabled: bool,
    mask: String,
    scope_dir: Option<PathBuf>,
    scope_label: String,
    /// 打开弹窗时一次性快照的工作区文件索引，避免每次按键重复枚举。
    index: Option<Arc<Vec<WorkspaceFile>>>,
    running: bool,
    hits: Arc<[SearchHit]>,
    truncated: bool,
    selected: usize,
    error: Option<String>,
    cancel: CancelToken,
}

/// 与 `AppState`/`EditorRefs` 一致：每个引擎一份全局信号，不用 thread_local。
fn sig() -> Signal<ContentSearchState> {
    global::<ContentSearchState>()
}

fn with_state<R>(f: impl FnOnce(&ContentSearchState) -> R) -> R {
    sig().with(f)
}

fn update_state(f: impl FnOnce(&mut ContentSearchState)) {
    sig().update(f);
}

// ============================================================================
// 查找结果面板：把一次稳定搜索钉进底部面板，与模态后续活动完全解耦
// ============================================================================

/// 面板快照：查询、命中以及命中解析所需的文件索引都在钉入时一次性克隆，
/// 并绑定钉入时的工作区；工作区变化后既不显示，也不再打开旧命中。
#[derive(Clone)]
struct FindResultsView {
    workspace: String,
    query: String,
    hits: Arc<[SearchHit]>,
    index: Arc<Vec<WorkspaceFile>>,
}

#[derive(Clone, Default)]
struct FindResultsState {
    open: bool,
    view: Option<FindResultsView>,
    selected: usize,
}

fn find_sig() -> Signal<FindResultsState> {
    global::<FindResultsState>()
}

pub fn find_panel_available() -> bool {
    let workspace = app_signal().with(|s| s.cfg.workspace_path.clone());
    find_panel_available_in(&workspace)
}

pub(crate) fn find_panel_available_in(workspace: &str) -> bool {
    find_sig().with(|s| {
        s.view
            .as_ref()
            .is_some_and(|view| view.workspace == workspace)
    })
}

pub fn hide_find_panel() {
    find_sig().update(|s| s.open = false);
    if app_signal().with_untracked(|s| s.focus_zone == state::FocusZone::FindResults) {
        app_signal().update(|s| s.focus_zone = state::editor_focus_zone(s));
        state::focus_active_editor_ref();
    }
}

fn focus_find_panel() {
    let editor = app_signal().with_untracked(state::editor_focus_zone);
    if let Some(editor) = state::active_editor_api(editor) {
        editor.blur();
    }
    state::set_text_input_focused(false);
    app_signal().update(|s| s.focus_zone = state::FocusZone::FindResults);
}

/// 左侧「查找结果」按钮的状态（同时作为高亮与可见性来源）。
pub fn find_panel_visible() -> bool {
    find_panel_available() && find_sig().with(|s| s.open)
}

pub fn toggle_find_panel() {
    if find_panel_visible() {
        hide_find_panel();
    } else if find_panel_available() {
        show_find_panel();
    }
}

pub(crate) fn show_find_panel() {
    app_signal().update(|s| s.show_git_panel = false);
    find_sig().update(|s| s.open = true);
    focus_find_panel();
}

/// 「在查找窗口中打开」：钉入快照、关闭模态，再显示底部面板。
fn pin_to_find_results() {
    let snapshot = with_state(|s| {
        if s.running || s.error.is_some() || s.hits.is_empty() {
            return None;
        }
        Some(FindResultsView {
            workspace: s.workspace.clone(),
            query: s.query.clone(),
            hits: s.hits.clone(),
            index: s.index.clone()?,
        })
    });
    let view = match snapshot {
        Some(view) => view,
        None => return,
    };
    let selected = with_state(|s| s.selected);
    find_sig().update(|s| {
        s.view = Some(view);
        s.selected = selected;
    });
    state::close_active_modal();
    show_find_panel();
}

pub(super) fn move_find_selection(delta: isize) {
    find_sig().update(|s| {
        if let Some(view) = &s.view {
            s.selected = s.selected.saturating_add_signed(delta).min(view.hits.len().saturating_sub(1));
        }
    });
}

pub(super) fn activate_find_selection() {
    activate_find_hit(find_sig().with_untracked(|s| s.selected));
}

/// 打开面板中的命中；快照所属工作区已变化时拒绝导航。
fn activate_find_hit(idx: usize) {
    let current_ws = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let target = find_sig().with(|s| {
        let view = s.view.as_ref()?;
        if view.workspace != current_ws {
            return None;
        }
        let hit = view.hits.get(idx)?;
        let file = view
            .index
            .iter()
            .find(|f| f.repo_id == hit.repo_id && f.rel_path == hit.rel_path)?
            .clone();
        Some((
            file,
            EditorPosition::new(hit.line.saturating_sub(1) as u32, hit.column_utf16 as u32),
        ))
    });
    if let Some((file, position)) = target {
        state::open_code_file(file, Some(position), true);
    }
}

// ============================================================================
// 入口
// ============================================================================

/// ⌘⇧F：在整个工作区中查找。
pub fn open_workspace() {
    open_with(None, false);
}

pub fn open_replace() {
    open_with(None, true);
}

/// 项目目录右键「在目录中查找」：仅在该目录内查找。
/// `dir` 为上下文菜单传入的绝对目录路径。
pub fn open_for_directory(dir: String) {
    open_with(Some(PathBuf::from(dir)), false);
}

pub fn is_open() -> bool {
    with_state(|s| s.open)
}

pub fn query_is_multiline() -> bool {
    with_state(|s| s.multiline)
}

pub fn move_selection(delta: isize) {
    if !matches!(replacement_flow::phase(), Phase::Idle) {
        return;
    }
    update_state(|s| {
        if !s.hits.is_empty() {
            s.selected = s
                .selected
                .saturating_add_signed(delta)
                .min(s.hits.len() - 1);
        }
    });
}

pub fn intercept_close() -> bool {
    if !with_state(|s| s.open && s.replace_mode) {
        return false;
    }
    match replacement_flow::phase() {
        Phase::Applying => true,
        Phase::Planning | Phase::Confirm { .. } => replacement_flow::cancel(),
        Phase::Idle => false,
    }
}

/// 关闭由本模块持有的模态（供 `state::close_active_modal` 调用）。
pub fn close_if_open() -> bool {
    let open = with_state(|s| s.open);
    if open {
        update_state(|s| {
            s.open = false;
            s.generation += 1;
            s.cancel.cancel();
            s.cancel = CancelToken::new();
            s.running = false;
            s.query.clear();
            s.multiline = false;
            s.replacement.clear();
            s.mask.clear();
            s.mask_enabled = false;
            s.hits = Arc::default();
            s.truncated = false;
            s.selected = 0;
            s.error = None;
            s.index = None;
            s.scope_dir = None;
            s.workspace.clear();
        });
    }
    open
}

fn open_with(scope: Option<PathBuf>, replace_mode: bool) {
    replacement_flow::reset_message();
    let scope_label = match &scope {
        None => "整个工作区".to_string(),
        Some(dir) => relative_label(dir),
    };
    let workspace = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let initial = initial_query().unwrap_or_default();
    let index = Arc::new(candidates());

    // Release the underlying native editor before the search input mounts.
    let editor_zone = app_signal().with_untracked(state::editor_focus_zone);
    if let Some(editor) = state::active_editor_api(editor_zone) {
        editor.blur();
    }
    app_signal().update(state::enter_modal_focus);
    state::set_text_input_focused(true);
    update_state(|s| {
        s.cancel.cancel();
        s.cancel = CancelToken::new();
        s.open = true;
        s.replace_mode = replace_mode;
        s.generation += 1;
        s.workspace = workspace;
        s.scope_dir = scope.clone();
        s.scope_label = scope_label;
        s.index = Some(index);
        s.running = false;
        s.error = None;
        s.hits = Arc::default();
        s.truncated = false;
        s.selected = 0;
        s.multiline = initial.contains('\n');
        s.query = initial;
    });

    if with_state(|s| !s.query.trim().is_empty()) {
        schedule();
    }
}

/// 当前工作区的候选文件（来自既有工作区快照，绝不在此模块扫描磁盘）。
fn candidates() -> Vec<WorkspaceFile> {
    app_signal().with_untracked(|s| s.workspace_files.to_vec())
}

/// 从活动编辑器取当前选中文本作为初始查询；无法取得时返回 `None`。
fn initial_query() -> Option<String> {
    let zone = app_signal().with_untracked(|s| s.focus_zone);
    let api = state::active_editor_api(zone)?;
    let text = api.selected_text();
    // 多行选区保留原始换行（LF/CRLF）；只有单行选择才裁剪首尾空白。
    let text = if text.contains('\n') {
        text.to_string()
    } else {
        text.trim().to_string()
    };
    if text.is_empty() || text.chars().count() > MAX_INITIAL_QUERY_CHARS {
        None
    } else {
        Some(text)
    }
}

fn relative_label(dir: &Path) -> String {
    let ws = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    match dir.strip_prefix(Path::new(&ws)) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().to_string(),
        _ => dir.to_string_lossy().to_string(),
    }
}

// ============================================================================
// 搜索调度
// ============================================================================

fn mask_of(s: &ContentSearchState) -> Option<String> {
    if !s.mask_enabled {
        return None;
    }
    let mask = s.mask.trim();
    if mask.is_empty() {
        None
    } else {
        Some(mask.to_string())
    }
}

fn schedule() {
    let mut request = None;
    update_state(|s| {
        if !s.open {
            return;
        }
        // 任何输入/选项变化都立刻让旧结果失效：清掉可点击行、取消旧扫描、代次自增。
        s.generation += 1;
        s.cancel.cancel();
        s.cancel = CancelToken::new();
        s.error = None;
        s.hits = Arc::default();
        s.truncated = false;
        s.selected = 0;
        if s.query.trim().is_empty() {
            s.running = false;
            if s.index.is_none() {
                s.index = Some(Arc::new(candidates()));
            }
            return;
        }
        s.running = true;
        request = Some((
            s.generation,
            s.workspace.clone(),
            s.query.clone(),
            s.case_sensitive,
            s.regex,
            s.whole_word,
            mask_of(s),
            s.scope_dir.clone(),
            s.cancel.clone(),
            s.index.clone(),
        ));
    });

    let Some((
        generation,
        workspace,
        query,
        case_sensitive,
        regex,
        whole_word,
        mask,
        scope,
        cancel,
        index,
    )) = request
    else {
        return;
    };
    let Some(index) = index else {
        return;
    };

    // 防抖：定时器负责等待，绝不占用阻塞线程池去 sleep。
    let owner = global::<state::RootOwnerHolder>().with_untracked(|root| root.owner);
    let Some(owner) = owner else { return };
    owner.run(|| {
        set_timeout(DEBOUNCE, move || {
            spawn(async move {
                // 等待期间被后续输入取消：直接放弃，不做无谓扫描。
                if cancel.is_cancelled() {
                    return;
                }
                let query_scope = scope.clone();
                let outcome = blocking(move || {
                    let options = SearchOptions {
                        case_sensitive,
                        regex,
                        whole_word,
                        mask: mask.as_deref(),
                    };
                    search_files(
                        index.as_slice(),
                        query_scope.as_deref(),
                        &query,
                        &options,
                        &cancel,
                    )
                })
                .await;
                // 工作区已切换：旧工作区的任务结果绝不覆盖当前结果。
                let now_ws = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
                if now_ws != workspace {
                    return;
                }
                match outcome {
                    Ok(outcome) => apply_results(generation, workspace, scope, outcome),
                    Err(_) => update_state(|s| {
                        if s.open && s.generation == generation {
                            s.running = false;
                            s.error = Some("文件搜索失败".to_string());
                        }
                    }),
                }
            });
        })
    });
}

fn apply_results(
    generation: u64,
    workspace: String,
    scope: Option<PathBuf>,
    outcome: Result<SearchOutcome, SearchError>,
) {
    update_state(|s| {
        if !s.open || s.generation != generation || s.workspace != workspace || s.scope_dir != scope
        {
            return;
        }
        s.running = false;
        match outcome {
            Ok(o) if o.status == SearchStatus::Cancelled => {}
            Ok(o) => {
                s.error = None;
                s.selected = 0;
                s.truncated = o.truncated;
                s.hits = o.hits.into();
            }
            Err(e) => {
                s.hits = Arc::default();
                s.truncated = false;
                s.selected = 0;
                s.error = Some(e.to_string());
            }
        }
    });
}

// ============================================================================
// 打开命中
// ============================================================================

/// 直接用候选 `WorkspaceFile` + 零基 UTF-16 `EditorPosition` 打开命中；
/// 工作区已切换或候选已过期时拒绝打开，只做关闭。
fn activate(idx: usize) {
    if with_state(|s| s.replace_mode) {
        if matches!(replacement_flow::phase(), Phase::Idle) {
            update_state(|s| s.selected = idx);
        }
        return;
    }
    let current_ws = app_signal().with_untracked(|s| s.cfg.workspace_path.clone());
    let target = with_state(|s| {
        if s.workspace != current_ws {
            return None;
        }
        let hit = s.hits.get(idx)?;
        let file = s
            .index
            .as_ref()?
            .iter()
            .find(|f| f.repo_id == hit.repo_id && f.rel_path == hit.rel_path)?
            .clone();
        Some((
            file,
            EditorPosition::new(hit.line.saturating_sub(1) as u32, hit.column_utf16 as u32),
        ))
    });

    // 先走统一关闭流程，恢复键盘所有权，再打开目标位置。
    state::close_active_modal();
    if let Some((file, position)) = target {
        state::open_code_file(file, Some(position), true);
    }
}

pub fn activate_selected() {
    if with_state(|s| s.replace_mode) {
        start_replacement(false);
        return;
    }
    let idx = with_state(|s| s.selected);
    if with_state(|s| s.hits.get(idx).is_some()) {
        activate(idx);
    }
}

// ============================================================================
// UI
// ============================================================================

pub fn modal_layer() -> impl IntoElement {
    // 仅依赖 open：结果刷新不会重建容器，查询输入框因此保持挂载。
    let open = computed(|| with_state(|s| s.open));
    when(move || open.get(), content_search_modal)
}

fn content_search_modal() -> impl IntoElement {
    let p = current_palette();
    let preview = signal(true);
    let query = element_ref();
    after_answer(move || query.focus());
    let target = computed(move || {
        let workspace = app_signal().with(|s| s.cfg.workspace_path.clone());
        with_state(|s| {
            (s.open && !s.replace_mode && s.workspace == workspace)
                .then(|| s.hits.get(s.selected).cloned()).flatten()
        })
    });
    let show_preview = computed(move || preview.get() && target.with(Option::is_some));
    view()
        .test_id("content-search-modal")
        .absolute()
        .inset_0()
        .p(px(12.0))
        .bg(rgba(0x00000070))
        .items_center()
        .justify_center()
        .child(
            view()
                .w_full()
                .max_w(px(900.0))
                .h_full()
                .max_h(px(640.0))
                .rounded(px(12.0))
                .bg(p.surface)
                .border_1()
                .border_color(p.border)
                .flex_col()
                .overflow_hidden()
                .child(header(p))
                .child(query_row(p, query))
                .child(options_row(p))
                .child(when(
                    || with_state(|s| s.replace_mode),
                    move || {
                        view()
                            .flex_col()
                            .child(replacement_input(p))
                            .child(dynamic(move || replacement_actions(p)))
                    },
                ))
                .child(
                    v_resizable()
                        .test_id("content-search-result-split")
                        .handle_accessibility_label(Some(SharedString::from("调整代码预览高度")))
                        .flex_1()
                        .min_h(px(0.0))
                        .child(resizable_panel().min_size(px(90.0)).child(results_view(preview)))
                        .child(
                            resizable_panel()
                                .visible(move || show_preview.get())
                                .size_range(px(100.0)..px(460.0))
                                .child(code_preview(target, show_preview, false)),
                        ),
                ),
        )
}

fn header(p: Palette) -> impl IntoElement {
    let (scope, replace) = with_state(|s| (s.scope_label.clone(), s.replace_mode));
    let title = if replace {
        "在文件中替换"
    } else {
        "在文件中查找"
    };
    view()
        .h(px(44.0))
        .px(px(14.0))
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            text(title)
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text_primary),
        )
        .child(
            view()
                .test_id("content-search-scope")
                .child(text(scope).text_size(px(11.0)).text_color(p.text_muted)),
        )
        .child(when(
            move || !replace,
            move || dynamic(move || pin_button(p)),
        ))
        .child(view().flex_1())
        .child(
            text(if replace { "⇧⌘R" } else { "⇧⌘F" })
                .text_size(px(10.5))
                .text_color(p.text_muted),
        )
        .child(
            view()
                .cursor(Cursor::Pointer)
                .on_press(state::close_active_modal)
                .child(icons::icon(ICO_CLOSE, 12.0, p.text_muted)),
        )
}

fn query_row(p: Palette, query: ElementRef) -> impl IntoElement {
    let multiline = computed(query_is_multiline);
    view()
        .px(px(14.0))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(
            input()
                .test_id("content-search-input")
                .element_ref(&query)
                .auto_focus(true)
                .h(move || if multiline.get() { px(84.0) } else { px(32.0) })
                .px(px(8.0))
                .py(move || if multiline.get() { px(6.0) } else { px(7.0) })
                .line_height(px(16.0))
                .rounded(px(6.0))
                .flex_1()
                .bg(p.rail_bg)
                .border_1()
                .border_color(p.accent)
                .text_size(px(12.5))
                .text_color(p.text_primary)
                .multiline(move || multiline.get())
                .placeholder("搜索文件内容…")
                .value(move || with_state(|s| s.query.clone()))
                .on_focus(|| state::set_text_input_focused(true))
                .on_blur(|| state::set_text_input_focused(false))
                .on_change(|v: &str| {
                    if replacement_input_changed() {
                        update_state(|s| s.query = v.to_string());
                        schedule();
                    }
                })
                .on_submit(|_: &str| {
                    if !query_is_multiline() {
                        activate_selected();
                    }
                }),
        )
        .child(toggle_button(
            "content-search-multiline",
            "多行",
            "多行查询",
            p,
            || with_state(|s| s.multiline),
            || {
                if replacement_input_changed() {
                    update_state(|s| {
                        s.multiline = !s.multiline || s.query.contains('\n');
                    });
                }
            },
        ))
        .child(toggle_button(
            "content-search-case",
            "Aa",
            "区分大小写",
            p,
            || with_state(|s| s.case_sensitive),
            || {
                if replacement_input_changed() {
                    update_state(|s| s.case_sensitive = !s.case_sensitive);
                    schedule();
                }
            },
        ))
        .child(toggle_button(
            "content-search-regex",
            ".*",
            "使用正则表达式",
            p,
            || with_state(|s| s.regex),
            || {
                if replacement_input_changed() {
                    update_state(|s| s.regex = !s.regex);
                    schedule();
                }
            },
        ))
        .child(toggle_button(
            "content-search-word",
            "W",
            "仅匹配整个单词",
            p,
            || with_state(|s| s.whole_word),
            || {
                if replacement_input_changed() {
                    update_state(|s| s.whole_word = !s.whole_word);
                    schedule();
                }
            },
        ))
        .child(toggle_button(
            "content-search-mask-toggle",
            "文件掩码",
            "按文件名过滤",
            p,
            || with_state(|s| s.mask_enabled),
            || {
                if replacement_input_changed() {
                    update_state(|s| s.mask_enabled = !s.mask_enabled);
                    schedule();
                }
            },
        ))
}

fn options_row(p: Palette) -> impl IntoElement {
    view()
        .px(px(14.0))
        .py(px(6.0))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(when(
            move || with_state(|s| s.mask_enabled),
            move || mask_input(p),
        ))
}

fn mask_input(p: Palette) -> impl IntoElement {
    input()
        .test_id("content-search-mask-input")
        .h(px(26.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .flex_1()
        .bg(p.rail_bg)
        .border_1()
        .border_color(p.border)
        .text_size(px(11.5))
        .text_color(p.text_primary)
        .placeholder("文件名掩码，如 *.rs, !*_test.rs")
        .value(move || with_state(|s| s.mask.clone()))
        .on_focus(|| state::set_text_input_focused(true))
        .on_blur(|| state::set_text_input_focused(false))
        .on_change(|v: &str| {
            if replacement_input_changed() {
                update_state(|s| s.mask = v.to_string());
                schedule();
            }
        })
}

fn replacement_input_changed() -> bool {
    if !with_state(|s| s.replace_mode) {
        return true;
    }
    if !replacement_flow::cancel() {
        return false;
    }
    replacement_flow::reset_message();
    true
}

fn replacement_input(p: Palette) -> impl IntoElement {
    view().px(px(14.0)).child(
        input()
            .test_id("content-search-replacement-input")
            .h(px(28.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .bg(p.rail_bg)
            .border_1()
            .border_color(p.border)
            .text_size(px(11.5))
            .text_color(p.text_primary)
            .placeholder("替换为…")
            .value(|| with_state(|s| s.replacement.clone()))
            .on_focus(|| state::set_text_input_focused(true))
            .on_blur(|| state::set_text_input_focused(false))
            .on_change(|value: &str| {
                if replacement_input_changed() {
                    update_state(|s| s.replacement = value.to_string());
                }
            })
            .on_submit(|_: &str| activate_selected()),
    )
}

fn start_replacement(all: bool) {
    let request = with_state(|s| {
        if !s.replace_mode || s.running || s.error.is_some() || s.hits.is_empty() {
            return None;
        }
        Some(replacement_flow::Request {
            workspace: s.workspace.clone(),
            files: s.index.clone()?,
            scope: s.scope_dir.clone(),
            query: ReplaceQuery {
                pattern: s.query.clone(),
                replacement: s.replacement.clone(),
                regex: s.regex,
                whole_word: s.whole_word,
                case_sensitive: s.case_sensitive,
                mask: mask_of(s),
            },
            hit: if all {
                None
            } else {
                Some(s.hits.get(s.selected)?.clone())
            },
        })
    });
    if let Some(request) = request {
        replacement_flow::start(request, refresh_after_replacement);
    }
}

fn refresh_after_replacement() {
    state::refresh_workspace_async(None);
    schedule();
}

fn replacement_actions(p: Palette) -> impl IntoElement {
    let phase = replacement_flow::phase();
    let message = match phase {
        Phase::Planning => Some("正在检查替换范围…".into()),
        Phase::Applying => Some("正在替换…".into()),
        _ => replacement_flow::message(),
    };
    let available = with_state(|s| !s.running && s.error.is_none() && !s.hits.is_empty());
    view()
        .min_h(px(36.0))
        .px(px(14.0))
        .py(px(4.0))
        .gap(px(6.0))
        .flex_row()
        .items_center()
        .children(match phase {
            Phase::Idle if available => vec![
                toggle_button(
                    "replace-selected",
                    "替换选中项",
                    "替换选中的匹配",
                    p,
                    || false,
                    || start_replacement(false),
                )
                .into_element(),
                toggle_button(
                    "replace-all",
                    "全部替换…",
                    "检查范围内所有匹配",
                    p,
                    || false,
                    || start_replacement(true),
                )
                .into_element(),
            ],
            Phase::Confirm { .. } => vec![
                toggle_button(
                    "replace-confirm",
                    "确认全部替换",
                    "执行已检查的替换",
                    p,
                    || true,
                    || replacement_flow::confirm(refresh_after_replacement),
                )
                .into_element(),
                toggle_button(
                    "replace-cancel",
                    "取消",
                    "取消待确认的替换",
                    p,
                    || false,
                    || {
                        replacement_flow::cancel();
                    },
                )
                .into_element(),
            ],
            _ => Vec::new(),
        })
        .children(message.map(|message| {
            view()
                .test_id("replace-status")
                .child(text(message).text_size(px(11.0)).text_color(p.text_muted))
        }))
}

fn toggle_button(
    tid: &'static str,
    label: &'static str,
    tip: &'static str,
    p: Palette,
    active: impl Fn() -> bool + 'static,
    on_click: impl Fn() + 'static,
) -> impl IntoElement {
    let active = computed(active);
    view()
        .test_id(tid)
        .h(px(28.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .items_center()
        .justify_center()
        .cursor(Cursor::Pointer)
        .label(tip)
        .bg(move || {
            if active.get() {
                p.accent_soft
            } else {
                p.rail_bg
            }
        })
        .on_press(move || on_click())
        .child(
            text(label)
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || {
                    if active.get() {
                        p.accent_soft_text
                    } else {
                        p.text_primary
                    }
                }),
        )
}

fn pin_button(p: Palette) -> impl IntoElement {
    let (fg, bg) = with_state(|s| {
        if s.open && !s.running && s.error.is_none() && !s.hits.is_empty() {
            (p.text_primary, p.surface_hover)
        } else {
            (p.text_muted, p.surface)
        }
    });
    view()
        .test_id("content-search-pin")
        .h(px(22.0))
        .px(px(8.0))
        .rounded(px(5.0))
        .items_center()
        .justify_center()
        .bg(bg)
        .cursor(Cursor::Pointer)
        .label("在查找窗口中打开")
        .on_press(move || {
            if with_state(|s| s.open && !s.running && s.error.is_none() && !s.hits.is_empty()) {
                pin_to_find_results();
            }
        })
        .child(text("在查找窗口中打开").text_size(px(11.0)).text_color(fg))
}

fn preview_toggle(p: Palette, preview: Signal<bool>, id: &'static str) -> impl IntoElement {
    view()
        .test_id(id)
        .w(px(25.0)).h(px(24.0)).flex_none()
        .rounded(px(4.0)).justify_center().items_center()
        .cursor(Cursor::Pointer)
        .label(move || if preview.get() { "隐藏代码预览" } else { "显示代码预览" })
        .bg(move || if preview.get() { p.accent_soft } else { transparent_black() })
        .hover(move |style| style.bg(p.surface_hover))
        .on_press(move || preview.update(|enabled| *enabled = !*enabled))
        .child(icons::icon(ICO_COLUMNS, 13.0, p.text_control))
}

#[derive(Clone, Default)]
struct PreviewFocus(Option<bool>);

pub(super) fn preview_has_focus() -> bool {
    global::<PreviewFocus>().with(|focus| focus.0.is_some())
}

fn clear_preview_focus(pinned: bool) {
    let focus = global::<PreviewFocus>();
    if focus.with_untracked(|focus| focus.0 == Some(pinned)) {
        focus.set(PreviewFocus(None));
        state::set_text_input_focused(false);
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PreviewKey {
    path: String,
    digest: String,
}

impl From<&SearchHit> for PreviewKey {
    fn from(hit: &SearchHit) -> Self {
        Self { path: hit.abs_path.clone(), digest: hit.raw_sha256.clone() }
    }
}

fn code_preview(
    target: Memo<Option<SearchHit>>,
    visible: Memo<bool>,
    pinned: bool,
) -> impl IntoElement {
    let loaded = signal(None::<(PreviewKey, Option<SharedString>)>);
    let desired = computed(move || {
        visible.get().then(|| target.with(|hit| hit.as_ref().map(PreviewKey::from))).flatten()
    });
    let owner = Owner::current();
    effect(move || {
        let Some(key) = desired.get() else {
            clear_preview_focus(pinned);
            loaded.set(None);
            return;
        };
        if loaded.with_untracked(|preview| preview.as_ref().is_some_and(|(loaded, _)| *loaded == key)) {
            return;
        }
        let Some(hit) = target.get_untracked() else { return };
        owner.run(|| {
            set_timeout(Duration::from_millis(45), move || {
                if desired.with_untracked(|wanted| wanted.as_ref() != Some(&key)) {
                    return;
                }
                spawn(async move {
                    let body = blocking(move || crate::search::read_preview_file(&hit)).await
                        .ok().flatten().map(SharedString::from);
                    if owner.is_live() && desired.with_untracked(|wanted| wanted.as_ref() == Some(&key)) {
                        loaded.set(Some((key, body)));
                    }
                });
            });
        });
    });
    cleanup(move || clear_preview_focus(pinned));
    let document = computed(move || {
        let key = desired.get()?;
        loaded.with(|preview| preview.as_ref().filter(|(loaded, _)| *loaded == key).cloned())
    });
    let font_size = computed(|| app_signal().with(|s| s.cfg.editor_font_size));
    let title = computed(move || target.with(|hit| hit.as_ref().map(|hit| (
        crate::paths::basename(&hit.rel_path),
        hit.rel_path.rsplit_once('/').map_or_else(String::new, |(parent, _)| parent.to_owned()),
        hit.line,
        hit.column,
    ))));
    let p = current_palette();

    view()
        .test_id("search-code-preview")
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .overflow_hidden()
        .bg(p.surface)
        .child(dynamic(move || {
            let (file, directory, line, column) = title.get()?;
            Some(view().h(px(28.0)).px(px(10.0)).flex_none().flex_row().items_center().gap(px(7.0))
                .border_b_1().border_color(p.border).bg(p.rail_bg)
                .child(icons::file_icon(&file, 12.0, p))
                .child(text(file).text_left().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text_primary).number_of_lines(1u32))
                .child(text(directory).flex_1().min_w(px(0.0)).text_left().text_size(px(11.0))
                    .text_color(p.text_muted).number_of_lines(1u32))
                .child(text(format!("{line}:{column}")).flex_none().text_size(px(10.5)).text_color(p.text_secondary)))
        }))
        .child(dynamic(move || {
            let status = match document.get() {
                Some((key, Some(source))) => {
                    let language = target.with_untracked(|hit| hit.as_ref().map(|hit| SharedString::from(hit.rel_path.clone())));
                    let range = computed(move || target.with(|hit| {
                        let hit = hit.as_ref()?;
                        preview_range(hit, &source)
                    }));
                    let text = document.with_untracked(|loaded| loaded.as_ref().and_then(|(_, body)| body.clone()));
                    let ed = component_ref::<Editor>();
                    effect(move || {
                        if let Some(range) = range.get() {
                            after_answer(move || {
                                if let Some(api) = ed.api::<EditorApi>() {
                                    api.reveal_position(range.start, true);
                                }
                            });
                        }
                    });
                    return view().key(key.path).flex_1().min_h(px(0.0)).overflow_hidden()
                        .child(editor()
                            .test_id("search-preview-editor")
                            .component_ref(&ed)
                            .styles(super::review_editor_styles)
                            .default_value(text)
                            .language(language)
                            .font_size(move || px(font_size.get()))
                            .read_only(true)
                            .folding(false)
                            .active_line(false)
                            .accessibility_label(Some(SharedString::from("只读代码预览")))
                            .decorations(move || range.get().map_or_else(Vec::new, |range| preview_decorations(range, p)))
                            .on_focus(move || {
                                if pinned {
                                    app_signal().update(|s| s.focus_zone = state::FocusZone::FindResults);
                                }
                                global::<PreviewFocus>().set(PreviewFocus(Some(pinned)));
                                state::set_text_input_focused(true);
                            })
                            .on_blur(move || clear_preview_focus(pinned))
                            .flex_1())
                        .into_element();
                }
                None => "正在读取预览…",
                Some((_, None)) => "文件已变化或无法读取，请重新查找。",
            };
            text(status).test_id("search-preview-status").px(px(12.0)).py(px(8.0))
                .text_left().text_size(px(11.0)).text_color(p.text_muted).into_element()
        }))
}

fn preview_range(hit: &SearchHit, source: &str) -> Option<EditorRange> {
    let matched = source.get(hit.file_range.start as usize..hit.file_range.end as usize)?;
    let line = hit.line.saturating_sub(1);
    let newlines = matched.bytes().filter(|byte| *byte == b'\n').count();
    let end_column = if newlines == 0 {
        hit.column_utf16 + matched.encode_utf16().count()
    } else {
        matched.rsplit('\n').next().unwrap_or_default().encode_utf16().count()
    };
    Some(EditorRange::new(
        EditorPosition::new(line as u32, hit.column_utf16 as u32),
        EditorPosition::new((line + newlines) as u32, end_column as u32),
    ))
}

fn preview_decorations(range: EditorRange, p: Palette) -> Vec<EditorDecoration> {
    let bg = |color: Hsla| nsh_core::element::Style::default().with(
        nsh_core::StyleKey::BackgroundColor,
        nsh_core::PropertyValue::Color(Some(color.into())),
    );
    let mut row = EditorDecoration::default();
    row.range = EditorRange::new(range.start, range.start);
    row.line_style = Some(bg(p.surface2));
    let mut hit = EditorDecoration::default();
    hit.range = range;
    hit.style = Some(bg(p.accent_soft).with(
        nsh_core::StyleKey::Color,
        nsh_core::PropertyValue::Color(Some(p.accent_soft_text.into())),
    ));
    vec![row, hit]
}

fn hit_rows(
    p: Palette,
    hits: &[SearchHit],
    scope: &str,
    selected: impl Fn(usize) -> bool + Copy + 'static,
    on_press: impl Fn(usize, &PressEvent) + Copy + 'static,
    size: f32,
) -> AnyElement {
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut last_group = None;
    for (idx, hit) in hits.iter().enumerate() {
        let row = element_ref();
        let is_selected = computed(move || selected(idx));
        effect(move || {
            if is_selected.get() {
                super::reveal_selected_row(row);
            }
        });
        let group = (&hit.repo_id, &hit.rel_path);
        if last_group != Some(group) {
            last_group = Some(group);
            let file = crate::paths::basename(&hit.rel_path);
            let dir = hit.rel_path.rsplit_once('/').map_or("", |(dir, _)| dir);
            let location = if dir.is_empty() { hit.repo_name.clone() } else { format!("{} / {dir}", hit.repo_name) };
            rows.push(
                view()
                    .test_id(&format!("{scope}-group-{}", hit.rel_path))
                    .h(px(size + 12.0)).px(px(10.0)).flex_row().items_center().gap(px(7.0))
                    .overflow_hidden()
                    .bg(p.rail_bg)
                    .child(icons::file_icon(&file, 12.0, p))
                    .child(text(file).text_left().text_size(px(size)).font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_primary).number_of_lines(1u32))
                    .child(text(location).flex_1().min_w(px(0.0)).text_left()
                        .text_size(px((size - 1.0).max(10.0))).text_color(p.text_muted).number_of_lines(1u32))
                    .into_element(),
            );
        }
        let range = hit.match_in_snippet.clone();
        let before = hit.snippet.get(..range.start).unwrap_or("");
        let matched = hit.snippet.get(range.clone()).unwrap_or("");
        let after = hit.snippet.get(range.end..).unwrap_or("");
        rows.push(
            view()
                .test_id(&format!("{scope}-hit-{idx}"))
                .element_ref(&row)
                .h(px(size + 10.0)).px(px(10.0)).flex_row().items_center().gap(px(8.0))
                .cursor(Cursor::Pointer)
                .bg(move || if is_selected.get() { p.active_row_bg } else { p.surface })
                .hover(move |style| style.bg(p.surface_hover))
                .on_press(move |e: &PressEvent| on_press(idx, e))
                .child(
                    text(format!("{}:{}", hit.line, hit.column))
                        .w(px(size * 5.5)).flex_none().text_right()
                        .text_size(px((size - 1.0).max(10.0))).text_color(p.text_muted),
                )
                .child(
                    text(before.to_owned())
                        .flex_1()
                        .min_w(px(0.0))
                        .text_left()
                        .text_size(px(size))
                        .font_family("monospace")
                        .number_of_lines(1u32)
                        .text_color(p.text_secondary)
                        .child(text(matched.to_owned()).text_color(p.accent_soft_text).bg(p.accent_soft))
                        .child(text(after.to_owned())),
                )
                .into_element(),
        );
    }
    view().flex_col().children(rows).into_element()
}

pub fn find_panel() -> impl IntoElement {
    let visible = computed(find_panel_visible);
    when(move || visible.get(), find_panel_body)
}

fn find_panel_body() -> impl IntoElement {
    let p = current_palette();
    let preview = signal(true);
    let focus = element_ref();
    let target = computed(|| {
        let workspace = app_signal().with(|s| s.cfg.workspace_path.clone());
        find_sig().with(|s| {
            s.view.as_ref().filter(|v| v.workspace == workspace)?.hits.get(s.selected).cloned()
        })
    });
    let show_preview = computed(move || preview.get() && target.with(Option::is_some));
    let results = computed(|| find_sig().with(|s| s.view.as_ref().map(|v| v.hits.clone()).unwrap_or_default()));
    let font_size = computed(|| app_signal().with(|s| s.cfg.search_font_size));
    let query = computed(|| find_sig().with(|s| s.view.as_ref().map(|v| v.query.clone()).unwrap_or_default()));
    view()
        .test_id("find-results-panel")
        .element_ref(&focus)
        .focusable(true)
        .on_focus(focus_find_panel)
        .h(px(276.0)).flex_none().flex_col().overflow_hidden()
        .border_1().border_color(p.border).bg(p.surface)
        .child(
            view().h(px(32.0)).px(px(10.0)).flex_row().items_center().gap(px(8.0)).bg(p.rail_bg)
                .child(
                    text(move || format!("查找 · {}", query.get()))
                        .test_id("find-results-title").flex_1().text_left().number_of_lines(1u32)
                        .text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(p.text_primary),
                )
                .child(preview_toggle(p, preview, "find-results-preview-toggle"))
                .child(
                    view().test_id("find-results-hide").cursor(Cursor::Pointer)
                        .label("隐藏查找结果").on_press(hide_find_panel)
                        .child(icons::icon(ICO_CLOSE, 12.0, p.text_muted)),
                ),
        )
        .child(
            h_resizable()
                .test_id("find-results-preview-split")
                .handle_accessibility_label(Some(SharedString::from("调整代码预览宽度")))
                .flex_1()
                .min_h(px(0.0))
                .child(resizable_panel().min_size(px(130.0)).child(
                    scroll().flex_1().overflow_hidden().child(dynamic(move || {
                        let size = font_size.get();
                        hit_rows(p, &results.get(), "find-results",
                            |idx| find_sig().with(|s| s.selected == idx),
                            move |idx, e| {
                                find_sig().update(|s| s.selected = idx);
                                if e.clicks >= 2 {
                                    activate_find_hit(idx);
                                } else {
                                    focus_find_panel();
                                    focus.focus();
                                }
                            },
                            size,
                        )
                    })),
                ))
                .child(
                    resizable_panel()
                        .visible(move || show_preview.get())
                        .min_size(px(170.0))
                        .child(code_preview(target, show_preview, true)),
                ),
        )
}

fn results_view(preview: Signal<bool>) -> impl IntoElement {
    let p = current_palette();
    let status = computed(|| with_state(|s| (
        s.running, s.error.clone(), s.hits.len(), s.truncated, s.query.trim().is_empty(), s.replace_mode,
    )));
    let results = computed(|| with_state(|s| s.hits.clone()));
    let font_size = computed(|| app_signal().with(|s| s.cfg.search_font_size));
    view().flex_1().min_h(px(0.0)).flex_col().overflow_hidden()
        .child(dynamic(move || {
            let (running, error, count, truncated, empty_query, replace) = status.get();
            let (status, color) = if let Some(err) = error {
                (format!("搜索表达式无效：{err}"), p.danger)
            } else if running {
                ("正在搜索…".to_string(), p.text_muted)
            } else if count == 0 {
                (if empty_query { "输入关键字开始搜索" } else { "无匹配结果" }.to_string(), p.text_muted)
            } else if truncated {
                (format!("{count} 条匹配（达到上限）"), p.text_muted)
            } else {
                (format!("{count} 条匹配"), p.text_muted)
            };
            view().h(px(30.0)).px(px(10.0)).flex_none().flex_row().items_center()
                .child(text(status).test_id("content-search-status").flex_1().text_left()
                    .text_size(px(11.0)).text_color(color))
                .child((!replace && count != 0).then(|| preview_toggle(p, preview, "content-search-preview-toggle")))
        }))
        .child(scroll().flex_1().overflow_hidden().child(dynamic(move || {
            let size = font_size.get();
            hit_rows(p, &results.get(), "content-search",
                |idx| with_state(|s| s.selected == idx),
                |idx, e| {
                    if e.clicks >= 2 {
                        activate(idx);
                    } else if matches!(replacement_flow::phase(), Phase::Idle) {
                        update_state(|s| s.selected = idx);
                    }
                },
                size,
            )
        })))
}
