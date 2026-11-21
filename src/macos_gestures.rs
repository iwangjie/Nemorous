//! App-local observer for bare double Shift and the Control+Tab switcher.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Instant;

use block2::RcBlock;
use nsh_platform_macos::objc2::{
    MainThreadMarker, ProtocolType, msg_send, rc::Retained, runtime::AnyObject,
};
use nsh_platform_macos::objc2_app_kit::{
    NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSTextInputClient,
};
use nsh_platform_macos::objc2_foundation::NSObjectProtocol;
use tokio::sync::broadcast;

use crate::ui::key_gestures::ShiftTap;

const TAB: u16 = 48;
const FORWARD: u8 = 1;
const REVERSE: u8 = 2;
const LEFT_SHIFT: u16 = 56;
const RIGHT_SHIFT: u16 = 60;
const LEFT_CONTROL: u16 = 59;
const RIGHT_CONTROL: u16 = 62;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gesture {
    DoubleShift,
    SwitcherPressed { reverse: bool },
    ControlReleased,
}

static SENDER: OnceLock<broadcast::Sender<Gesture>> = OnceLock::new();
static INSTALLED: AtomicBool = AtomicBool::new(false);
static SWITCHER_BINDINGS: AtomicU8 = AtomicU8::new(0);

pub(crate) fn configure_switcher(forward: bool, reverse: bool) {
    let forward = if forward { FORWARD } else { 0 };
    let reverse = if reverse { REVERSE } else { 0 };
    SWITCHER_BINDINGS.store(forward | reverse, Ordering::Release);
}

fn sender() -> &'static broadcast::Sender<Gesture> {
    SENDER.get_or_init(|| broadcast::channel(16).0)
}

/// UI owner task: `while let Ok(g) = rx.recv().await { ... }`.
pub(crate) fn subscribe() -> broadcast::Receiver<Gesture> {
    sender().subscribe()
}

/// True only while a live `Guard` exists (raw NSH path must stand down).
pub(crate) fn is_installed() -> bool {
    INSTALLED.load(Ordering::Acquire)
}

/// Removes the monitor on drop; keep it alive for the app's lifetime.
pub struct Guard {
    monitor: Retained<AnyObject>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY: the monitor came from addLocalMonitor…; removed once, on main thread.
        unsafe { NSEvent::removeMonitor(&self.monitor) };
        configure_switcher(false, false);
        INSTALLED.store(false, Ordering::Release);
    }
}

pub fn install() -> Option<Guard> {
    let mtm = MainThreadMarker::new()?; // NSApp / AppKit must be on the main thread
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return None;
    }
    NSApplication::sharedApplication(mtm);
    let sender = sender().clone();
    let state = RefCell::new((None::<isize>, ShiftTap::default()));
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // AppKit lends this event until the monitor returns.
        if observe(unsafe { event.as_ref() }, mtm, &state, &sender) {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });
    let mask = NSEventMask::FlagsChanged | NSEventMask::KeyDown;
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    let Some(monitor) = monitor else {
        INSTALLED.store(false, Ordering::Release);
        return None;
    };
    Some(Guard { monitor })
}

fn observe(
    event: &NSEvent,
    mtm: MainThreadMarker,
    state: &RefCell<(Option<isize>, ShiftTap)>,
    sender: &broadcast::Sender<Gesture>,
) -> bool {
    let Some(window) = event.window(mtm) else {
        return false;
    };
    let number = window.windowNumber();
    let key_window = NSApplication::sharedApplication(mtm).keyWindow();
    if !window.isVisible() || key_window.is_some_and(|key| key.windowNumber() != number) {
        return false;
    }
    let mut state = state.borrow_mut();
    let (window_number, shift) = &mut *state;
    if *window_number != Some(number) {
        *window_number = Some(number);
        *shift = ShiftTap::default();
    }
    let flags = event.modifierFlags();
    if event.r#type() == NSEventType::KeyDown {
        *shift = ShiftTap::default();
        let reverse = flags.contains(NSEventModifierFlags::Shift);
        let binding = if reverse { REVERSE } else { FORWARD };
        if event.keyCode() == TAB
            && flags.contains(NSEventModifierFlags::Control)
            && !flags.intersects(
                NSEventModifierFlags::Command
                    | NSEventModifierFlags::Option
                    | NSEventModifierFlags::Function,
            )
            && (SWITCHER_BINDINGS.load(Ordering::Acquire) & binding) != 0
        {
            let composing = window.firstResponder().is_some_and(|responder| {
                responder.conformsToProtocol(
                    <dyn NSTextInputClient>::protocol().expect("text input protocol"),
                ) && unsafe { msg_send![&*responder, hasMarkedText] }
            });
            if !composing {
                return sender.send(Gesture::SwitcherPressed { reverse }).is_ok();
            }
        }
        return false;
    }
    if matches!(event.keyCode(), LEFT_SHIFT | RIGHT_SHIFT) {
        let down = flags.contains(NSEventModifierFlags::Shift);
        if flags.intersects(
            NSEventModifierFlags::Control
                | NSEventModifierFlags::Option
                | NSEventModifierFlags::Command
                | NSEventModifierFlags::Function,
        ) {
            *shift = ShiftTap::default();
        } else if shift.push(down, Instant::now()) {
            let _ = sender.send(Gesture::DoubleShift);
        }
    } else {
        *shift = ShiftTap::default();
        if matches!(event.keyCode(), LEFT_CONTROL | RIGHT_CONTROL)
            && !flags.contains(NSEventModifierFlags::Control)
        {
            let _ = sender.send(Gesture::ControlReleased);
        }
    }
    false
}
