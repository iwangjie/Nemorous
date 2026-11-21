use std::time::{Duration, Instant};

use nsh_core::keyboard::KeyEvent;
use nsh_core::prelude::*;

use super::navigation_picker::{self, Mode};

const MAX_TAP: Duration = Duration::from_millis(350);
const MAX_BETWEEN_TAPS: Duration = Duration::from_millis(450);

#[derive(Clone, Default)]
pub(crate) struct ShiftTap {
    down: Option<Instant>,
    previous: Option<Instant>,
}

impl ShiftTap {
    pub(crate) fn push(&mut self, down: bool, now: Instant) -> bool {
        if down {
            self.down = Some(now);
            return false;
        }
        if self
            .down
            .take()
            .is_none_or(|start| now.duration_since(start) > MAX_TAP)
        {
            self.previous = None;
            return false;
        }
        if self
            .previous
            .take()
            .is_some_and(|previous| now.duration_since(previous) <= MAX_BETWEEN_TAPS)
        {
            return true;
        }
        self.previous = Some(now);
        false
    }
}

pub fn key_down(event: &KeyEvent) {
    #[cfg(target_os = "macos")]
    if crate::macos_gestures::is_installed() {
        return;
    }
    let gesture = global::<ShiftTap>();
    if matches!(event.key.as_ref(), "Shift")
        && !event.modifiers.ctrl
        && !event.modifiers.alt
        && !event.modifiers.cmd
        && !event.repeat
    {
        gesture.update(|gesture| {
            gesture.push(true, Instant::now());
        });
    } else {
        gesture.set(ShiftTap::default());
    }
}

pub fn key_up(event: &KeyEvent) {
    #[cfg(target_os = "macos")]
    if crate::macos_gestures::is_installed() {
        return;
    }
    if event.key.as_ref() == "Control" && !event.modifiers.ctrl {
        navigation_picker::release_control();
    }
    if event.key.as_ref() == "Shift" && !event.modifiers.shift {
        let mut double = false;
        global::<ShiftTap>().update(|gesture| {
            double = gesture.push(false, Instant::now());
        });
        if double {
            open_everywhere_if_bound();
        }
    }
}

fn open_everywhere_if_bound() {
    let modal = crate::state::app_signal()
        .with_untracked(|s| s.focus_zone == crate::state::FocusZone::Modal);
    if !modal
        && super::configured_keymap()
            .keys("searchEverywhere")
            .iter()
            .any(|key| key == crate::keymap::DOUBLE_SHIFT)
    {
        navigation_picker::open(Mode::Everywhere);
    }
}

#[cfg(target_os = "macos")]
pub(super) fn observe_macos() {
    use crate::macos_gestures::{self, Gesture};
    use tokio::sync::broadcast::error::RecvError;

    if !macos_gestures::is_installed() {
        return;
    }
    let keymap = super::keymap_memo();
    effect(move || {
        let keys = keymap.get();
        let bindings = keys.keys("switcher");
        let allowed = crate::state::app_signal()
            .with(|s| s.focus_zone != crate::state::FocusZone::Modal)
            || navigation_picker::is_switcher();
        macos_gestures::configure_switcher(
            allowed && bindings.iter().any(|key| key == "ctrl+tab"),
            allowed && bindings.iter().any(|key| key == "ctrl+shift+tab"),
        );
    });
    let mut gestures = macos_gestures::subscribe();
    spawn(async move {
        loop {
            match gestures.recv().await {
                Ok(Gesture::DoubleShift) => open_everywhere_if_bound(),
                Ok(Gesture::SwitcherPressed { reverse }) => {
                    if !super::modal_owns_focus() || navigation_picker::is_switcher() {
                        navigation_picker::start_or_move_switcher(reverse);
                    }
                }
                Ok(Gesture::ControlReleased) => navigation_picker::release_control(),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_two_quick_taps_and_rejects_a_held_modifier() {
        let mut gesture = ShiftTap::default();
        let start = Instant::now();
        assert!(!gesture.push(true, start));
        assert!(!gesture.push(false, start + Duration::from_millis(30)));
        assert!(!gesture.push(true, start + Duration::from_millis(150)));
        assert!(gesture.push(false, start + Duration::from_millis(180)));
        assert!(!gesture.push(false, start + Duration::from_millis(190)));
        assert!(!gesture.push(true, start + Duration::from_millis(300)));
        assert!(!gesture.push(false, start + Duration::from_millis(701)));
    }
}
