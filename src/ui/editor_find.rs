use nsh_base::{EditorSearch, EditorSearchResult};
use nsh_core::prelude::*;

use crate::state::{AppState, app_signal, current_palette};

#[derive(Clone, Copy)]
pub(super) enum FindBar {
    Source,
    Diff,
}

#[derive(Clone, Copy)]
enum SearchMode {
    MatchCase,
    WholeWord,
    Regex,
}

impl SearchMode {
    fn enabled(self, search: &EditorSearch) -> bool {
        match self {
            Self::MatchCase => search.case_sensitive,
            Self::WholeWord => search.whole_word,
            Self::Regex => search.regex,
        }
    }

    fn toggle(self, search: &mut EditorSearch) {
        let flag = match self {
            Self::MatchCase => &mut search.case_sensitive,
            Self::WholeWord => &mut search.whole_word,
            Self::Regex => &mut search.regex,
        };
        *flag = !*flag;
    }
}

impl FindBar {
    fn search_mut(self, state: &mut AppState) -> &mut EditorSearch {
        match self {
            Self::Source => &mut state.code_search,
            Self::Diff => &mut state.diff_search,
        }
    }

    pub(super) fn options(self) -> impl IntoElement {
        let prefix = match self {
            Self::Source => "code",
            Self::Diff => "diff",
        };
        view()
            .flex_row()
            .flex_none()
            .gap(px(2.0))
            .child(self.option(prefix, SearchMode::MatchCase))
            .child(self.option(prefix, SearchMode::WholeWord))
            .child(self.option(prefix, SearchMode::Regex))
    }

    fn option(
        self,
        prefix: &'static str,
        mode: SearchMode,
    ) -> impl IntoElement {
        let (id, text_label, description) = match mode {
            SearchMode::MatchCase => ("case", "Cc", "区分大小写"),
            SearchMode::WholeWord => ("word", "词", "全字匹配"),
            SearchMode::Regex => ("regex", ".*", "正则表达式"),
        };
        let active = computed(move || app_signal().with(|state| {
            let search = match self {
                Self::Source => &state.code_search,
                Self::Diff => &state.diff_search,
            };
            mode.enabled(search)
        }));
        super::controls::button()
            .test_id(&format!("{prefix}-find-{id}"))
            .label(description)
            .tooltip(dynamic(move || super::tooltip_label(description.to_owned(), current_palette())))
            .w(px(23.0))
            .h(px(22.0))
            .flex_none()
            .rounded(px(4.0))
            .cursor(Cursor::Pointer)
            .bg(move || {
                let palette = current_palette();
                if active.get() { palette.accent_soft } else { transparent_black() }
            })
            .hover(|style| style.bg(move || current_palette().surface_hover))
            .on_press(move || app_signal().update(|state| mode.toggle(self.search_mut(state))))
            .child(text(text_label).text_size(px(11.0)).text_color(move || current_palette().text_control))
    }
}

pub(super) fn result_text(result: &EditorSearchResult) -> String {
    if result.error.is_some() {
        "正则表达式无效".to_owned()
    } else if let Some(current) = result.current {
        format!("{}/{}", current + 1, result.count)
    } else {
        format!("{} 个结果", result.count)
    }
}

pub(super) fn handle_input_key(key: &str) {
    if key == "Escape" {
        crate::state::close_find_bar();
    }
}
