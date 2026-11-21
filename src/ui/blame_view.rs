use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nsh_base::{Editor, EditorApi, EditorPosition, EditorRange};
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
        let tab = s.code_tabs.get(s.active_code_tab_idx)?;
        if tab.binary || tab.too_large {
            return None;
        }
        Some((
            Source {
                workspace: s.cfg.workspace_path.clone(),
                tab: tab.id.clone(),
                revision: tab.content_rev,
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
                Some((tab.file.clone(), tab.revision.clone(), count, bytes))
            });
            let Some((file, revision, count, bytes)) = data else {
                return;
            };
            spawn(async move {
                let outcome = blocking(move || {
                    if count < first {
                        return Ok(Vec::new());
                    }
                    blame::read(
                        Path::new(&file.repo_path),
                        &file.rel_path,
                        &revision,
                        first,
                        last.min(count),
                        bytes,
                    )
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
                        Ok(Ok(lines)) if !lines.is_empty() => s.lines = Arc::new(lines),
                        Ok(Ok(_)) => s.error = Some("没有可归属的行".into()),
                        Ok(Err(err)) => s.error = Some(format!("读取 Git 归属失败：{err}")),
                        Err(err) => s.error = Some(format!("读取 Git 归属失败：{err}")),
                    }
                });
            });
        })
    });
}

#[derive(Clone, Copy, PartialEq, Default)]
struct Viewport {
    first: u32,
    last: u32,
    line: u32,
    scroll: Point<Points>,
}

pub(super) fn editor_body(
    editor: impl IntoElement,
    ed_ref: ComponentRef<Editor>,
    side: Option<PaneId>,
) -> impl IntoElement {
    let size = signal(Size::new(px(0.0), px(0.0)));
    let viewport = signal(Viewport::default());
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
        };
        if viewport.get_untracked() != view {
            viewport.set(view);
        }
        if mode == Mode::File {
            request(
                source,
                view.first,
                view.last.min(view.first + blame::MAX_LINES - 1),
            );
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
            let lines = visible_lines(vp);
            let p = current_palette();
            let height = size.get().height;
            let labels = ed_ref
                .api::<EditorApi>()
                .map(|api| {
                    lines
                        .into_iter()
                        .filter_map(move |line| {
                            let bounds = row_box(&api, line.number, 0)?;
                            (bounds.origin.y < height
                                && bounds.origin.y.0 + bounds.size.height.0 > 0.0)
                                .then(|| label(line, bounds.origin.y, px(4.0), px(152.0), p))
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
                    if mode != Mode::Line || vp.line < vp.first || vp.line > vp.last {
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
                    let width = size.get().width;
                    if width.0 - next.0 >= 94.0 {
                        label(line, bounds.origin.y, next, width - next - px(6.0), p)
                    } else {
                        view()
                            .absolute()
                            .bottom(px(23.0))
                            .right(px(8.0))
                            .w(px(215.0))
                            .h(px(19.0))
                            .bg(p.rail_bg)
                            .child(label(line, px(0.0), px(0.0), px(215.0), p))
                            .into_element()
                    }
                })),
        )
}

fn visible_lines(view: Viewport) -> Vec<Line> {
    let source = active_source().map(|(source, _)| source);
    attribution().with(|s| {
        if s.request.as_ref().map(|request| &request.source) != source.as_ref() {
            return Vec::new();
        }
        s.lines
            .iter()
            .filter(|line| (view.first..=view.last).contains(&line.number))
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

fn label(
    line: Line,
    top: Points,
    left: Points,
    width: Points,
    p: crate::theme::Palette,
) -> AnyElement {
    let date = chrono::DateTime::from_timestamp(line.author_unix, 0)
        .map_or_else(String::new, |date| date.format("%Y-%m-%d").to_string());
    let pending = line.hash.is_empty();
    let name = if pending {
        "未提交的本地修改".into()
    } else {
        format!("{} · {date}", line.author)
    };
    let hint = if pending {
        name.clone()
    } else {
        format!(
            "{} · {} · {}",
            line.hash.get(..8).unwrap_or(&line.hash),
            name,
            line.summary
        )
    };
    view()
        .test_id(&format!("blame-line-{}", line.number))
        .absolute()
        .top(top)
        .left(left)
        .w(width)
        .h(px(19.0))
        .px(px(4.0))
        .justify_center()
        .overflow_hidden()
        .label(hint)
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
        .child(
            text(name)
                .text_size(px(10.0))
                .text_color(p.text_muted)
                .number_of_lines(1u32),
        )
        .into_element()
}
