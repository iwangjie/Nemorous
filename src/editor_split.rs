use std::collections::{HashMap, VecDeque};
use std::ops::Range;

use nsh_base::EditorPosition;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Left,
    Right,
}

impl PaneId {
    fn index(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewState {
    pub cursor: EditorPosition,
    pub scroll_x: f32,
    pub scroll_y: f32,
}

#[derive(Debug, Clone)]
pub struct Pane {
    tabs: Vec<String>,
    active: Option<String>,
    views: HashMap<String, ViewState>,
}

impl Pane {
    pub fn tabs(&self) -> &[String] {
        &self.tabs
    }

    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn view(&self, id: &str) -> Option<ViewState> {
        self.views.get(id).copied()
    }

    pub fn save_view(&mut self, id: &str, view: ViewState) {
        if self.tabs.iter().any(|tab| tab == id) {
            self.views.insert(id.to_string(), view);
        }
    }

    fn open(&mut self, id: &str) {
        if !self.tabs.iter().any(|tab| tab == id) {
            self.tabs.push(id.to_string());
        }
        self.active = Some(id.to_string());
    }

    fn retain(&mut self, keep: &impl Fn(&str) -> bool) {
        let previous = self
            .active()
            .and_then(|id| self.tabs.iter().position(|tab| tab == id));
        self.tabs.retain(|id| keep(id));
        self.views.retain(|id, _| keep(id));
        if self.active().is_some_and(|id| keep(id)) {
            return;
        }
        self.active = self
            .tabs
            .get(previous.unwrap_or_default())
            .or_else(|| self.tabs.last())
            .cloned();
    }
}

#[derive(Debug, Clone)]
pub struct EditorSplit {
    panes: [Pane; 2],
    pub focused: PaneId,
}

impl EditorSplit {
    pub fn new(tabs: Vec<String>, id: &str, view: ViewState) -> Self {
        let active = Some(id.to_string());
        let views = HashMap::from([(id.to_string(), view)]);
        Self {
            panes: [
                Pane {
                    tabs,
                    active: active.clone(),
                    views: views.clone(),
                },
                Pane {
                    tabs: vec![id.to_string()],
                    active,
                    views,
                },
            ],
            focused: PaneId::Right,
        }
    }

    pub fn pane(&self, pane: PaneId) -> &Pane {
        &self.panes[pane.index()]
    }

    pub fn pane_mut(&mut self, pane: PaneId) -> &mut Pane {
        &mut self.panes[pane.index()]
    }

    pub fn open(&mut self, id: &str) {
        self.pane_mut(self.focused).open(id);
    }

    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        for pane in &mut self.panes {
            pane.retain(&keep);
        }
        if self.pane(self.focused).active().is_none() {
            self.focused = if self.focused == PaneId::Left {
                PaneId::Right
            } else {
                PaneId::Left
            };
        }
    }

    pub fn has_two_panes(&self) -> bool {
        self.panes.iter().all(|pane| pane.active().is_some())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChangeSpan {
    pub start: usize,
    pub before_end: usize,
    pub after_end: usize,
}

impl ChangeSpan {
    pub fn between(before: &str, after: &str) -> Option<Self> {
        if before == after {
            return None;
        }

        let before_bytes = before.as_bytes();
        let after_bytes = after.as_bytes();
        let max_prefix = before_bytes.len().min(after_bytes.len());

        let mut prefix = 0;
        while prefix < max_prefix && before_bytes[prefix] == after_bytes[prefix] {
            prefix += 1;
        }

        while prefix > 0 && (!before.is_char_boundary(prefix) || !after.is_char_boundary(prefix)) {
            prefix -= 1;
        }

        let max_suffix = (before_bytes.len() - prefix).min(after_bytes.len() - prefix);
        let mut suffix = 0;
        while suffix < max_suffix
            && before_bytes[before_bytes.len() - 1 - suffix]
                == after_bytes[after_bytes.len() - 1 - suffix]
        {
            suffix += 1;
        }

        while suffix > 0
            && (!before.is_char_boundary(before_bytes.len() - suffix)
                || !after.is_char_boundary(after_bytes.len() - suffix))
        {
            suffix -= 1;
        }

        let start = prefix;
        let before_end = before_bytes.len() - suffix;
        let after_end = after_bytes.len() - suffix;

        if start == before_end && start == after_end {
            return None;
        }

        Some(Self {
            start,
            before_end,
            after_end,
        })
    }

    pub fn before_range(self) -> Range<usize> {
        self.start..self.before_end
    }

    pub fn after_range(self) -> Range<usize> {
        self.start..self.after_end
    }
}

const MAX_UNDO_ENTRIES: usize = 200;
const MAX_UNDO_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
struct UndoEntry {
    start: usize,
    before: String,
    after: String,
}

impl UndoEntry {
    fn size(&self) -> usize {
        self.before.len() + self.after.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct UndoLog {
    undo: VecDeque<UndoEntry>,
    redo: Vec<UndoEntry>,
    bytes: usize,
}

impl UndoLog {
    pub fn record(&mut self, before: &str, after: &str) {
        let Some(span) = ChangeSpan::between(before, after) else {
            return;
        };
        let before = &before[span.before_range()];
        let after = &after[span.after_range()];
        if before.len() + after.len() > MAX_UNDO_BYTES {
            self.clear();
            return;
        }
        self.bytes -= self.redo.drain(..).map(|entry| entry.size()).sum::<usize>();
        let entry = UndoEntry {
            start: span.start,
            before: before.to_string(),
            after: after.to_string(),
        };
        self.bytes += entry.size();
        self.undo.push_back(entry);
        while self.undo.len() > MAX_UNDO_ENTRIES || self.bytes > MAX_UNDO_BYTES {
            let Some(entry) = self.undo.pop_front() else {
                break;
            };
            self.bytes -= entry.size();
        }
    }

    /// Return the edit's start byte for restoring the active caret.
    pub fn apply(&mut self, text: &mut String, redo: bool) -> Option<usize> {
        let entry = if redo {
            self.redo.pop()?
        } else {
            self.undo.pop_back()?
        };
        let (expected, next) = if redo {
            (&entry.before, &entry.after)
        } else {
            (&entry.after, &entry.before)
        };
        let range = entry.start..entry.start + expected.len();
        if text.get(range.clone()) != Some(expected) {
            self.clear();
            return None;
        }
        text.replace_range(range, next);
        let offset = entry.start;
        if redo {
            self.undo.push_back(entry);
        } else {
            self.redo.push(entry);
        }
        Some(offset)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_document_has_independent_views_and_group_membership() {
        let first = ViewState {
            cursor: EditorPosition::new(3, 2),
            scroll_x: 4.0,
            scroll_y: 50.0,
        };
        let mut split = EditorSplit::new(vec!["a".into(), "b".into()], "a", first);
        let second = ViewState {
            cursor: EditorPosition::new(8, 1),
            scroll_x: 0.0,
            scroll_y: 120.0,
        };
        split.pane_mut(PaneId::Right).save_view("a", second);
        split.open("c");
        assert_eq!(split.pane(PaneId::Left).active(), Some("a"));
        assert_eq!(split.pane(PaneId::Right).active(), Some("c"));
        assert_eq!(split.pane(PaneId::Left).view("a"), Some(first));
        assert_eq!(split.pane(PaneId::Right).view("a"), Some(second));
        split.retain(|id| id == "b");
        assert!(!split.has_two_panes());
        assert_eq!(split.focused, PaneId::Left);
        assert_eq!(split.pane(PaneId::Left).tabs(), &["b"]);
    }

    #[test]
    fn shared_undo_uses_unicode_boundaries_and_rejects_stale_replay() {
        let mut log = UndoLog::default();
        log.record("x😀終", "x😃終");
        let mut text = "x😃終".to_string();
        assert_eq!(log.apply(&mut text, false), Some(1));
        assert_eq!(text, "x😀終");
        assert_eq!(log.apply(&mut text, true), Some(1));
        assert_eq!(text, "x😃終");
        text = "x😺終".into();
        assert_eq!(log.apply(&mut text, false), None);
        assert_eq!(text, "x😺終");
        assert!(log.is_empty());
    }
}
