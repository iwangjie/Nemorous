use std::collections::HashMap;

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
        let history = crate::state::DocumentUndo::default();
        history.record_source_replacement("x😀終", "x😃終");
        let caret = EditorPosition::new(0, 1);
        assert_eq!(history.step("x😃終", false), Some(("x😀終".to_owned(), caret)));
        assert_eq!(history.step("x😀終", true), Some(("x😃終".to_owned(), caret)));
        assert_eq!(history.step("x😺終", false), None);
        assert_eq!(history.step("x😃終", false), None);
    }
}
