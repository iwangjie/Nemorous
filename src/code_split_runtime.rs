use nsh_base::{Editor, EditorApi, EditorPosition};
use nsh_core::prelude::*;

use crate::editor_split::{EditorSplit, PaneId, ViewState};
use crate::state::{self, AppState, EditorTabKind, FocusZone, app_signal, editor_refs_signal};

#[derive(Clone)]
pub(crate) struct PaneSnapshot {
    pub id: String,
    pub version: u64,
    pub text: String,
    pub cursor: EditorPosition,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub active: bool,
}

pub(crate) fn pane_key(side: PaneId) -> Option<(String, u64)> {
    app_signal().with(|s| {
        let id = s.code_split.as_ref()?.pane(side).active()?;
        let tab = s.code_tabs.iter().find(|tab| tab.id == id)?;
        Some((tab.id.clone(), tab.content_rev))
    })
}

pub(crate) fn pane(side: PaneId) -> Option<PaneSnapshot> {
    app_signal().with_untracked(|s| {
        let split = s.code_split.as_ref()?;
        let pane = split.pane(side);
        let id = pane.active()?;
        let tab = s.code_tabs.iter().find(|tab| tab.id == id)?;
        let view = pane.view(id).unwrap_or(ViewState {
            cursor: tab.cursor,
            scroll_x: tab.scroll_x,
            scroll_y: tab.scroll_y,
        });
        Some(PaneSnapshot {
            id: tab.id.clone(),
            version: tab.content_rev,
            text: tab.text.clone(),
            cursor: view.cursor,
            scroll_x: view.scroll_x,
            scroll_y: view.scroll_y,
            active: split.focused == side && s.focus_zone == FocusZone::CodeEditor,
        })
    })
}

pub(crate) fn file_properties(side: PaneId) -> Option<(String, bool)> {
    app_signal().with(|s| {
        let id = s.code_split.as_ref()?.pane(side).active()?;
        let tab = s.code_tabs.iter().find(|tab| tab.id == id)?;
        Some((tab.file.rel_path.clone(), tab.editable))
    })
}

pub(crate) fn tab_list(side: PaneId) -> Vec<(String, String, bool)> {
    app_signal().with(|s| {
        let Some(split) = &s.code_split else {
            return Vec::new();
        };
        let pane = split.pane(side);
        pane.tabs()
            .iter()
            .filter_map(|id| {
                let tab = s.code_tabs.iter().find(|tab| tab.id == *id)?;
                Some((
                    tab.id.clone(),
                    tab.title(),
                    pane.active() == Some(id.as_str()),
                ))
            })
            .collect()
    })
}

pub(crate) fn split_for_tab(id: &str) {
    let is_visible = app_signal().with_untracked(|s| {
        s.editor_tab == EditorTabKind::File
            && s.code_tabs.get(s.active_code_tab_idx).is_some_and(|tab| tab.id == id)
    });
    let offset = is_visible.then(|| state::active_editor_api(FocusZone::CodeEditor))
        .flatten().map(|api| api.scroll_offset());
    app_signal().update(|s| {
        if s.code_split.is_some() {
            return;
        }
        let Some(index) = s.code_tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let tab = &s.code_tabs[index];
        let view = ViewState {
            cursor: tab.cursor,
            scroll_x: offset.map_or(tab.scroll_x, |point| point.x.0),
            scroll_y: offset.map_or(tab.scroll_y, |point| point.y.0),
        };
        let tabs = s.code_tabs.iter().map(|tab| tab.id.clone()).collect();
        s.code_split = Some(EditorSplit::new(tabs, id, view));
        s.code_tabs[index].preview = false;
        s.active_code_tab_idx = index;
        s.editor_tab = EditorTabKind::File;
        s.focus_zone = FocusZone::CodeEditor;
    });
}

pub(crate) fn merge() {
    capture_views();
    app_signal().update(|s| {
        if let Some(split) = s.code_split.take() {
            preserve_view(s, &split);
            s.focus_zone = FocusZone::CodeEditor;
        }
    });
    state::focus_active_editor_ref();
}

fn preserve_view(s: &mut AppState, split: &EditorSplit) {
    let pane = split.pane(split.focused);
    if let Some(id) = pane.active()
        && let Some(index) = s.code_tabs.iter().position(|tab| tab.id == id)
    {
        s.active_code_tab_idx = index;
        if let Some(view) = pane.view(id) {
            let tab = &mut s.code_tabs[index];
            tab.cursor = view.cursor;
            tab.scroll_x = view.scroll_x;
            tab.scroll_y = view.scroll_y;
        }
    }
}

pub(crate) fn select_tab_in_model(s: &mut AppState, index: usize) {
    s.active_code_tab_idx = index;
    if let Some(split) = &mut s.code_split
        && let Some(tab) = s.code_tabs.get_mut(index)
    {
        split.open(&tab.id);
        if let Some(view) = split.pane(split.focused).view(&tab.id) {
            tab.cursor = view.cursor;
        }
    }
}

pub(crate) fn prune_closed_tabs(s: &mut AppState) {
    s.code_undo
        .retain(|id, _| s.code_tabs.iter().any(|tab| &tab.id == id));
    let Some(mut split) = s.code_split.take() else {
        return;
    };
    split.retain(|id| s.code_tabs.iter().any(|tab| tab.id == id));
    preserve_view(s, &split);
    if split.has_two_panes() {
        s.code_split = Some(split);
    }
}

pub(crate) fn select(side: PaneId, id: &str) {
    app_signal().update(|s| {
        let Some(index) = s.code_tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let Some(split) = &mut s.code_split else {
            return;
        };
        split.focused = side;
        select_tab_in_model(s, index);
        state::sync_dirty_working_diff_into_code_tab(s, index);
        s.focus_zone = FocusZone::CodeEditor;
    });
}

pub(crate) fn focus(side: PaneId) {
    if app_signal().with_untracked(|s| s.focus_zone == FocusZone::Modal) {
        return;
    }
    let id = app_signal().with_untracked(|s| {
        s.code_split
            .as_ref()?
            .pane(side)
            .active()
            .map(str::to_string)
    });
    if let Some(id) = id {
        select(side, &id);
    }
}

pub(crate) fn register_ref(side: PaneId, reference: ComponentRef<Editor>) {
    editor_refs_signal().update(|refs| match side {
        PaneId::Left => refs.split_left = Some(reference),
        PaneId::Right => refs.split_right = Some(reference),
    });
}

pub(crate) fn capture_views() {
    let refs = editor_refs_signal().get_untracked();
    for (side, reference) in [
        (PaneId::Left, refs.split_left),
        (PaneId::Right, refs.split_right),
    ] {
        if let Some((id, _)) = pane_key_untracked(side)
            && let Some(api) = reference.and_then(|reference| reference.api::<EditorApi>())
            && let Some(selection) = api.selections().last()
        {
            let offset = api.scroll_offset();
            save_view(side, &id, selection.head, offset.x.0, offset.y.0);
        }
    }
}

fn pane_key_untracked(side: PaneId) -> Option<(String, u64)> {
    pane(side).map(|pane| (pane.id, pane.version))
}

pub(crate) fn save_view(
    side: PaneId,
    id: &str,
    cursor: EditorPosition,
    scroll_x: f32,
    scroll_y: f32,
) {
    app_signal().update(|s| {
        let Some(split) = &mut s.code_split else {
            return;
        };
        let view = ViewState {
            cursor,
            scroll_x,
            scroll_y,
        };
        split.pane_mut(side).save_view(id, view);
        if split.focused == side
            && split.pane(side).active() == Some(id)
            && let Some(tab) = s
                .code_tabs
                .get_mut(s.active_code_tab_idx)
                .filter(|tab| tab.id == id)
        {
            tab.cursor = cursor;
            tab.scroll_x = scroll_x;
            tab.scroll_y = scroll_y;
        }
    });
}

pub(crate) fn record_editor_change(id: &str, value: &str) {
    app_signal().update(|s| {
        let Some(tab) = s.code_tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if !tab.editable || tab.text == value {
            return;
        }
        s.code_undo
            .entry(id.to_string())
            .or_default()
            .record(&tab.text, value);
        tab.text = value.to_string();
        if tab.is_dirty() {
            tab.preview = false;
        }
        tab.content_rev += 1;
    });
}

pub(crate) fn change(side: PaneId, id: &str, value: &str) {
    let same = app_signal().with_untracked(|s| {
        s.code_split
            .as_ref()
            .is_some_and(|split| split.pane(side).active() == Some(id))
    });
    if same {
        record_editor_change(id, value);
    }
}

pub(crate) fn set_active_position(s: &mut AppState, position: EditorPosition) {
    let Some(tab) = s.code_tabs.get_mut(s.active_code_tab_idx) else {
        return;
    };
    tab.cursor = position;
    if let Some(split) = &mut s.code_split {
        let pane = split.pane_mut(split.focused);
        let mut view = pane.view(&tab.id).unwrap_or(ViewState {
            cursor: position,
            scroll_x: tab.scroll_x,
            scroll_y: tab.scroll_y,
        });
        view.cursor = position;
        pane.save_view(&tab.id, view);
    }
}

pub(crate) fn undo(redo: bool) {
    app_signal().update(|s| {
        let Some(tab) = s.code_tabs.get_mut(s.active_code_tab_idx) else {
            return;
        };
        let Some(log) = s.code_undo.get_mut(&tab.id) else {
            return;
        };
        let Some(offset) = log.apply(&mut tab.text, redo) else {
            return;
        };
        tab.content_rev += 1;
        let before = &tab.text[..offset];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32;
        let column = before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .encode_utf16()
            .count() as u32;
        set_active_position(s, EditorPosition::new(line, column));
    });
}
