use std::{cell::Cell, rc::Rc};

use nsh_core::prelude::*;

use crate::config::Config;
use crate::state::{self, app_signal};

#[derive(Clone, Copy)]
pub(super) enum FontArea {
    Source,
    Diff,
    Tree,
    Search,
}

#[derive(Clone, Default)]
struct LiveZoom {
    source: Option<f32>,
    diff: Option<f32>,
}

impl FontArea {
    fn live_field(self, zoom: &mut LiveZoom) -> &mut Option<f32> {
        match self {
            Self::Source => &mut zoom.source,
            Self::Diff => &mut zoom.diff,
            _ => unreachable!("pinch is only available in code views"),
        }
    }

    pub(super) fn current(self) -> f32 {
        global::<LiveZoom>()
            .with(|zoom| match self {
                Self::Source => zoom.source,
                Self::Diff => zoom.diff,
                _ => None,
            })
            .unwrap_or_else(|| app_signal().with(|state| self.size(&state.cfg)))
    }

    fn field(self, config: &mut Config) -> &mut f32 {
        match self {
            Self::Source => &mut config.editor_font_size,
            Self::Diff => &mut config.diff_font_size,
            Self::Tree => &mut config.tree_font_size,
            Self::Search => &mut config.search_font_size,
        }
    }

    pub(super) fn size(self, config: &Config) -> f32 {
        match self {
            Self::Source => config.editor_font_size,
            Self::Diff => config.diff_font_size,
            Self::Tree => config.tree_font_size,
            Self::Search => config.search_font_size,
        }
    }

    pub(super) fn change(self, delta: f32) {
        app_signal().update(|state| {
            let size = self.field(&mut state.cfg);
            *size = if delta == 0.0 {
                self.size(&Config::default())
            } else {
                (*size + delta).clamp(10.0, 24.0)
            };
        });
        state::save_config_quiet();
    }

    pub(super) fn pinch(self) -> impl Fn(&PinchEvent) {
        let gesture = Rc::new(Cell::new(None::<(f32, f32)>));
        let in_flight = gesture.clone();
        let live = global::<LiveZoom>();
        cleanup(move || {
            if in_flight.get().is_some() {
                live.update(|zoom| *self.live_field(zoom) = None);
            }
        });
        move |event| match event.phase {
            PinchPhase::Start => {
                let size = app_signal().with_untracked(|state| self.size(&state.cfg));
                gesture.replace(Some((size, size)));
            }
            PinchPhase::Move if event.delta.0.is_finite() => {
                let Some((original, target)) = gesture.get() else {
                    return;
                };
                let target = (target * (1.0 + event.delta.0)).clamp(10.0, 24.0);
                gesture.replace(Some((original, target)));
                let size = (target * 2.0).round() / 2.0;
                let display = (size != original).then_some(size);
                let mut zoom = live.get_untracked();
                let field = self.live_field(&mut zoom);
                if *field != display {
                    *field = display;
                    live.set(zoom);
                }
            }
            PinchPhase::End => {
                let Some((original, target)) = gesture.replace(None) else {
                    return;
                };
                let size = (target * 2.0).round() / 2.0;
                if size != original {
                    app_signal().update(|state| *self.field(&mut state.cfg) = size);
                    state::save_config_quiet();
                }
                live.update(|zoom| *self.live_field(zoom) = None);
            }
            _ => {}
        }
    }
}
