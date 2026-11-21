use std::time::{Duration, Instant};

use nsh_core::keyboard::KeyEvent;
use nsh_core::prelude::*;

use super::navigation_picker::{self, Mode};

const MAX_TAP: Duration = Duration::from_millis(350);
const MAX_BETWEEN_TAPS: Duration = Duration::from_millis(450);

#[derive(Clone, Default)]
struct ShiftTap {
    down: Option<Instant>,
    previous: Option<Instant>,
}

impl ShiftTap {
    fn push(&mut self, down: bool, now: Instant) -> bool {
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
    if event.key.as_ref() == "Control" && !event.modifiers.ctrl {
        navigation_picker::release_control();
    }
    if event.key.as_ref() == "Shift" && !event.modifiers.shift {
        let mut double = false;
        global::<ShiftTap>().update(|gesture| {
            double = gesture.push(false, Instant::now());
        });
        if double {
            navigation_picker::open(Mode::Everywhere);
        }
    }
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
