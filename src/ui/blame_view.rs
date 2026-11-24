use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nsh_base::{Editor, EditorApi, EditorFoldRange, EditorPosition, EditorRange};
use nsh_core::element::desktop::MenuItem;
use nsh_core::prelude::*;

use crate::blame::{self, Line};
use crate::editor_split::PaneId;
use crate::state::{self, RootOwnerHolder, app_signal, current_palette};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Line,
    File,
}

#[derive(Clone, PartialEq, Eq)]
struct Source {
    workspace: String,
    tab: String,
    revision: u64,
    git_revision: blame::Revision,
}

#[derive(Clone, PartialEq, Eq)]
struct Request {
    source: Source,
    first: u32,
    last: u32,
}

impl Request {
    fn covers(&self, source: &Source, first: u32, last: u32) -> bool {
        self.source == *source && self.first <= first && self.last >= last
    }
}

#[derive(Clone, Default)]
struct Attribution {
    active: Option<(String, Mode)>,
    hidden_tab: Option<String>,
    generation: u64,
    request: Option<Request>,
    lines: Arc<Vec<Line>>,
    loading: bool,
    error: Option<String>,
}

fn attribution() -> Signal<Attribution> {
    global::<Attribution>()
}

fn active_source() -> Option<(Source, u32)> {
    app_signal().with(|s| {
        if s.editor_tab != state::EditorTabKind::File {
            return None;
        }
        let tab = s.code_tabs.get(s.active_code_tab_idx)?;
        if tab.binary || tab.too_large {
            return None;
        }
        Some((
            Source {
                workspace: s.cfg.workspace_path.clone(),
                tab: tab.id.clone(),
                revision: tab.content_rev,
                git_revision: if tab.revision.is_empty() {
                    blame::Revision::Working(s.repository_head(&tab.file.repo_path))
                } else {
                    blame::Revision::Commit(tab.revision.clone())
                },
            },
            tab.cursor.line + 1,
        ))
    })
}

fn enable(mode: Mode) {
    if let Some((source, _)) = active_source() {
        attribution().update(|s| {
            s.hidden_tab = None;
            s.active = Some((source.tab, mode));
        });
    }
}

pub(super) fn note_text_press() {
    if let Some((source, _)) = active_source()
        && attribution().with_untracked(|s| {
            s.hidden_tab.as_ref() != Some(&source.tab)
                && s.active.as_ref().is_none_or(|(id, _)| id != &source.tab)
        })
    {
        enable(Mode::Line);
    }
}

pub(super) fn menu_items(line: u32) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::new("显示整文件 Git Blame").on_select(|| enable(Mode::File)),
        MenuItem::new("只显示该行 Git Blame").on_select(move || {
            enable(Mode::Line);
            if let Some(api) = state::active_editor_api(state::FocusZone::CodeEditor) {
                api.set_selections(vec![nsh_base::EditorSelection::caret(EditorPosition::new(
                    line.saturating_sub(1),
                    0,
                ))]);
            }
        }),
    ];
    if let Some((source, _)) = active_source() {
        let attribution = attribution().get_untracked();
        if attribution
            .active
            .as_ref()
            .is_some_and(|(id, _)| id == &source.tab)
        {
            items.push(MenuItem::new("隐藏 Git Blame").on_select(move || {
                attribution_signal_reset(&source);
            }));
        }
        if let Some(item) = attribution
            .lines
            .iter()
            .find(|item| item.number == line && !item.hash.is_empty())
            && let Some(request) = &attribution.request
            && active_source().is_some_and(|(source, _)| source == request.source)
        {
            let hash = item.hash.clone();
            items.push(
                MenuItem::new("在 Git 树中查看该提交").on_select(move || reveal(hash.clone())),
            );
        }
    }
    items
}

fn attribution_signal_reset(source: &Source) {
    attribution().update(|s| {
        *s = Attribution {
            hidden_tab: Some(source.tab.clone()),
            generation: s.generation.wrapping_add(1),
            ..Attribution::default()
        };
    });
}

fn reveal(hash: String) {
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let Some((workspace, repo)) = app_signal().with_untracked(|s| {
        let tab = s.code_tabs.get(s.active_code_tab_idx)?;
        let repo = s
            .repos
            .iter()
            .find(|repo| repo.id == tab.file.repo_id)?
            .clone();
        Some((s.cfg.workspace_path.clone(), repo))
    }) else {
        return;
    };
    owner.run(|| {
        spawn(async move {
            let outcome = blocking(move || crate::git_log_query::lookup(&repo, &hash)).await;
            if !app_signal().with_untracked(|s| s.cfg.workspace_path == workspace) {
                return;
            }
            match outcome {
                Ok(Ok(commit)) => super::git_forest::reveal(commit),
                Ok(Err(err)) => state::show_toast("warn", format!("读取归属提交失败：{err}")),
                Err(err) => state::show_toast("warn", format!("读取归属提交失败：{err}")),
            }
        })
    });
}

fn request(source: Source, first: u32, last: u32) {
    if attribution().with_untracked(|s| {
        s.request
            .as_ref()
            .is_some_and(|req| req.covers(&source, first, last))
    }) {
        return;
    }
    let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) else {
        return;
    };
    let start = ((first - 1) / blame::MAX_LINES) * blame::MAX_LINES + 1;
    let first = if last >= start.saturating_add(blame::MAX_LINES) {
        first
    } else {
        start
    };
    let last = first.saturating_add(blame::MAX_LINES - 1);
    let request = Request {
        source,
        first,
        last,
    };
    let generation = attribution().with_untracked(|s| s.generation.wrapping_add(1));
    attribution().update(|s| {
        s.generation = generation;
        s.request = Some(request.clone());
        s.lines = Arc::default();
        s.loading = true;
        s.error = None;
    });
    owner.run(|| {
        set_timeout(Duration::from_millis(115), move || {
            if !attribution().with_untracked(|s| s.generation == generation)
                || !active_source().is_some_and(|(source, _)| source == request.source)
            {
                return;
            }
            let data = app_signal().with_untracked(|s| {
                let tab = s.code_tabs.get(s.active_code_tab_idx)?;
                let count = tab.text.lines().count() as u32;
                let bytes = tab
                    .revision
                    .is_empty()
                    .then(|| tab.text.as_bytes().to_vec());
                Some((tab.file.clone(), count, bytes))
            });
            let Some((file, count, bytes)) = data else {
                return;
            };
            spawn(async move {
                let revision = request.source.git_revision.clone();
                let outcome = blocking(move || -> anyhow::Result<Option<Vec<Line>>> {
                    if count < first {
                        return Ok(Some(Vec::new()));
                    }
                    let last = last.min(count);
                    let Some(lines) = revision.read(
                        Path::new(&file.repo_path),
                        &file.rel_path,
                        first,
                        last,
                        bytes,
                    )? else {
                        return Ok(None);
                    };
                    if revision.uses_buffer() && lines.is_empty() {
                        // Files the agent just created have no committed lines yet.
                        return Ok(Some((first..=last).map(|number| Line {
                            number,
                            hash: String::new(),
                            author: String::new(),
                            author_unix: 0,
                            summary: String::new(),
                        }).collect()));
                    }
                    Ok(Some(lines))
                })
                .await;
                if !active_source().is_some_and(|(source, _)| source == request.source) {
                    return;
                }
                attribution().update(|s| {
                    if s.generation != generation {
                        return;
                    }
                    s.loading = false;
                    match outcome {
                        Ok(Ok(Some(lines))) if !lines.is_empty() => s.lines = Arc::new(lines),
                        Ok(Ok(Some(_))) => s.error = Some("没有可归属的行".into()),
                        Ok(Ok(None)) => {}
                        Ok(Err(err)) => s.error = Some(format!("读取 Git 归属失败：{err}")),
                        Err(err) => s.error = Some(format!("读取 Git 归属失败：{err}")),
                    }
                });
            });
        })
    });
}

#[derive(Clone, PartialEq, Default)]
struct Viewport {
    first: u32,
    last: u32,
    line: u32,
    scroll: Point<Points>,
    folds: Vec<EditorFoldRange>,
}

impl Viewport {
    fn shows_line(&self, number: u32) -> bool {
        let row = number.saturating_sub(1);
        (self.first..=self.last).contains(&number)
            && !self.folds.iter().any(|fold| fold.start_line < row && row <= fold.end_line)
    }
}

pub(super) fn editor_body(
    editor: impl IntoElement,
    ed_ref: ComponentRef<Editor>,
    side: Option<PaneId>,
) -> impl IntoElement {
    let size = signal(Size::new(px(0.0), px(0.0)));
    let viewport = signal(Viewport::default());
    let scrolling = signal(false);
    let last_wheel = stored(None::<nsh_core::time::Instant>);
    let active = computed(move || {
        let pane_has_focus = side.is_none_or(|side| {
            app_signal().with(|s| {
                s.code_split
                    .as_ref()
                    .is_some_and(|split| split.focused == side)
            })
        });
        let source = active_source()?.0;
        pane_has_focus
            .then(|| {
                attribution().with(|s| {
                    s.active
                        .as_ref()
                        .filter(|(id, _)| id == &source.tab)
                        .map(|(_, mode)| *mode)
                })
            })
            .flatten()
    });
    let redraw = move || {
        let Some(mode) = active.get_untracked() else {
            return;
        };
        let Some((source, line)) = active_source() else {
            return;
        };
        let Some(api) = ed_ref.api::<EditorApi>() else {
            return;
        };
        if size.get_untracked().height.0 <= 0.0 {
            return;
        }
        let visible = api.visible_range();
        let view = Viewport {
            first: visible.start.line + 1,
            last: visible.end.line + 1,
            line,
            scroll: api.scroll_offset(),
            folds: api.folds(),
        };
        let (first, last) = (view.first, view.last);
        viewport.set(view);
        if scrolling.get_untracked() && last_wheel.get().is_some_and(|at| {
            nsh_core::time::now().duration_since(at) >= Duration::from_millis(90)
        }) {
            scrolling.set(false);
        }
        if mode == Mode::File {
            request(source, first, last.min(first + blame::MAX_LINES - 1));
        } else {
            request(source, line, line);
        }
    };
    effect(move || {
        if active.get().is_some() {
            let timer = set_interval(Duration::from_millis(90), redraw);
            cleanup(move || timer.cancel());
            after_answer(redraw);
        }
    });
    view()
        .flex_1()
        .flex_row()
        .overflow_hidden()
        .child(dynamic(move || {
            if active.get() != Some(Mode::File) {
                return view().into_element();
            }
            let vp = viewport.get();
            let lines = if scrolling.get() { Vec::new() } else { visible_lines(&vp) };
            let p = current_palette();
            let height = size.get().height;
            let labels = ed_ref
                .api::<EditorApi>()
                .map(|api| {
                    lines
                        .into_iter()
                        .filter_map(move |line| {
                            let bounds = row_box(&api, line.number, 0)?;
                            row_in_view(bounds, height)
                                .then(|| label(line, bounds, px(4.0), px(152.0), false, p))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            view()
                .test_id("blame-gutter")
                .relative()
                .w(px(156.0))
                .flex_none()
                .overflow_hidden()
                .bg(p.rail_bg)
                .border_r_1()
                .border_color(p.border)
                .children(labels)
                .into_element()
        }))
        .child(
            view()
                .relative()
                .flex_1()
                .overflow_hidden()
                .on_pinch(super::font_zoom::FontArea::Source.pinch())
                .on_wheel(move |_: &WheelEvent| {
                    if active.get_untracked().is_some() {
                        last_wheel.replace(Some(nsh_core::time::now()));
                        scrolling.set(true);
                    }
                })
                .on_layout(move |bounds: &Bounds<Points>| {
                    if size.get_untracked() != bounds.size {
                        size.set(bounds.size);
                    }
                })
                .child(editor)
                .child(dynamic(move || {
                    let Some(mode) = active.get() else {
                        return view().into_element();
                    };
                    let vp = viewport.get();
                    let Some((source, _)) = active_source() else {
                        return view().into_element();
                    };
                    let state = attribution().get();
                    if state
                        .request
                        .as_ref()
                        .is_none_or(|request| request.source != source)
                    {
                        return view().into_element();
                    }
                    let p = current_palette();
                    if state.loading || state.error.is_some() {
                        return view()
                            .test_id("blame-status")
                            .absolute()
                            .right(px(10.0))
                            .bottom(px(6.0))
                            .bg(p.surface2)
                            .px(px(6.0))
                            .py(px(2.0))
                            .child(
                                text(state.error.unwrap_or_else(|| "正在读取 Git 归属…".into()))
                                    .text_size(px(10.5))
                                    .text_color(p.text_muted),
                            )
                            .into_element();
                    }
                    if mode != Mode::Line || scrolling.get() || !vp.shows_line(vp.line) {
                        return view().into_element();
                    }
                    let Some(line) = state
                        .lines
                        .iter()
                        .find(|line| line.number == vp.line)
                        .cloned()
                    else {
                        return view().into_element();
                    };
                    let Some(bounds) = ed_ref
                        .api::<EditorApi>()
                        .and_then(|api| row_box(&api, vp.line, u32::MAX))
                    else {
                        return view().into_element();
                    };
                    let next = bounds.origin.x + px(12.0);
                    let size = size.get();
                    if !row_in_view(bounds, size.height) {
                        return view().into_element();
                    }
                    let width = size.width;
                    if width.0 - next.0 >= 94.0 {
                        label(line, bounds, next, width - next - px(6.0), false, p)
                    } else {
                        label(line, bounds, (width - px(26.0)).max(px(0.0)), px(22.0), true, p)
                    }
                })),
        )
}

fn visible_lines(view: &Viewport) -> Vec<Line> {
    let source = active_source().map(|(source, _)| source);
    attribution().with(|s| {
        if s.request.as_ref().map(|request| &request.source) != source.as_ref() {
            return Vec::new();
        }
        s.lines
            .iter()
            .filter(|line| view.shows_line(line.number))
            .cloned()
            .collect()
    })
}

fn row_box(api: &EditorApi, number: u32, col: u32) -> Option<Bounds<Points>> {
    let pos = EditorPosition::new(number.saturating_sub(1), col);
    api.range_bounds(EditorRange::new(pos, pos))
        .first()
        .map(|b| b.bounds)
}

fn row_in_view(bounds: Bounds<Points>, height: Points) -> bool {
    bounds.origin.y < height && bounds.origin.y.0 + bounds.size.height.0 > 0.0
}

fn label(
    line: Line,
    bounds: Bounds<Points>,
    left: Points,
    width: Points,
    compact: bool,
    p: crate::theme::Palette,
) -> AnyElement {
    let date = chrono::DateTime::from_timestamp(line.author_unix, 0)
        .map(|date| date.with_timezone(&chrono::Local));
    let pending = line.hash.is_empty();
    let name = if pending {
        "未提交的本地修改".into()
    } else {
        format!("{} · {}", line.author, date.map_or(String::new(), |date| date.format("%Y-%m-%d").to_string()))
    };
    let hint = if pending {
        name.clone()
    } else {
        format!(
            "{} · {} · {} · {}",
            line.hash.get(..8).unwrap_or(&line.hash),
            line.author,
            date.map_or(String::new(), |date| date.format("%Y-%m-%d %H:%M").to_string()),
            line.summary
        )
    };
    view()
        .test_id(&format!("blame-line-{}", line.number))
        .absolute()
        .top(bounds.origin.y)
        .left(left)
        .w(width)
        .h(bounds.size.height)
        .px(px(4.0))
        .justify_center()
        .overflow_hidden()
        .when(compact, |label| label.items_center().bg(p.rail_bg))
        .label(hint.clone())
        .tooltip(super::tooltip_label(hint, p))
        .cursor(if pending {
            Cursor::Default
        } else {
            Cursor::Pointer
        })
        .on_press(move || {
            if !pending {
                reveal(line.hash.clone());
            }
        })
        .child(if compact {
            super::icons::icon(super::icons::ICO_BRANCH, 12.0, p.text_control).into_element()
        } else {
            text(name)
                .text_size(px(10.0))
                .text_color(p.text_muted)
                .number_of_lines(1u32)
                .into_element()
        })
        .into_element()
}
