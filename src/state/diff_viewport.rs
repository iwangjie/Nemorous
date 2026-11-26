use nsh_base::EditorFoldRange;
use nsh_core::geometry::{Points, point, px};

use super::*;
use crate::diff_model::{
    DiffHunk, UnifiedLineRef, compute_unchanged_fold_ranges, compute_unified_layout, logical_line_count,
    map_new_line_to_old, map_old_line_to_new, unified_line_for_side, unified_source_eof,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffViewport {
    single_column: bool,
    old: Option<(i32, i32)>,
    new: Option<(i32, i32)>,
}

#[derive(Clone, Default)]
struct LayoutSwitch(u64);

struct DiffText<'a> {
    revision: u64,
    active_side: DiffSide,
    old: &'a str,
    new: &'a str,
    computed: &'a ComputedDiff,
    source_editable: bool,
    comparable: bool,
    caret: EditorPosition,
    boundary: Option<i32>,
}

impl<'a> DiffText<'a> {
    fn active(state: &'a AppState) -> Option<Self> {
        match state.editor_tab {
            EditorTabKind::WorkingDiff => {
                let diff = state.working_diff.as_ref()?;
                Some(Self {
                    revision: diff.content_rev,
                    active_side: diff.active_side,
                    old: &diff.data.old_text,
                    new: &diff.current_new_text,
                    computed: &diff.computed,
                    source_editable: diff.editor_editable(),
                    comparable: diff.can_compare_text(),
                    caret: diff.active_cursor(),
                    boundary: diff.boundary_hint,
                })
            }
            EditorTabKind::CommitDiff => {
                let diff = state.commit_diff.as_ref()?;
                Some(Self {
                    revision: diff.content_rev,
                    active_side: diff.active_side,
                    old: &diff.data.old_text,
                    new: &diff.data.new_text,
                    computed: &diff.computed,
                    source_editable: false,
                    comparable: !diff.data.binary && !diff.data.too_large,
                    caret: diff.active_cursor(),
                    boundary: diff.boundary_hint,
                })
            }
            _ => None,
        }
    }

    fn hunks(&self) -> &[DiffHunk] {
        &self.computed.hunks
    }

    fn active_text(&self) -> &str {
        match self.active_side {
            DiffSide::Old => self.old,
            DiffSide::New => self.new,
        }
    }
}

struct DiffCarets<'a> {
    active: &'a mut DiffSide,
    old: &'a mut EditorPosition,
    new: &'a mut EditorPosition,
    boundary: &'a mut Option<i32>,
}

impl<'a> DiffCarets<'a> {
    fn active(state: &'a mut AppState, tab: EditorTabKind) -> Option<Self> {
        if state.editor_tab != tab {
            return None;
        }
        match tab {
            EditorTabKind::WorkingDiff => state.working_diff.as_mut().map(|diff| Self {
                active: &mut diff.active_side, old: &mut diff.old_cursor,
                new: &mut diff.new_cursor, boundary: &mut diff.boundary_hint,
            }),
            EditorTabKind::CommitDiff => state.commit_diff.as_mut().map(|diff| Self {
                active: &mut diff.active_side, old: &mut diff.old_cursor,
                new: &mut diff.new_cursor, boundary: &mut diff.boundary_hint,
            }),
            _ => None,
        }
    }

    fn select(&mut self, side: DiffSide) {
        if *self.active != side {
            *self.active = side;
            *self.boundary = None;
        }
    }

    fn record(&mut self, side: DiffSide, position: EditorPosition) {
        let cursor = match side {
            DiffSide::Old => &mut *self.old,
            DiffSide::New => &mut *self.new,
        };
        if *cursor != position && *self.active == side {
            *self.boundary = None;
        }
        *cursor = position;
    }
}

pub(crate) fn focus_diff_side(tab: EditorTabKind, side: DiffSide) {
    let Some((old, new)) = diff_zones(tab) else { return };
    app_signal().update(|s| {
        let Some(mut carets) = DiffCarets::active(s, tab) else { return };
        carets.select(side);
        s.focus_zone = if side == DiffSide::Old { old } else { new };
        if s.diff_find_open && !s.cfg.diff_single_column {
            s.diff_find_side = side;
        }
    });
}

pub(crate) fn record_diff_caret(tab: EditorTabKind, side: DiffSide, position: EditorPosition) {
    let Some((old, new)) = diff_zones(tab) else { return };
    app_signal().update(|s| {
        let unified = s.cfg.diff_single_column;
        let Some(mut carets) = DiffCarets::active(s, tab) else { return };
        if unified {
            carets.select(side);
        }
        carets.record(side, position);
        if unified && (s.focus_zone == old || s.focus_zone == new) {
            s.focus_zone = if side == DiffSide::Old { old } else { new };
        }
    });
}

pub(crate) fn copy_diff_selection(tab: EditorTabKind, side: DiffSide) {
    let Some((old, new)) = diff_zones(tab) else { return };
    if app_signal().with_untracked(|s| s.editor_tab == tab) {
        copy_editor_selection(if side == DiffSide::Old { old } else { new });
    }
}

pub(super) fn reveal_opened_diff_hunk(tab: EditorTabKind, index: usize) {
    let revision = move |s: &AppState| {
        if s.editor_tab != tab {
            return None;
        }
        DiffText::active(s).map(|diff| diff.revision)
    };
    let Some(current) = app_signal().with_untracked(revision) else { return };
    let reveal = move || {
        if app_signal().with_untracked(revision) == Some(current) {
            scroll_to_diff_hunk(tab, index);
        }
    };
    // Wait for the replacement editor and its folds before revealing the initial difference.
    if let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) {
        owner.run(|| after_answer(reveal));
    } else {
        after_answer(reveal);
    }
}

pub fn jump_diff_hunk(delta: i32) {
    let direction = delta.signum();
    if direction == 0 {
        return;
    }
    let info = app_signal().with_untracked(|s| {
        let diff = DiffText::active(s).filter(|diff| diff.comparable)?;
        let split_text = (!s.cfg.diff_single_column).then(|| diff.active_text());
        let position = diff.computed.hunk_navigation(diff.active_side, diff.caret, split_text);
        let can_switch_files = s.editor_tab == EditorTabKind::WorkingDiff
            || s.commit_diff.as_ref().is_some_and(|session| session.scope == CommitDiffScope::AllFiles);
        Some((s.editor_tab, position, diff.boundary, can_switch_files))
    });
    let Some((tab, position, boundary, can_switch_files)) = info else { return };
    let target = if direction < 0 { position.previous } else { position.next };
    if let Some(index) = target {
        scroll_to_diff_hunk(tab, index);
        return;
    }
    if let Some(index) = position.current.filter(|&index| diff_hunk_is_offscreen(tab, index)) {
        scroll_to_diff_hunk(tab, index);
        return;
    }
    if !can_switch_files {
        return;
    }
    if boundary == Some(direction) {
        let moved = if tab == EditorTabKind::WorkingDiff {
            move_change_selection_with_hunk(direction, true, direction < 0)
        } else {
            diff_open::move_commit_diff_file_with_hunk(direction, direction < 0, true)
        };
        if !moved {
            let message = match (tab, direction) {
                (EditorTabKind::WorkingDiff, -1) => "已是第一个变更文件",
                (EditorTabKind::WorkingDiff, _) => "已是最后一个变更文件",
                (_, -1) => "已是本提交第一个文件",
                _ => "已是本提交最后一个文件",
            };
            show_toast("info", message);
        }
    } else {
        app_signal().update(|s| {
            if let Some(carets) = DiffCarets::active(s, tab) {
                *carets.boundary = Some(direction);
            }
        });
    }
}

fn diff_hunk_is_offscreen(tab: EditorTabKind, index: usize) -> bool {
    let Some((old_zone, new_zone)) = diff_zones(tab) else { return false };
    let target = app_signal().with_untracked(|s| {
        let diff = DiffText::active(s)?;
        let hunk = diff.hunks().get(index)?;
        if s.cfg.diff_single_column {
            let (_, starts, _) = compute_unified_layout(diff.old, diff.new, diff.hunks());
            let start = *starts.get(index)?;
            let len = hunk.line_range(DiffSide::Old).len() + hunk.line_range(DiffSide::New).len();
            Some((new_zone, start..start + len))
        } else {
            let old_lines = hunk.line_range(DiffSide::Old);
            let new_lines = hunk.line_range(DiffSide::New);
            if new_lines.is_empty() || (diff.active_side == DiffSide::Old && !old_lines.is_empty()) {
                Some((old_zone, old_lines))
            } else {
                Some((new_zone, new_lines))
            }
        }
    });
    target.is_some_and(|(zone, lines)| {
        active_editor_api(zone).is_some_and(|api| {
            let visible = api.visible_range();
            let end = lines.end.max(lines.start + 1);
            end <= visible.start.line as usize || lines.start > visible.end.line as usize
        })
    })
}

fn scroll_to_diff_hunk(tab: EditorTabKind, index: usize) {
    let Some((old_zone, new_zone)) = diff_zones(tab) else { return };
    let info = app_signal().with_untracked(|s| {
        let diff = DiffText::active(s)?;
        let hunk = diff.hunks().get(index)?;
        let unified = if s.cfg.diff_single_column {
            let (lines, starts, _) = compute_unified_layout(diff.old, diff.new, diff.hunks());
            let start = *starts.get(index)?;
            Some((EditorPosition::new(start as u32, 2), *lines.get(start)?))
        } else {
            None
        };
        let old = diff.computed.split_hunk_position(hunk, DiffSide::Old, diff.old);
        let new = diff.computed.split_hunk_position(hunk, DiffSide::New, diff.new);
        Some((old, new, unified))
    });
    let Some((old, new, unified)) = info else { return };
    app_signal().update(|s| {
        if let Some(mut carets) = DiffCarets::active(s, tab) {
            *carets.old = old;
            *carets.new = new;
            *carets.boundary = None;
            if let Some((_, row)) = unified {
                carets.select(row.side);
            }
        }
    });
    let reveal = |zone, position| {
        if let Some(api) = active_editor_api(zone) {
            api.set_selections(vec![EditorSelection::caret(position)]);
            api.reveal_position(position, true);
        }
    };
    if let Some((position, _)) = unified {
        reveal(new_zone, position);
    } else {
        reveal(old_zone, old);
        reveal(new_zone, new);
    }
}

struct LayoutAnchor {
    position: EditorPosition,
    top: Points,
    horizontal: Points,
}

impl LayoutAnchor {
    fn capture(api: &EditorApi) -> Self {
        let position = api.visible_range().start;
        let offset = api.scroll_offset();
        Self {
            position,
            top: api
                .scroll_y_for_position(position)
                .map(|y| y - offset.y)
                .unwrap_or_default(),
            horizontal: offset.x,
        }
    }
}

pub(crate) fn sync_diff_scroll(tab: EditorTabKind, old: Rc<EditorApi>, new: Rc<EditorApi>) {
    let settings = computed(move || {
        app_signal().with(|s| {
            if s.editor_tab != tab || s.cfg.diff_single_column || !s.cfg.diff_sync_scroll {
                return None;
            }
            DiffText::active(s)
                .filter(|diff| diff.comparable)
                .map(|diff| (diff.revision, s.cfg.diff_collapse_unchanged))
        })
    });
    let (old_x, new_x) = (old.clone(), new.clone());
    let previous = stored(None);
    effect(move || {
        let Some(settings) = settings.get() else {
            previous.replace(None);
            return;
        };
        let offsets = (old.vertical_scroll_offset(), new.vertical_scroll_offset());
        let last = previous.get();
        previous.replace(Some((settings, offsets)));
        let side = match last {
            Some((prior_settings, (old_y, new_y))) if prior_settings == settings => {
                match (old_y != offsets.0, new_y != offsets.1) {
                    (true, false) => DiffSide::Old,
                    (false, true) => DiffSide::New,
                    // Hunk navigation and viewport restoration can move both editors together.
                    _ => return,
                }
            }
            Some(_) => return,
            None => app_signal().with_untracked(|s| {
                DiffText::active(s).map_or(DiffSide::New, |diff| diff.active_side)
            }),
        };
        let (source, companion) = match side {
            DiffSide::Old => (&old, &new),
            DiffSide::New => (&new, &old),
        };
        let anchor = LayoutAnchor::capture(source);
        let (line, next_line) = app_signal().with_untracked(|s| {
            let Some(diff) = DiffText::active(s) else { return (0, 0) };
            let map = match side {
                DiffSide::Old => map_old_line_to_new,
                DiffSide::New => map_new_line_to_old,
            };
            let line = anchor.position.line as usize;
            (map(line, diff.hunks()), map(line + 1, diff.hunks()))
        });
        let position = EditorPosition::new(line as u32, anchor.position.column);
        let Some(y) = companion.scroll_y_for_position(position) else { return };
        // Rows present on only one side share the same anchor on the other side.
        let top = if line == next_line { px(0.0) } else { anchor.top };
        let target_y = (y - top).clamp(px(0.0), companion.max_vertical_scroll_offset());
        if (companion.scroll_offset().y - target_y).abs() < px(0.5) {
            return;
        }
        let offsets = match side {
            DiffSide::Old => (offsets.0, target_y),
            DiffSide::New => (target_y, offsets.1),
        };
        previous.replace(Some((settings, offsets)));
        companion.invalidate_hover();
        companion.set_vertical_scroll_offset(target_y);
    });
    let horizontal = stored(None::<(Points, Points)>);
    effect(move || {
        if settings.get().is_none() {
            horizontal.replace(None);
            return;
        }
        let now = (old_x.horizontal_scroll_offset(), new_x.horizontal_scroll_offset());
        let last = horizontal.get().unwrap_or(now);
        horizontal.replace(Some(now));
        let (from, to) = match (last.0 != now.0, last.1 != now.1) {
            (true, false) => (&old_x, &new_x),
            (false, true) => (&new_x, &old_x),
            _ => return,
        };
        let x = from.horizontal_scroll_offset();
        to.set_horizontal_scroll_offset(x);
        horizontal.replace(Some((x, x)));
    });
}

struct TargetPane {
    zone: FocusZone,
    anchor: EditorPosition,
    folds: Vec<EditorFoldRange>,
}

struct Transfer {
    source: LayoutAnchor,
    panes: Vec<TargetPane>,
    caret: Option<(FocusZone, EditorPosition)>,
}

fn split_position(
    line: UnifiedLineRef,
    column: u32,
    side: DiffSide,
    hunks: &[DiffHunk],
) -> EditorPosition {
    let row = match (line.side, side) {
        (DiffSide::Old, DiffSide::New) => map_old_line_to_new(line.orig_line, hunks),
        (DiffSide::New, DiffSide::Old) => map_new_line_to_old(line.orig_line, hunks),
        _ => line.orig_line,
    };
    EditorPosition::new(row as u32, column.saturating_sub(2))
}

pub(crate) fn toggle_diff_layout(scrolled_side: Option<(u64, DiffSide)>) {
    let ticket = global::<LayoutSwitch>();
    ticket.update(|value| value.0 = value.0.wrapping_add(1));
    let switch = ticket.with_untracked(|value| value.0);
    let (tab, was_single, workspace, source) = app_signal().with_untracked(|s| {
        (
            s.editor_tab,
            s.cfg.diff_single_column,
            s.cfg.workspace_path.clone(),
            DiffText::active(s).map(|diff| (diff.revision, diff.active_side)),
        )
    });
    let transfer = source.and_then(|(revision, active_side)| {
        let (old_zone, new_zone) = diff_zones(tab)?;
        let zone = |side| {
            if side == DiffSide::Old {
                old_zone
            } else {
                new_zone
            }
        };
        let viewport_side = scrolled_side
            .filter(|&(rev, _)| rev == revision)
            .map_or(active_side, |(_, side)| side);
        let source_editor = active_editor_api(zone(viewport_side))?;
        let viewport = LayoutAnchor::capture(&source_editor);
        let caret = active_editor_api(zone(active_side))?
            .selections()
            .last()
            .map(|s| s.head);

        app_signal().with_untracked(|s| {
            let diff = DiffText::active(s)?;
            let (mut lines, hunk_starts, unified_folds) = compute_unified_layout(diff.old, diff.new, diff.hunks());
            if diff.source_editable {
                lines.extend(unified_source_eof(diff.new));
            }
            let collapse = s.cfg.diff_collapse_unchanged;
            let (panes, caret) = if was_single {
                let anchor = lines
                    .get(viewport.position.line as usize)
                    .or_else(|| lines.last())?;
                let pane = |side, text, ranges: Vec<_>| TargetPane {
                    zone: zone(side),
                    anchor: split_position(*anchor, viewport.position.column, side, diff.hunks()),
                    folds: if collapse {
                        compute_unchanged_fold_ranges(logical_line_count(text), &ranges, 3)
                    } else {
                        Vec::new()
                    },
                };
                let old = pane(
                    DiffSide::Old,
                    diff.old,
                    diff.hunks().iter().map(|h| h.old_start..h.old_end).collect(),
                );
                let new = pane(
                    DiffSide::New,
                    diff.new,
                    diff.hunks().iter().map(|h| h.new_start..h.new_end).collect(),
                );
                let caret = caret.and_then(|pos| {
                    let line = *lines.get(pos.line as usize)?;
                    Some((
                        zone(line.side),
                        split_position(line, pos.column, line.side, diff.hunks()),
                    ))
                });
                (vec![old, new], caret)
            } else {
                let to_unified = |side, pos: EditorPosition| {
                    let row = unified_line_for_side(&lines, diff.hunks(), side, pos.line as usize);
                    EditorPosition::new(row as u32, pos.column.saturating_add(2))
                };
                let pane = TargetPane {
                    zone: new_zone,
                    anchor: to_unified(viewport_side, viewport.position),
                    folds: if collapse { unified_folds } else { Vec::new() },
                };
                let caret = caret.map(|pos| {
                    let nav = diff.computed.hunk_navigation(active_side, pos, Some(diff.active_text()));
                    let empty_side = nav.current.filter(|&index| diff.hunks()[index].line_range(active_side).is_empty());
                    let position = if let Some(index) = empty_side {
                        EditorPosition::new(hunk_starts[index] as u32, 2)
                    } else {
                        to_unified(active_side, pos)
                    };
                    (new_zone, position)
                });
                (vec![pane], caret)
            };
            Some((
                revision,
                Transfer {
                    source: viewport,
                    panes,
                    caret,
                },
            ))
        })
    });

    app_signal().update(|s| {
        s.cfg.diff_single_column = !was_single;
        if let Some(carets) = DiffCarets::active(s, tab) {
            *carets.boundary = None;
        }
    });
    save_config_quiet();
    let Some((revision, mut transfer)) = transfer else {
        return;
    };
    let current = move || {
        ticket.with_untracked(|value| value.0 == switch)
            && app_signal().with_untracked(|s| {
                s.editor_tab == tab
                    && s.cfg.workspace_path == workspace
                    && s.cfg.diff_single_column != was_single
                    && DiffText::active(s).is_some_and(|diff| diff.revision == revision)
            })
    };

    // The replacement editor's geometry comes from the host layout pass.
    after_answer(move || {
        spawn(async move {
            for _ in 0..4 {
                next_frame().await;
                if !current() {
                    return;
                }
                let editors: Option<Vec<_>> = transfer
                    .panes
                    .iter()
                    .map(|pane| {
                        let api = active_editor_api(pane.zone)?;
                        api.scroll_y_for_position(pane.anchor)?;
                        Some(api)
                    })
                    .collect();
                let Some(editors) = editors else { continue };
                for (pane, api) in transfer.panes.iter_mut().zip(&editors) {
                    api.set_folds(std::mem::take(&mut pane.folds));
                }
                if let Some((zone, caret)) = transfer.caret
                    && let Some(api) = active_editor_api(zone)
                {
                    api.set_selections(vec![EditorSelection::caret(caret)]);
                }
                for (pane, api) in transfer.panes.iter().zip(editors) {
                    if let Some(top) = api.scroll_y_for_position(pane.anchor) {
                        api.set_scroll_offset(point(
                            transfer.source.horizontal,
                            top - transfer.source.top,
                        ));
                    }
                }
                return;
            }
        });
    });
}

pub(super) fn capture_diff_view() {
    let (tab, single_column) =
        app_signal().with_untracked(|s| (s.editor_tab, s.cfg.diff_single_column));
    let Some((old_zone, new_zone)) = diff_zones(tab) else {
        return;
    };
    let offset = |zone| {
        active_editor_api(zone).map(|api| {
            let offset = api.scroll_offset();
            (offset.x.0.round() as i32, offset.y.0.round() as i32)
        })
    };
    let old = if single_column {
        None
    } else {
        offset(old_zone)
    };
    let new = offset(new_zone);
    if old.is_none() && new.is_none() {
        return;
    }
    let viewport = Some(DiffViewport {
        single_column,
        old,
        new,
    });
    app_signal().update(|s| match tab {
        EditorTabKind::WorkingDiff => {
            if let Some(diff) = &mut s.working_diff {
                diff.viewport = viewport;
            }
        }
        EditorTabKind::CommitDiff => {
            if let Some(diff) = &mut s.commit_diff {
                diff.viewport = viewport;
            }
        }
        _ => {}
    });
}

pub(crate) fn restore_diff_view_after_render(
    tab: EditorTabKind,
    from_navigation: bool,
) {
    let Some((old_zone, new_zone)) = diff_zones(tab) else {
        return;
    };
    let snapshot = app_signal().with_untracked(|s| {
        match tab {
            EditorTabKind::WorkingDiff => s.working_diff.as_ref().map(|d| {
                (
                    d.content_rev,
                    d.active_side,
                    d.old_cursor,
                    d.new_cursor,
                    d.viewport,
                )
            }),
            EditorTabKind::CommitDiff => s.commit_diff.as_ref().map(|d| {
                (
                    d.content_rev,
                    d.active_side,
                    d.old_cursor,
                    d.new_cursor,
                    d.viewport,
                )
            }),
            _ => None,
        }
        .map(|(rev, side, old, new, viewport)| {
            (rev, side, old, new, viewport, s.cfg.diff_single_column)
        })
    });
    let Some((rev, side, old, new, viewport, single_column)) = snapshot else {
        return;
    };
    if viewport.is_none() && !from_navigation {
        return;
    }
    let restore = move || {
        let still_current = app_signal().with_untracked(|s| {
            s.editor_tab == tab
                && s.cfg.diff_single_column == single_column
                && match tab {
                    EditorTabKind::WorkingDiff => s
                        .working_diff
                        .as_ref()
                        .is_some_and(|d| d.content_rev == rev),
                    EditorTabKind::CommitDiff => {
                        s.commit_diff.as_ref().is_some_and(|d| d.content_rev == rev)
                    }
                    _ => false,
                }
        });
        if !still_current {
            return;
        }
        let active_caret = if side == DiffSide::Old { old } else { new };
        let viewport = viewport.filter(|view| view.single_column == single_column);
        if single_column || viewport.is_none() {
            reveal_in_active_diff(side, active_caret);
            if let Some((x, y)) = viewport.and_then(|view| view.new)
                && let Some(api) = active_editor_api(new_zone)
            {
                api.set_scroll_offset(point(px(x as f32), px(y as f32)));
            }
        } else if let Some(view) = viewport {
            for (zone, cursor, scroll) in [(old_zone, old, view.old), (new_zone, new, view.new)] {
                if let Some((x, y)) = scroll
                    && let Some(api) = active_editor_api(zone)
                {
                    api.set_selections(vec![EditorSelection::caret(cursor)]);
                    api.set_scroll_offset(point(px(x as f32), px(y as f32)));
                }
            }
        }
        if from_navigation {
            let zone = if side == DiffSide::Old { old_zone } else { new_zone };
            if app_signal().with_untracked(|s| s.focus_zone == zone)
                && !text_input_focus_signal().with_untracked(|focus| focus.focused)
            {
                focus_active_editor_ref();
            }
        }
    };
    if let Some(owner) = global::<RootOwnerHolder>().with_untracked(|root| root.owner) {
        owner.run(|| after_answer(restore));
    } else {
        after_answer(restore);
    }
}

fn diff_zones(tab: EditorTabKind) -> Option<(FocusZone, FocusZone)> {
    match tab {
        EditorTabKind::WorkingDiff => Some((FocusZone::WorkingDiffOld, FocusZone::WorkingDiffNew)),
        EditorTabKind::CommitDiff => Some((FocusZone::CommitDiffOld, FocusZone::CommitDiffNew)),
        _ => None,
    }
}
