use std::path::Path;
use std::sync::Arc;

use nsh_base::{Editor, EditorApi, EditorHoverEvent};
use nsh_core::{PointerEvents, prelude::*};

use crate::blame::{self, Line};
use crate::diff_model::{DiffSide, UnifiedLineRef};
use crate::state::{EditorTabKind, app_signal, current_palette};

#[derive(Clone, PartialEq, Eq)]
struct Target {
    repo: String,
    path: String,
    revision: blame::Revision,
    side: DiffSide,
}

struct Request {
    target: Target,
    last_line: u32,
}

impl Request {
    fn for_line(is_commit: bool, side: DiffSide, row: usize) -> Option<Self> {
        app_signal().with_untracked(|s| {
            let expected_tab = if is_commit { EditorTabKind::CommitDiff } else { EditorTabKind::WorkingDiff };
            if s.editor_tab != expected_tab {
                return None;
            }
            let (diff, repo, path, revision) = if is_commit {
                let session = s.commit_diff.as_ref()?;
                let data = &session.data;
                let (path, commit) = match side {
                    DiffSide::Old => (data.old_path.as_str(), data.parent_hash.as_str()),
                    DiffSide::New => (data.new_path.as_str(), data.commit_hash.as_str()),
                };
                if commit.is_empty() {
                    return None;
                }
                (&session.computed, &data.repo_path, path, blame::Revision::Commit(commit.to_owned()))
            } else {
                let session = s.working_diff.as_ref()?;
                if !session.can_compare_text() {
                    return None;
                }
                let data = &session.data;
                let ancestor = session.file.old_path_or_current();
                let working = || blame::Revision::Working(s.repository_head(&data.repo_path));
                let (path, revision) = match side {
                    DiffSide::Old if data.staged_only => {
                        (data.old_path.as_str(), blame::Revision::Commit(data.head_snapshot.clone()?))
                    }
                    DiffSide::Old if session.file.conflict => {
                        (data.new_path.as_str(), working())
                    }
                    _ => (ancestor, working()),
                };
                (&session.computed, &data.repo_path, path, revision)
            };
            let changed = diff.hunks.iter().any(|hunk| match side {
                DiffSide::Old => (hunk.old_start..hunk.old_end).contains(&row),
                DiffSide::New => (hunk.new_start..hunk.new_end).contains(&row),
            });
            if !changed {
                return None;
            }
            let count = match side {
                DiffSide::Old => diff.old_line_count,
                DiffSide::New => diff.new_line_count,
            };
            Some(Self {
                target: Target {
                    repo: repo.clone(),
                    path: path.to_owned(),
                    revision,
                    side,
                },
                last_line: count as u32,
            })
        })
    }

    fn buffer_contents(&self) -> Option<Vec<u8>> {
        app_signal().with_untracked(|s| {
            let diff = s.working_diff.as_ref()?;
            let text = match self.target.side {
                DiffSide::Old => &diff.data.old_text,
                DiffSide::New => &diff.current_new_text,
            };
            (text.len() <= crate::git::MAX_REVIEW_BYTES).then(|| text.as_bytes().to_vec())
        })
    }
}

#[derive(Clone, PartialEq)]
struct CachedLines {
    target: Target,
    first: u32,
    last: u32,
    lines: Arc<Vec<Line>>,
}

#[derive(Clone, PartialEq)]
struct Hint {
    anchor: Bounds<Points>,
    title: String,
    summary: String,
}

#[derive(Clone, Copy)]
pub(super) struct Hover {
    generation: Signal<u64>,
    cache: Signal<Option<CachedLines>>,
    hint: Signal<Option<Hint>>,
    wheel_position: Signal<Option<Point<Points>>>,
}

impl Hover {
    pub(super) fn new(is_commit: bool, editor: ComponentRef<Editor>) -> Self {
        let hover = Self {
            generation: signal(0),
            cache: signal(None),
            hint: signal(None),
            wheel_position: signal(None),
        };
        let layout = computed(move || {
            app_signal().with(|s| {
                let revision = if is_commit {
                    s.commit_diff.as_ref().map(|diff| (diff.content_rev, true, None))
                } else {
                    s.working_diff.as_ref().map(|diff| (
                        diff.content_rev,
                        diff.can_compare_text(),
                        Some(s.repository_head(&diff.data.repo_path)),
                    ))
                };
                (
                    revision,
                    s.editor_tab,
                    s.cfg.diff_collapse_unchanged,
                    s.cfg.editor_soft_wrap,
                    s.cfg.editor_font_size,
                )
            })
        });
        effect(move || {
            layout.get();
            hover.invalidate();
            hover.wheel_position.set(None);
            if let Some(api) = editor.api::<EditorApi>() {
                api.invalidate_hover();
            }
        });
        hover
    }

    pub(super) fn clear(self) {
        self.generation.update(|id| *id = id.wrapping_add(1));
        if self.hint.with_untracked(Option::is_some) {
            self.hint.set(None);
        }
    }

    pub(super) fn invalidate(self) {
        self.clear();
        if self.cache.with_untracked(Option::is_some) {
            self.cache.set(None);
        }
    }

    pub(super) fn scroll(self, ev: &WheelEvent) {
        self.wheel_position.set(Some(ev.page_position));
        self.clear();
    }

    pub(super) fn show(self, is_commit: bool, side: DiffSide, row: usize, ev: &EditorHoverEvent) {
        self.clear();
        if let Some(point) = self.wheel_position.get_untracked() {
            // AppKit can hit-test the resting pointer again as rows scroll below it.
            if (ev.page_position.x - point.x).abs() < px(1.0)
                && (ev.page_position.y - point.y).abs() < px(1.0)
            {
                return;
            }
            self.wheel_position.set(None);
        }
        let Some(request) = Request::for_line(is_commit, side, row) else {
            return;
        };
        let number = row as u32 + 1;
        let anchor = ev.page_bounds;
        if let Some(cached) = self.cache.with_untracked(|cache| {
            cache
                .as_ref()
                .filter(|cached| {
                    cached.target == request.target
                        && (cached.first..=cached.last).contains(&number)
                })
                .cloned()
        }) {
            self.show_line(&cached, number, anchor);
            return;
        }

        let from_buffer = request.target.revision.uses_buffer();
        let contents = if from_buffer {
            let Some(contents) = request.buffer_contents() else {
                self.show_error(anchor);
                return;
            };
            Some(contents)
        } else {
            None
        };
        let generation = self.generation.get_untracked();
        let first = (row as u32 / blame::MAX_LINES) * blame::MAX_LINES + 1;
        let last = (first + blame::MAX_LINES - 1).min(request.last_line);
        let target = request.target;
        spawn(async move {
            let read_target = target.clone();
            let result = blocking(move || {
                read_target.revision.read(
                    Path::new(&read_target.repo),
                    &read_target.path,
                    first,
                    last,
                    contents,
                )
            })
            .await;
            if self.generation.get_untracked() != generation {
                return;
            }
            match result {
                Ok(Ok(Some(lines))) if from_buffer || !lines.is_empty() => {
                    let cached = CachedLines {
                        target,
                        first,
                        last,
                        lines: Arc::new(lines),
                    };
                    self.show_line(&cached, number, anchor);
                    self.cache.set(Some(cached));
                }
                Ok(Ok(None)) => {}
                _ => self.show_error(anchor),
            }
        });
    }

    fn show_error(self, anchor: Bounds<Points>) {
        self.hint.set(Some(Hint {
            anchor,
            title: "无法读取 Git 归属".into(),
            summary: String::new(),
        }));
    }

    pub(super) fn show_unified(
        self,
        is_commit: bool,
        lines: &[UnifiedLineRef],
        ev: &EditorHoverEvent,
    ) {
        if let Some(line) = lines.get(ev.position.line as usize) {
            self.show(is_commit, line.side, line.orig_line, ev);
        } else {
            self.clear();
        }
    }

    fn show_line(self, cached: &CachedLines, number: u32, anchor: Bounds<Points>) {
        let committed = cached
            .lines
            .iter()
            .find(|line| line.number == number && !line.hash.is_empty());
        let (title, summary) = committed.map_or_else(
            || ("未提交的本地修改".into(), String::new()),
            |line| {
                let date = chrono::DateTime::from_timestamp(line.author_unix, 0)
                    .map(|date| {
                        date.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                (format!("{} · {date}", line.author), line.summary.clone())
            },
        );
        self.hint.set(Some(Hint {
            anchor,
            title,
            summary,
        }));
    }

    pub(super) fn tooltip(self) -> impl IntoElement {
        dynamic(move || {
            let Some(hint) = self.hint.get() else {
                return view().into_element();
            };
            let p = current_palette();
            overlay()
                .anchor_bounds(move || hint.anchor)
                .align(Align::Start)
                .offset(px(4.0))
                .margin(px(8.0))
                .on_cancel(move || self.clear())
                .child(
                    view()
                        .test_id("diff-blame-tooltip")
                        .pointer_events(PointerEvents::None)
                        .max_w(px(340.0))
                        .px(px(8.0))
                        .py(px(5.0))
                        .gap(px(3.0))
                        .bg(p.surface2)
                        .border_1()
                        .border_color(p.border_strong)
                        .rounded(px(4.0))
                        .child(
                            text(hint.title)
                                .text_size(px(11.5))
                                .text_color(p.text_primary),
                        )
                        .child(when(!hint.summary.is_empty(), move || {
                            text(hint.summary.clone())
                                .text_size(px(10.5))
                                .text_color(p.text_muted)
                        })),
                )
                .into_element()
        })
    }
}
