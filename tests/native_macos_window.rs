//! 真实 macOS AppKit `NSWindow` + `AppView` + CoreText 实机同仓验收测试与真实窗口截屏。
//!
//! 必须运行在 macOS 主线程（`harness = false`），直接使用 NSH v0.6.1 `nsh-platform-macos`
//! 公开 API 启动真实 `nemorous::App`、发送真实 `NSEvent` 鼠标与键盘事件、验证：
//! 1. 同仓隔离副本（`build/migration-handoff/baseline/repo` 副本）加载；
//! 2. Agent 改盘后经 FSEvents 自动刷新进入真实 AppKit Commit 视图；
//! 3. 单击 Commit 行仅改选不打开 Diff，真实 `NSEvent` `⌘D` 在聚焦窗口成功打开双栏 Diff；
//! 4. 真实 `NSEvent` `F4` 跳源码、`⌘[` 返回工作区 Diff；
//! 5. 真实双击另一文件打开对应 Diff、`⌘W` 关闭后保留原行；
//! 6. 1440×900 与 1040×660 窄栏真实窗口像素截屏落盘供 A/B 视觉对照。

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("native_macos_window skipped on non-macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    macos::run();
}

#[cfg(target_os = "macos")]
mod macos {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use nsh_platform_macos as platform;
    use platform::objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, rc::Retained, sel};
    use platform::objc2_app_kit::{
        NSAccessibility, NSAccessibilityElement, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType,
        NSBitmapImageFileType, NSBitmapImageRepPropertyKey, NSEvent, NSEventModifierFlags,
        NSEventPhase, NSEventType, NSMenuItem, NSPanel, NSPasteboard, NSPasteboardTypeString, NSScreen, NSView, NSWindow,
        NSWindowStyleMask,
    };
    use platform::objc2_foundation::{
        NSDate, NSDictionary, NSIntersectionRect, NSNumber, NSPoint, NSProcessInfo, NSRect, NSRunLoop,
        NSSize, NSString,
    };

    struct PinchIvars {
        point: NSPoint,
        phase: NSEventPhase,
        amount: f64,
    }

    define_class!(
        #[unsafe(super(NSEvent))]
        #[thread_kind = MainThreadOnly]
        #[ivars = PinchIvars]
        struct TestPinchEvent;

        impl TestPinchEvent {
            #[unsafe(method(type))]
            fn event_type(&self) -> NSEventType {
                NSEventType::Magnify
            }

            #[unsafe(method(phase))]
            fn phase(&self) -> NSEventPhase {
                self.ivars().phase
            }

            #[unsafe(method(magnification))]
            fn magnification(&self) -> f64 {
                self.ivars().amount
            }

            #[unsafe(method(locationInWindow))]
            fn location(&self) -> NSPoint {
                self.ivars().point
            }

            #[unsafe(method(modifierFlags))]
            fn flags(&self) -> NSEventModifierFlags {
                NSEventModifierFlags::empty()
            }

            #[unsafe(method(timestamp))]
            fn timestamp(&self) -> f64 {
                NSProcessInfo::processInfo().systemUptime()
            }
        }
    );

    fn pinch(mtm: MainThreadMarker, input: &NSView, point: NSPoint) {
        for (phase, amount) in [
            (NSEventPhase::Began, 0.0),
            (NSEventPhase::Changed, 0.2),
            (NSEventPhase::Ended, 0.0),
        ] {
            let event = mtm.alloc::<TestPinchEvent>().set_ivars(PinchIvars { point, phase, amount });
            let event: Retained<TestPinchEvent> = unsafe { msg_send![super(event), init] };
            input.magnifyWithEvent(&event);
            pump(50);
        }
    }

    fn pasteboard_available() -> bool {
        unsafe {
            let cls = platform::objc2::class!(NSPasteboard);
            let pb: *mut platform::objc2::runtime::AnyObject =
                platform::objc2::msg_send![cls, generalPasteboard];
            !pb.is_null()
        }
    }

    fn pasteboard_set(text: &str) {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let kind = unsafe { NSPasteboardTypeString };
        pb.setString_forType(&NSString::from_str(text), kind);
    }

    fn pasteboard_get() -> Option<String> {
        let pb = NSPasteboard::generalPasteboard();
        let kind = unsafe { NSPasteboardTypeString };
        pb.stringForType(kind).map(|s| s.to_string())
    }

    fn probe_pasteboard_usable() -> bool {
        if !pasteboard_available() {
            return false;
        }
        pasteboard_set("__nemorous_pb_probe__");
        pasteboard_get().as_deref() == Some("__nemorous_pb_probe__")
    }

    fn find_edit_copy_menu_item(app: &NSApplication) -> Option<Retained<NSMenuItem>> {
        let main = app.mainMenu()?;
        for top in main.itemArray().iter() {
            let title = top.title().to_string();
            if title == "编辑" || title == "Edit" {
                if let Some(sub) = top.submenu() {
                    for item in sub.itemArray().iter() {
                        let t = item.title().to_string();
                        if t == "复制" || t == "Copy" {
                            return Some(item);
                        }
                    }
                }
            }
        }
        None
    }

    fn trigger_edit_copy_menu(app: &NSApplication) -> bool {
        let Some(item) = find_edit_copy_menu_item(app) else {
            return false;
        };
        let Some(action) = item.action() else {
            return false;
        };
        let target = item.target();
        let sent = unsafe {
            app.sendAction_to_from(action, target.as_deref(), Some(&*item))
        };
        pump(120);
        sent
    }

    fn a11y_contains(view: &NSView, expected: &str) -> bool {
        view_a11y_text(view).contains(expected)
            || view.subviews().iter().any(|child| a11y_contains(&child, expected))
    }

    fn git_run(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn view_a11y_text(view: &NSView) -> String {
        if let Some(lbl) = view.accessibilityLabel() {
            let s = lbl.to_string();
            if !s.is_empty() {
                return s;
            }
        }
        if let Some(val) = view.accessibilityValue() {
            if let Ok(ns) = val.downcast::<NSString>() {
                let s = ns.to_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
        if let Some(title) = view.accessibilityTitle() {
            let s = title.to_string();
            if !s.is_empty() {
                return s;
            }
        }
        for child in view.subviews().iter() {
            let t = view_a11y_text(&child);
            if !t.is_empty() {
                return t;
            }
        }
        String::new()
    }

    fn pump(ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            NSRunLoop::currentRunLoop()
                .runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        }
    }

    fn wait_until(what: &str, timeout: Duration, mut pred: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while !pred() {
            assert!(
                Instant::now() < deadline,
                "timeout waiting for: {what}"
            );
            pump(15);
        }
    }

    fn view_with_id(view: &NSView, id: &str) -> Option<Retained<NSView>> {
        if view
            .accessibilityIdentifier()
            .is_some_and(|v| v.to_string() == id)
        {
            return Some(view.retain());
        }
        view.subviews()
            .iter()
            .find_map(|child| view_with_id(&child, id))
    }

    fn dump_ids(view: &NSView, out: &mut Vec<String>) {
        if let Some(id) = view.accessibilityIdentifier() {
            let s = id.to_string();
            if !s.is_empty() {
                out.push(s);
            }
        }
        for child in view.subviews().iter() {
            dump_ids(&child, out);
        }
    }

    fn center_in_window(view: &NSView) -> NSPoint {
        let b = view.bounds();
        let local = NSPoint::new(b.origin.x + b.size.width * 0.5, b.origin.y + b.size.height * 0.5);
        view.convertPoint_toView(local, None)
    }

    fn send_click(window: &NSWindow, target: &NSView, clicks: isize) {
        send_click_at(window,center_in_window(target),clicks);
    }

    fn send_wheel(window: &NSWindow, input_surface: &NSView, point: NSPoint) {
        dispatch_wheel(window, input_surface, point);
        pump(400);
    }

    fn dispatch_wheel(window: &NSWindow, input_surface: &NSView, point: NSPoint) {
        dispatch_wheel_delta(window, input_surface, point, 0, 12);
    }

    fn dispatch_wheel_delta(window: &NSWindow, input_surface: &NSView, point: NSPoint, dx: i32, dy: i32) {
        use std::ffi::c_void;
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventCreateScrollWheelEvent2(source: *const c_void, units: u32, wheels: u32,
                wheel1: i32, wheel2: i32, wheel3: i32) -> *mut c_void;
            fn CGEventSetLocation(event: *mut c_void, location: NSPoint);
            fn CGEventSetTimestamp(event: *mut c_void, stamp: u64);
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFRelease(value: *const c_void);
        }
        // Windowless NSEvents use screen coordinates. Inject a point that the
        // input surface reads as window-relative, as NSH's scripted wheel does.
        let height = NSScreen::screens(MainThreadMarker::from(window)).firstObject()
            .expect("screen for native test").frame().size.height;
        let event: Option<Retained<NSEvent>> = unsafe {
            let cg = CGEventCreateScrollWheelEvent2(std::ptr::null(), 1, 2, -dy, -dx, 0);
            assert!(!cg.is_null());
            CGEventSetLocation(cg, NSPoint::new(point.x, height - point.y));
            CGEventSetTimestamp(cg, (NSProcessInfo::processInfo().systemUptime() * 1e9) as u64);
            let event = platform::objc2::msg_send![platform::objc2::class!(NSEvent), eventWithCGEvent: cg];
            CFRelease(cg);
            event
        };
        let event = event.expect("synthesized wheel event");
        let location = event.locationInWindow();
        assert!((location.x - point.x).abs() < 1.0 && (location.y - point.y).abs() < 1.0);
        assert!(dx == 0 || event.scrollingDeltaX().signum() == -f64::from(dx).signum());
        assert!(dy == 0 || event.scrollingDeltaY().signum() == -f64::from(dy).signum());
        input_surface.scrollWheel(&event);
    }

    fn move_pointer(window: &NSWindow, input_surface: &NSView, point: NSPoint) {
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::MouseMoved, point, NSEventModifierFlags::empty(),
            NSProcessInfo::processInfo().systemUptime(), window.windowNumber(), None, 0, 0, 0.0,
        ).unwrap();
        input_surface.mouseMoved(&event);
    }

    fn left_button_event(window: &NSWindow, pt: NSPoint, clicks: isize, down: bool) -> Retained<NSEvent> {
        let kind = if down { NSEventType::LeftMouseDown } else { NSEventType::LeftMouseUp };
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            pt,
            NSEventModifierFlags::empty(),
            NSProcessInfo::processInfo().systemUptime(),
            window.windowNumber(),
            None,
            0,
            clicks,
            if down { 1.0 } else { 0.0 },
        )
        .expect("left mouse event")
    }

    fn send_left_button(window: &NSWindow, pt: NSPoint, clicks: isize, down: bool) {
        window.sendEvent(&left_button_event(window, pt, clicks, down));
    }

    fn send_click_at(window: &NSWindow, pt: NSPoint, clicks: isize) {
        for c in 1..=clicks {
            send_left_button(window, pt, c, true);
            pump(25);
            send_left_button(window, pt, c, false);
            pump(40);
        }
        pump(120);
    }

    fn choose_context_item(window: &NSWindow, editor: &NSView, label: &'static str) {
        choose_native_menu_item(label, || {
            let bounds = editor.bounds();
            let y = if editor.isFlipped() {
                9.0
            } else {
                bounds.size.height - 9.0
            };
            let point = editor.convertPoint_toView(
                NSPoint::new(bounds.origin.x + 96.0_f64.min(bounds.size.width / 2.0), bounds.origin.y + y),
                None,
            );
            for (kind, pressure) in [(NSEventType::RightMouseDown, 1.0), (NSEventType::RightMouseUp, 0.0)] {
                let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                    kind, point, NSEventModifierFlags::empty(), NSProcessInfo::processInfo().systemUptime(),
                    window.windowNumber(), None, 0, 1, pressure,
                ).unwrap();
                window.sendEvent(&event);
                pump(60);
            }
        });
    }

    fn choose_native_menu_item(label: &'static str, open_menu: impl FnOnce()) {
        use std::ptr::NonNull;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        use platform::objc2_app_kit::{NSMenu, NSMenuDidBeginTrackingNotification};
        use platform::objc2_foundation::{NSNotification, NSNotificationCenter};

        fn choose(menu: &NSMenu, title: &NSString) -> bool {
            let index = menu.indexOfItemWithTitle(title);
            if index >= 0 {
                menu.performActionForItemAtIndex(index);
                return true;
            }
            menu.itemArray().iter().filter_map(|item| item.submenu()).any(|child| choose(&child, title))
        }

        let chosen = Arc::new(AtomicBool::new(false));
        let did_choose = chosen.clone();
        let handle_menu = block2::RcBlock::new(move |notification: NonNull<NSNotification>| {
            let Some(menu) = (unsafe { notification.as_ref() })
                .object()
                .and_then(|object| object.downcast::<NSMenu>().ok())
            else {
                return;
            };
            if choose(&menu, &NSString::from_str(label)) {
                did_choose.store(true, Ordering::Relaxed);
            }
            let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
            let escape = NSString::from_str("\u{1b}");
            let dismiss = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown, NSPoint::new(0.0, 0.0), NSEventModifierFlags::empty(),
                NSProcessInfo::processInfo().systemUptime(), 0, None, &escape, &escape, false, 53,
            ).unwrap();
            app.postEvent_atStart(&dismiss, true);
        });
        let center = NSNotificationCenter::defaultCenter();
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSMenuDidBeginTrackingNotification),
                None,
                None,
                &handle_menu,
            )
        };
        open_menu();
        unsafe { center.removeObserver((*observer).as_ref()) };
        assert!(chosen.load(Ordering::Relaxed), "missing menu item {label}");
    }

    fn make_key_event(
        window: &NSWindow,
        kind: NSEventType,
        flags: NSEventModifierFlags,
        ts: f64,
        chars: &NSString,
        ignoring: &NSString,
        code: u16,
    ) -> Retained<NSEvent> {
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            flags,
            ts,
            window.windowNumber(),
            None,
            chars,
            ignoring,
            false,
            code,
        )
        .expect("key event")
    }

    fn send_key(
        app: &NSApplication,
        window: &NSWindow,
        chars: &str,
        ignoring: &str,
        key_code: u16,
        flags: NSEventModifierFlags,
    ) {
        let c = NSString::from_str(chars);
        let ig = NSString::from_str(ignoring);
        let ts = NSProcessInfo::processInfo().systemUptime();
        let down = make_key_event(window, NSEventType::KeyDown, flags, ts, &c, &ig, key_code);
        let up = make_key_event(window, NSEventType::KeyUp, flags, ts + 0.01, &c, &ig, key_code);
        app.postEvent_atStart(&down, false);
        app.postEvent_atStart(&up, false);
        drain_native_keys(app, 220);
    }

    fn drain_native_keys(app: &NSApplication, millis: u64) {
        let deadline = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < deadline {
            if let Some(ev) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                platform::objc2_app_kit::NSEventMask::Any,
                Some(&NSDate::dateWithTimeIntervalSinceNow(0.01)),
                unsafe { platform::objc2_foundation::NSDefaultRunLoopMode },
                true,
            ) {
                app.sendEvent(&ev);
            }
        }
    }

    fn post_modifier(app: &NSApplication, window: &NSWindow, code: u16, flags: NSEventModifierFlags) {
        let empty = NSString::from_str("");
        let time = NSProcessInfo::processInfo().systemUptime();
        let event = make_key_event(window, NSEventType::FlagsChanged, flags, time, &empty, &empty, code);
        app.postEvent_atStart(&event, false);
        drain_native_keys(app, 30);
    }

    fn save_window_png(window: &NSWindow, out_path: &Path) {
        save_view_png(&window.contentView().expect("window contentView"), out_path);
    }

    fn save_view_png(view: &NSView, out_path: &Path) {
        pump(180);
        let bounds = view.bounds();
        let data = snapshot_png(view, bounds);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(out_path, &data).expect("write PNG");
        println!(
            "SCREENSHOT saved={} bytes={} bounds={}x{}",
            out_path.display(),
            data.len(),
            bounds.size.width,
            bounds.size.height
        );
    }

    fn snapshot_png(view: &NSView, bounds: NSRect) -> Vec<u8> {
        let rep = view
            .bitmapImageRepForCachingDisplayInRect(bounds)
            .expect("bitmapImageRepForCachingDisplayInRect");
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props =
            NSDictionary::<NSBitmapImageRepPropertyKey, platform::objc2::runtime::AnyObject>::new();
        unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props)
        }
        .expect("PNG encode").to_vec()
    }

    fn copy_dir_all(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let ty = entry.file_type().unwrap();
            let to = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir_all(&entry.path(), &to);
            } else {
                fs::copy(entry.path(), to).unwrap();
            }
        }
    }

    fn batch2_search_and_replace(
        app: &NSApplication,
        window: &NSWindow,
        root: &NSView,
        repo: &Path,
        evidence: &Path,
    ) {
        fs::create_dir_all(evidence).unwrap();
        let shortcut = NSEventModifierFlags::Command | NSEventModifierFlags::Shift;
        let type_work = || {
            for (ch, code) in [("w", 13), ("o", 31), ("r", 15), ("k", 40)] {
                send_key(app, window, ch, ch, code, NSEventModifierFlags::empty());
            }
        };

        send_key(app, window, "F", "f", 3, shortcut);
        wait_until("Find in Files search input", Duration::from_secs(4), || {
            view_with_id(root, "content-search-input").is_some()
        });
        send_click(window, &view_with_id(root, "content-search-input").unwrap(), 1);
        type_work();
        wait_until("workspace search result", Duration::from_secs(4), || {
            view_with_id(root, "content-search-hit-0").is_some()
        });
        save_window_png(window, &evidence.join("native-find-in-files.png"));
        send_click(window, &view_with_id(root, "content-search-pin").unwrap(), 1);
        wait_until("pinned Find result", Duration::from_secs(4), || {
            view_with_id(root, "find-results-hit-0").is_some()
        });
        send_click(window, &view_with_id(root, "find-results-hit-0").unwrap(), 2);
        wait_until("opened source caret at matching line", Duration::from_secs(4), || {
            view_with_id(root, "editor-tab-file-tracked.txt").is_some()
                && view_with_id(root, "status-cursor-pos")
                    .is_some_and(|view| view_a11y_text(&view) == "2:1")
        });
        println!("BATCH2_NATIVE_FIND_AND_PIN ok");

        send_key(app, window, "R", "r", 15, shortcut);
        wait_until("Replace in Files fields", Duration::from_secs(4), || {
            view_with_id(root, "content-search-replacement-input").is_some()
        });
        send_click(window, &view_with_id(root, "content-search-input").unwrap(), 1);
        type_work();
        let replacement = view_with_id(root, "content-search-replacement-input").unwrap();
        send_click(window, &replacement, 1);
        for (ch, code) in [("e", 14), ("d", 2), ("i", 34), ("t", 17)] {
            send_key(app, window, ch, ch, code, NSEventModifierFlags::empty());
        }
        wait_until("checked replacement available", Duration::from_secs(4), || {
            view_with_id(root, "replace-all").is_some()
        });
        send_click(window, &view_with_id(root, "replace-all").unwrap(), 1);
        wait_until("checked replacement confirmation", Duration::from_secs(4), || {
            view_with_id(root, "replace-confirm").is_some()
        });
        let tracked = repo.join("tracked.txt");
        assert_eq!(fs::read_to_string(&tracked).unwrap(), "first\nworking selection\nthird\n");
        save_window_png(window, &evidence.join("native-replace-confirmation.png"));
        send_click(window, &view_with_id(root, "replace-confirm").unwrap(), 1);
        wait_until("confirmed replacement on disk", Duration::from_secs(4), || {
            fs::read_to_string(&tracked).is_ok_and(|text| text == "first\nediting selection\nthird\n")
        });
        println!("BATCH2_NATIVE_REPLACE_CONFIRM ok");
    }

    fn batch2_project_and_keyboard(
        app: &NSApplication,
        window: &NSWindow,
        root: &NSView,
        evidence: &Path,
    ) {
        let empty = NSEventModifierFlags::empty();
        let command = NSEventModifierFlags::Command;
        let visible = |id: &str| view_with_id(root, id).is_some();
        let wait = |id: &str| wait_until(id, Duration::from_secs(4), || visible(id));
        let click = |id: &str, count| {
            wait(id);
            send_click(window, &view_with_id(root, id).unwrap(), count);
        };
        if visible("content-search-modal") {
            send_key(app, window, "\u{1b}", "\u{1b}", 53, empty);
        }
        send_key(app, window, "1", "1", 18, command);
        wait("project-workspace-root");
        click("project-file-tracked.txt", 2);
        wait("code-editor-view");
        save_window_png(window, &evidence.join("native-project-tree.png"));
        click("project-collapse-all", 1);
        assert!(!visible("project-file-tracked.txt"));
        click("project-workspace-root", 1);
        click("project-workspace-root", 1);
        click("project-file-new.txt", 2);
        wait("editor-tab-file-new.txt");
        println!("BATCH2_NATIVE_PROJECT ok");

        click("code-editor-view", 1);
        println!("BATCH2_NATIVE_FLAGS_WINDOW visible={} key={} active={}",
            window.isVisible(), window.isKeyWindow(), app.isActive());
        post_modifier(app, window, 56, NSEventModifierFlags::Shift);
        post_modifier(app, window, 56, empty);
        post_modifier(app, window, 56, NSEventModifierFlags::Shift);
        post_modifier(app, window, 56, empty);
        wait("navigation-picker-modal");
        save_window_png(window, &evidence.join("native-search-everywhere.png"));
        send_key(app, window, "\u{1b}", "\u{1b}", 53, empty);
        wait_until("Search Everywhere closed", Duration::from_secs(4), || !visible("navigation-picker-modal"));

        click("code-editor-view", 1);
        post_modifier(app, window, 59, NSEventModifierFlags::Control);
        send_key(app, window, "\t", "\t", 48, NSEventModifierFlags::Control);
        wait_until("Control+Tab opened the Switcher", Duration::from_secs(4), || visible("navigation-picker-modal"));
        println!("BATCH2_NATIVE_SWITCHER_OPEN ok");
        post_modifier(app, window, 59, empty);
        wait_until("Switcher accepts on Control release", Duration::from_secs(4), || {
            !visible("navigation-picker-modal")
                && view_with_id(root, "code-editor-view")
                    .is_some_and(|editor| a11y_contains(&editor, "editing selection"))
        });
        println!("BATCH2_NATIVE_SWITCHER_RELEASE ok");

        click("code-editor-view", 1);
        let modal_has = |title: &str| view_with_id(root, "navigation-picker-modal")
            .is_some_and(|modal| a11y_contains(&modal, title));
        let dismiss_picker = || {
            send_key(app, window, "\u{1b}", "\u{1b}", 53, empty);
            wait_until("navigation picker dismissed", Duration::from_secs(4), || !visible("navigation-picker-modal"));
            click("code-editor-view", 1);
        };
        send_key(app, window, "P", "p", 35, command | NSEventModifierFlags::Shift);
        wait_until("Command+Shift+P opened Action Search", Duration::from_secs(4), || modal_has("查找操作"));
        dismiss_picker();
        send_key(app, window, "e", "e", 14, command);
        wait_until("Command+E opened Recent Files", Duration::from_secs(4), || modal_has("最近文件"));
        dismiss_picker();
        println!("BATCH2_NATIVE_KEYBOARD double-Shift Control-release Action-Search Recent-Files ok");

        assert!(view_with_id(root, "code-editor-view").is_some_and(|editor| a11y_contains(&editor, "editing selection")));
        choose_context_item(window, &view_with_id(root, "editor-tab-file-tracked.txt").unwrap(), "向右拆分");
        wait("split-editor-left");
        wait("split-editor-right");
        click("split-tab-right-file::repo::tracked.txt", 1);
        let editors_have = |value: &str| ["split-editor-left", "split-editor-right"].into_iter()
            .all(|id| view_with_id(root, id).is_some_and(|editor| a11y_contains(&editor, value)));
        wait_until("both native panes show the same buffer", Duration::from_secs(4), || {
            editors_have("editing selection")
        });
        send_key(app, window, "z", "z", 6, empty);
        wait_until("right split editor received the real keydown", Duration::from_secs(4), || {
            view_with_id(root, "split-editor-right").is_some_and(|editor| a11y_contains(&editor, "z"))
        });
        wait_until("left native pane reflects the edit", Duration::from_secs(4), || editors_have("z"));
        save_window_png(window, &evidence.join("native-editor-split.png"));
        send_key(app, window, "z", "z", 6, command);
        let any_editor_has = |value: &str| ["split-editor-left", "split-editor-right"].into_iter()
            .any(|id| view_with_id(root, id).is_some_and(|editor| a11y_contains(&editor, value)));
        wait_until("undo reaches both panes", Duration::from_secs(4), || {
            editors_have("editing selection") && !any_editor_has("z")
        });
        click("split-code-merge", 1);
        wait("code-editor-view");
        println!("BATCH2_NATIVE_SPLIT_SHARED_UNDO ok");

        choose_context_item(window, &view_with_id(root, "code-editor-view").unwrap(), "查看文件历史");
        wait("git-bottom-pane");
        wait("git-file-history");
        wait("git-commit-file-tracked.txt");
        save_window_png(window, &evidence.join("native-file-history.png"));
        println!("BATCH2_NATIVE_FILE_HISTORY ok");

    }

    fn batch3_stage_and_branch(
        app: &NSApplication,
        window: &NSWindow,
        root: &NSView,
        repo: &Path,
        evidence: &Path,
    ) {
        let empty = NSEventModifierFlags::empty();
        let command = NSEventModifierFlags::Command;
        let row = "change-row-unstaged-batch3-ticket.txt";
        let wait = |id: &str| wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
        let click = |id: &str| {
            wait(id);
            send_click(window, &view_with_id(root, id).unwrap(), 1);
        };

        wait(row);
        println!("WORKSPACE {}", repo.display());
        println!("HEAD {}", git_run(repo, &["rev-parse", "HEAD"]));
        println!("INDEX {}", git_run(repo, &["ls-files", "--stage", "--", "batch3-ticket.txt"]));
        use sha2::Digest;
        println!("FILE_SHA256 {:x}", sha2::Sha256::digest(fs::read(repo.join("batch3-ticket.txt")).unwrap()));
        click(row);
        send_key(app, window, "s", "s", 1, empty);
        wait("change-row-staged-batch3-ticket.txt");
        let staged = git_run(repo, &["show", ":batch3-ticket.txt"]);
        assert!(staged.contains("edited first") && staged.contains("edited last"));
        click("change-row-staged-batch3-ticket.txt");
        send_key(app, window, "u", "u", 32, empty);
        wait(row);
        let staged = git_run(repo, &["show", ":batch3-ticket.txt"]);
        assert!(!staged.contains("edited first") && !staged.contains("edited last"));
        assert!(fs::read_to_string(repo.join("batch3-ticket.txt")).unwrap().contains("edited last"));
        println!("BATCH3_NATIVE_WHOLE_FILE_STAGING ok");

        click(row);
        send_key(app, window, "d", "d", 2, command);
        let editor = "working-diff-new-editor";
        wait(editor);
        assert!(view_with_id(root, "diff-stage-hunk").is_none());
        assert!(view_with_id(root, "diff-include-hunk").is_none());
        choose_context_item(window, &view_with_id(root, editor).unwrap(), "暂存此块");
        wait_until("only the selected hunk entered the real index", Duration::from_secs(6), || {
            let staged = git_run(repo, &["show", ":batch3-ticket.txt"]);
            staged.contains("edited first") && !staged.contains("edited last")
        });
        wait("change-row-staged-batch3-ticket.txt");
        assert!(fs::read_to_string(repo.join("batch3-ticket.txt")).unwrap().contains("edited last"));
        save_window_png(window, &evidence.join("native-selected-hunk.png"));
        click("change-row-staged-batch3-ticket.txt");
        send_key(app, window, "d", "d", 2, command);
        wait_until("staged Diff is open", Duration::from_secs(6), || {
            view_with_id(root, "diff-old-pane-bar").is_some_and(|view| a11y_contains(&view, "HEAD"))
        });
        wait(editor);
        choose_context_item(window, &view_with_id(root, editor).unwrap(), "取消暂存此块");
        wait_until("selected staged hunk was reverted from the index", Duration::from_secs(6), || {
            !git_run(repo, &["show", ":batch3-ticket.txt"]).contains("edited first")
        });
        wait_until("unstaging refreshed Commit", Duration::from_secs(6), || {
            view_with_id(root, "change-row-staged-batch3-ticket.txt").is_none()
        });
        println!("BATCH3_NATIVE_STAGE_AND_UNSTAGE ok");

        click(row);
        send_key(app, window, "d", "d", 2, command);
        click("diff-index-review");
        wait("index-review-middle");
        wait("index-review-head");
        wait("index-review-worktree");
        click("index-review-middle");
        send_key(app, window, "a", "a", 0, command);
        send_key(app, window, "x", "x", 7, empty);
        send_key(app, window, "s", "s", 1, command);
        wait_until("index editor wrote only selected stage-0 entry", Duration::from_secs(6), || {
            git_run(repo, &["show", ":batch3-ticket.txt"]) == "x"
        });
        assert!(fs::read_to_string(repo.join("batch3-ticket.txt")).unwrap().contains("edited last"));
        save_window_png(window, &evidence.join("native-index-three-pane.png"));
        wait_until("index save completion reached AppKit", Duration::from_secs(6), || {
            a11y_contains(root, "已保存暂存版本")
        });
        click("index-close");
        wait_until("index overlay closed", Duration::from_secs(6), || {
            view_with_id(root, "git-index-review").is_none()
        });
        println!("BATCH3_NATIVE_INDEX_EDIT ok");
        git_run(repo, &["restore", "--staged", "--", "batch3-ticket.txt"]);
        git_run(repo, &["restore", "--", "batch3-ticket.txt", "tracked.txt"]);

        send_key(app, window, "b", "b", 11, command);
        wait("branch-filter-input");
        send_key(app, window, "t", "t", 17, empty);
        wait("branch-switch-repo-topic");
        save_window_png(window, &evidence.join("native-branch-matrix.png"));
        click("branch-switch-repo-topic");
        wait_until("branch selected through AppKit", Duration::from_secs(6), || {
            git_run(repo, &["symbolic-ref", "--short", "HEAD"]) == "topic"
        });
        println!("BATCH3_NATIVE_BRANCH_MATRIX ok");
    }

    fn batch4_resolver(
        app: &NSApplication,
        window: &NSWindow,
        root: &NSView,
        repo: &Path,
        evidence: &Path,
    ) {
        let command = NSEventModifierFlags::Command;
        let empty = NSEventModifierFlags::empty();
        let wait = |id: &str| wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
        let click = |id: &str| {
            wait(id);
            send_click(window, &view_with_id(root, id).unwrap(), 1);
        };
        println!("BATCH4_NATIVE_FIXTURE {}", repo.display());
        println!("BATCH4_NATIVE_HEAD {}", git_run(repo, &["rev-parse", "HEAD"]));
        println!("BATCH4_NATIVE_STAGES\n{}", git_run(repo, &["ls-files", "--stage", "--", "conflict.txt"]));
        click("change-row-unstaged-conflict.txt");
        send_key(app, window, "d", "d", 2, command);
        click("diff-open-conflicts");
        wait("conflict-result");
        wait("conflict-ours");
        wait("conflict-theirs");
        wait("conflict-take-theirs");
        send_key(app, window, "™", "2", 19, NSEventModifierFlags::Option);
        wait_until("conflict draft selected the source block", Duration::from_secs(6), || {
            view_with_id(root, "conflict-result").is_some_and(|view| a11y_contains(&view, "source line"))
        });
        save_window_png(window, &evidence.join("native-conflict-draft.png"));
        send_key(app, window, "\u{1b}", "\u{1b}", 53, empty);
        wait_until("conflict workbench closed", Duration::from_secs(6), || {
            view_with_id(root, "git-conflict-workbench").is_none()
        });
        assert!(!git_run(repo, &["ls-files", "-u", "--", "conflict.txt"]).is_empty());
        click("diff-open-conflicts");
        wait("conflict-result");
        wait_until("reopened conflict draft preserved", Duration::from_secs(6), || {
            view_with_id(root, "conflict-result").is_some_and(|view| a11y_contains(&view, "source line"))
        });
        send_key(app, window, "s", "s", 1, command);
        wait_until("real Git conflict stages were replaced", Duration::from_secs(6), || {
            git_run(repo, &["ls-files", "-u", "--", "conflict.txt"]).is_empty()
        });
        assert_eq!(git_run(repo, &["show", ":conflict.txt"]), "source line");
        wait("conflict-continue");
        send_key(app, window, "\r", "\r", 36, command);
        wait_until("merge commit completed in the primary Git fixture", Duration::from_secs(6), || {
            git_run(repo, &["show", "-s", "--format=%P", "HEAD"]).split_whitespace().count() == 2
        });
        println!("BATCH4_NATIVE_BLOB {}", git_run(repo, &["rev-parse", "HEAD:conflict.txt"]));
        println!("BATCH4_NATIVE_PARENTS {}", git_run(repo, &["show", "-s", "--format=%P", "HEAD"]));
        println!("NATIVE_BATCH4_CONFLICT PASS");
    }

    fn batch5_workbench(app: &NSApplication, window: &NSWindow, root: &NSView, repo: &Path, evidence: &Path) {
        let click = |id: &str| {
            wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
            send_click(window, &view_with_id(root, id).unwrap(), 1);
        };
        if std::env::var_os("NEMOROUS_NATIVE_DESKTOP_PATH").is_some() {
            click("rail-run-btn");
            click("run-start");
            wait_until("Homebrew tool started from a desktop PATH", Duration::from_secs(6), || {
                view_with_id(root, "workbench-run-output").is_some_and(|view| a11y_contains(&view, "ripgrep "))
            });
            wait_until("Homebrew tool exited successfully", Duration::from_secs(6), || a11y_contains(root, "退出码 0"));
            save_window_png(window, &evidence.join("native-desktop-path.png"));
            println!("NATIVE_DESKTOP_PATH PASS");
            return;
        }
        click("rail-terminal-btn");
        click("terminal-screen");
        let command = "echo PTY > terminal-proof";
        for c in command.chars().chain(std::iter::once('\r')) {
            let text = NSString::from_str(&c.to_string());
            let timestamp = NSProcessInfo::processInfo().systemUptime();
            let keycode = if c == '\r' { 36 } else { 0 };
            let event = make_key_event(window, NSEventType::KeyDown, NSEventModifierFlags::empty(), timestamp, &text, &text, keycode);
            app.postEvent_atStart(&event, false);
        }
        drain_native_keys(app, 650);
        wait_until("PTY command wrote the expected bytes in the workspace", Duration::from_secs(6), || {
            fs::read(repo.join("terminal-proof")).is_ok_and(|bytes| bytes == b"PTY\n")
        });
        click("workbench-hide");
        click("rail-terminal-btn");
        click("terminal-screen");
        send_key(app, window, "p", "p", 35, NSEventModifierFlags::empty());
        click("terminal-stop");
        click("rail-run-btn");
        click("run-start");
        wait_until("native Run stdout", Duration::from_secs(6), || {
            view_with_id(root, "workbench-run-output").is_some_and(|view| a11y_contains(&view, "NATIVE_RUN_OUTPUT"))
        });
        wait_until("native Run exit", Duration::from_secs(6), || a11y_contains(root, "退出码 7"));
        save_window_png(window, &evidence.join("native-run-output.png"));
        println!("BATCH5_NATIVE_FIXTURE {}", repo.display());
        println!("BATCH5_NATIVE_HEAD {}", git_run(repo, &["rev-parse", "HEAD"]));
        println!("NATIVE_BATCH5_TERMINAL_RUN PASS");
    }

    fn batch6_settings(app: &NSApplication, window: &NSWindow, root: &NSView, repo: &Path, evidence: &Path) {
        let click = |id: &str| {
            wait_until(id, Duration::from_secs(6), || view_with_id(root,id).is_some());
            send_click(window,&view_with_id(root,id).unwrap(),1);
        };
        send_key(app,window,",",",",43,NSEventModifierFlags::Command);
        wait_until("Settings shortcut opened appearance",Duration::from_secs(6),|| view_with_id(root,"settings-modal").is_some());
        click("zoom-increase");
        wait_until("110 percent zoom applied",Duration::from_secs(6),|| a11y_contains(root,"110%"));
        let reset = view_with_id(root,"zoom-reset").unwrap();
        let logical = center_in_window(&reset);
        let top_left = root.convertPoint_toView(
            NSPoint::new(0.0, if root.isFlipped() { 0.0 } else { root.bounds().size.height }),
            None
        );
        let visible = NSPoint::new(top_left.x + (logical.x-top_left.x)*1.1,top_left.y + (logical.y-top_left.y)*1.1);
        send_click_at(window,visible,1);
        wait_until("zoom reset",Duration::from_secs(6),|| a11y_contains(root,"100%"));
        click("settings-updates");
        wait_until("native update settings rendered",Duration::from_secs(6),|| view_with_id(root,"update-check").is_some());
        save_window_png(window,&evidence.join("native-settings-update.png"));
        click("settings-appearance");
        save_window_png(window,&evidence.join("native-settings-appearance.png"));
        click("settings-keymap");
        wait_until("opened keyboard mapping from settings",Duration::from_secs(6),|| view_with_id(root,"keymap-settings").is_some());
        send_key(app,window,"\u{1b}","\u{1b}",53,NSEventModifierFlags::empty());
        println!("BATCH6_NATIVE_FIXTURE {}",repo.display());
        println!("BATCH6_NATIVE_HEAD {}",git_run(repo,&["rev-parse","HEAD"]));
        println!("NATIVE_BATCH6_SETTINGS PASS");
    }

    fn review_cleanup(app: &NSApplication, window: &NSWindow, root: &NSView, evidence: &Path) {
        let click = |id: &str| {
            wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
            send_click(window, &view_with_id(root, id).unwrap(), 1);
        };
        click("change-row-unstaged-tracked.txt");
        send_key(app, window, "d", "d", 2, NSEventModifierFlags::Command);
        wait_until("working diff opened", Duration::from_secs(6), || {
            view_with_id(root, "working-diff-new-editor").is_some()
        });
        click("diff-next-hunk");
        assert!(!a11y_contains(root, "再按 F7"));
        assert!(!a11y_contains(root, "⌘[ 返回"));
        save_window_png(window, &evidence.join("diff.png"));

        click("diff-jump-source-btn");
        wait_until("source opened", Duration::from_secs(6), || {
            view_with_id(root, "code-editor-view").is_some()
        });
        assert!(view_with_id(root, "code-return-diff-btn").is_none());
        assert!(view_with_id(root, "code-language-tools").is_none());
        choose_context_item(window, &view_with_id(root, "code-editor-view").unwrap(), "查看文件历史");
        wait_until("file history opened", Duration::from_secs(6), || {
            view_with_id(root, "git-file-history").is_some()
                && view_with_id(root, "git-file-view-options").is_some()
        });
        assert!(!a11y_contains(root, "已加载 1 个提交"));
        save_window_png(window, &evidence.join("source-and-history.png"));

        click("editor-tab-working-diff");
        wait_until("diff tab restored", Duration::from_secs(6), || {
            view_with_id(root, "working-diff-new-editor").is_some()
        });
        let narrow = NSRect::new(NSPoint::new(80.0, 80.0), NSSize::new(1040.0, 660.0));
        window.setFrame_display(window.frameRectForContentRect(narrow), true);
        save_window_png(window, &evidence.join("diff-narrow.png"));
        println!("NATIVE_REVIEW_CLEANUP PASS");
    }

    fn left_tool_windows(app: &NSApplication, window: &NSWindow, root: &NSView, evidence: &Path, width: f32) {
        let wait = |id: &str| wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
        let input_surface = root.subviews().firstObject().expect("AppKit input surface");
        let click = |id: &str| {
            wait(id);
            send_click(window, &view_with_id(root, id).unwrap(), 1);
        };
        let hover = |id: &str| {
            wait(id);
            let target = view_with_id(root, id).unwrap();
            move_pointer(window, &input_surface, center_in_window(&target));
            pump(120);
        };
        verify_left_tool_windows(window, root, evidence, width, click, hover, |label, code| {
            send_key(app, window, label, label, code, NSEventModifierFlags::Command);
        });
    }

    fn write_left_tool_files(repo: &Path) {
        for (name, text) in [
            ("src/nested/tracked.txt", "first\nbase\nlast\n"),
            ("src/nested/deeper/leaf.txt", "leaf\n"),
            ("docs/nested/guide.md", "guide\n"),
        ] {
            let path = repo.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    fn verify_left_tool_windows(
        window: &NSWindow,
        root: &NSView,
        evidence: &Path,
        width: f32,
        click: impl Fn(&str),
        hover: impl Fn(&str),
        command: impl Fn(&str, u16),
    ) {
        let wait = |id: &str| wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
        wait("changes-header-title-row");
        wait("change-row-unstaged-src/nested/tracked.txt");
        assert!((view_with_id(root, "commit-pane").unwrap().bounds().size.width - f64::from(width)).abs() < 2.0);
        if width < 150.0 {
            wait("changes-toolbar-more");
            hover("changes-tree-scroll");
            wait("changes-hide");
            save_window_png(window, &evidence.join("commit-narrow.png"));
            command("1", 18);
            wait("project-options-button");
            wait("project-hide");
            wait("project-workspace-root");
            assert!((view_with_id(root, "project-pane").unwrap().bounds().size.width - f64::from(width)).abs() < 2.0);
            save_window_png(window, &evidence.join("project-narrow.png"));
        } else {
            click("change-row-unstaged-src/nested/tracked.txt");
            command("d", 2);
            wait_until("tracked.txt working Diff", Duration::from_secs(6), || {
                view_with_id(root, "working-diff-new-editor")
                    .is_some_and(|editor| a11y_contains(&editor, "working edit"))
            });
            hover("working-diff-new-editor");
            assert!(view_with_id(root, "changes-title-actions").is_none());
            let tree = view_with_id(root, "changes-tree-scroll").unwrap().frame();
            hover("changes-tree-scroll");
            wait("changes-title-actions");
            assert_eq!(view_with_id(root, "changes-tree-scroll").unwrap().frame(), tree);
            save_window_png(window, &evidence.join("commit-350.png"));
            hover("working-diff-new-editor");
            assert!(view_with_id(root, "changes-title-actions").is_none());
            wait("changes-refresh-btn");
            wait("changes-rollback-btn");
            command("0", 29);
            wait_until("Commit title actions after Command-0", Duration::from_secs(6), || {
                view_with_id(root, "changes-title-actions").is_some()
            });
            let diff_width = view_with_id(root, "working-diff-new-editor").unwrap().bounds().size.width;
            click("changes-hide");
            pump(100);
            save_window_png(window, &evidence.join("commit-after-hide.png"));
            wait_until("Commit tool window hidden", Duration::from_secs(6), || {
                view_with_id(root, "working-diff-new-editor")
                    .is_some_and(|editor| editor.bounds().size.width > diff_width + f64::from(width) * 0.25)
            });
            command("0", 29);
            wait_until("Commit title actions after reopening", Duration::from_secs(6), || {
                view_with_id(root, "changes-title-actions").is_some()
            });

            click("diff-jump-source-btn");
            wait("code-editor-view");
            command("1", 18);
            click("editor-tab-file-src/nested/tracked.txt");
            hover("code-editor-view");
            assert!(view_with_id(root, "project-title-actions").is_none());
            hover("project-tree-scroll");
            wait("project-title-actions");
            click("project-locate-current");
            wait("project-file-src/nested/tracked.txt");
            click("project-dir-src");
            click("project-expand-selected");
            wait("project-file-src/nested/deeper/leaf.txt");
            assert!(view_with_id(root, "project-file-docs/nested/guide.md").is_none());
            click("project-collapse-all");
            wait_until("Project tree collapsed", Duration::from_secs(6), || {
                view_with_id(root, "project-file-src/nested/tracked.txt").is_none()
            });
            click("project-locate-current");
            wait("project-file-src/nested/tracked.txt");
            save_window_png(window, &evidence.join("project-350.png"));
            let code_width = view_with_id(root, "code-editor-view").unwrap().bounds().size.width;
            click("project-hide");
            wait_until("Project tool window hidden", Duration::from_secs(6), || {
                view_with_id(root, "code-editor-view")
                    .is_some_and(|editor| editor.bounds().size.width > code_width + f64::from(width) * 0.5)
            });
            click("rail-project-btn");
            wait("project-file-src/nested/tracked.txt");
        }
        println!("NATIVE_LEFT_TOOL_WINDOWS PASS width={width}");
    }

    fn darcula_review(
        app: &NSApplication,
        window: &NSWindow,
        root: &NSView,
        config: &Path,
        evidence: &Path,
    ) {
        let wait = |id: &str| {
            wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
        };
        let click = |id: &str| {
            wait(id);
            send_click(window, &view_with_id(root, id).unwrap(), 1);
        };
        let command = NSEventModifierFlags::Command;
        click("change-row-unstaged-src/RetryPolicy.java");
        send_key(app, window, "d", "d", 2, command);
        wait("working-diff-new-editor");
        click("diff-jump-source-btn");
        wait("code-editor-view");
        send_key(app, window, "1", "1", 18, command);
        click("project-locate-current");
        wait("project-file-src/RetryPolicy.java");

        let check_theme = |name: &str| {
            wait_until(name, Duration::from_secs(4), || {
                nemorous::config::load_from(config).is_ok_and(|cfg| cfg.theme == name)
            });
        };
        for name in ["system", "light"] {
            click("rail-theme-btn");
            check_theme(name);
        }
        save_window_png(window, &config.with_file_name("light-switch.png"));
        for name in ["dark", "darcula"] {
            click("rail-theme-btn");
            check_theme(name);
        }
        save_window_png(window, &evidence.join("source.png"));

        click("editor-tab-working-diff");
        wait("working-diff-old-editor");
        wait("working-diff-new-editor");
        save_window_png(window, &evidence.join("diff-split.png"));
        click("diff-layout-toggle");
        wait("working-diff-unified-editor");
        save_window_png(window, &evidence.join("diff-single.png"));
        if std::env::var_os("NEMOROUS_NATIVE_DIFF_INLINE").is_some() {
            for (label, mode) in [("按词", "word"), ("按字符", "char"), ("关闭", "none")] {
                choose_native_menu_item(label, || click("diff-options-btn"));
                wait_until(mode, Duration::from_secs(4), || {
                    nemorous::config::load_from(config).is_ok_and(|cfg| cfg.diff_inline_mode == mode)
                });
                save_view_png(&view_with_id(root, "working-diff-unified-editor").unwrap(),
                    &evidence.join(format!("diff-inline-{mode}.png")));
            }
        }
        println!("NATIVE_DARCULA PASS");
    }

    const LONG_CHANGE_PATH: &str =
        "assets/configuration/development/integration/review-window/long-commit-message-example.yaml";

    fn commit_panel_review(
        app: &NSApplication,
        window: &NSWindow,
        root: &NSView,
        config: &Path,
        evidence: &Path,
    ) {
        let row_id = format!("change-row-unstaged-{LONG_CHANGE_PATH}");
        wait_until(&row_id, Duration::from_secs(6), || view_with_id(root, &row_id).is_some());
        let row = view_with_id(root, &row_id).unwrap();
        let list = view_with_id(root, "changes-tree-scroll").unwrap();
        let rows = view_with_id(root, "changes-tree-rows").unwrap();
        let editor = view_with_id(root, "git-commit-message").unwrap();
        let pane = view_with_id(root, "commit-pane").unwrap();
        assert!(row.bounds().size.width > list.bounds().size.width + 50.0);
        assert!(rows.bounds().size.width > list.bounds().size.width + 50.0);
        save_view_png(&pane, &evidence.join("commit-empty.png"));

        send_click(window, &editor, 1);
        let plain = NSEventModifierFlags::empty();
        for (c, code) in [("f", 3), ("i", 34), ("x", 7), ("\r", 36), ("d", 2), ("i", 34), ("f", 3), ("f", 3)] {
            send_key(app, window, c, c, code, plain);
        }
        save_view_png(&pane, &evidence.join("commit-filled.png"));

        for theme in ["system", "light"] {
            send_click(window, &view_with_id(root, "rail-theme-btn").unwrap(), 1);
            wait_until(theme, Duration::from_secs(4), || {
                nemorous::config::load_from(config).is_ok_and(|cfg| cfg.theme == theme)
            });
        }
        save_window_png(window, &evidence.join("commit-light.png"));
        println!("NATIVE_COMMIT_PANEL PASS layout, multiline input, theme switching");
    }

    fn git_log_fixture(repo: &Path) {
        fs::create_dir_all(repo).unwrap();
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.name", "Nemorous QA"],
            &["config", "user.email", "native@example.invalid"],
            &["config", "commit.gpgsign", "false"],
            &["config", "gc.auto", "0"],
        ] {
            git_run(repo, args);
        }
        for directory in ["config/development", "config/production"] {
            let path = repo.join(directory);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("application.yml"), "review: off\n").unwrap();
        }
        let review_lines = |word| (1..=140)
            .map(|n| format!("{word} {n:03} {}\n", "review".repeat(19)))
            .collect::<String>();
        let review_file = repo.join("review-lines.txt");
        fs::write(&review_file, review_lines("baseline")).unwrap();
        git_run(repo, &["add", "config", "review-lines.txt"]);
        git_run(repo, &["commit", "-m", "Set up review configuration"]);
        for index in 1..=96 {
            git_run(repo, &["commit", "--quiet", "--allow-empty", "-m", &format!("Checkpoint {index:02}")]);
        }
        git_run(repo, &["switch", "-c", "agent/review"]);
        for directory in ["config/development", "config/production"] {
            fs::write(repo.join(directory).join("application.yml"), "review: true\n").unwrap();
        }
        git_run(repo, &["commit", "-am", "Enable review settings for both environments"]);
        git_run(repo, &["tag", "review-ready"]);
        git_run(repo, &["switch", "main"]);
        fs::write(repo.join("README.md"), "# Review fixture\n").unwrap();
        git_run(repo, &["add", "README.md"]);
        git_run(repo, &["commit", "-m", "Describe the review workflow"]);
        git_run(repo, &["merge", "--no-ff", "-m", "Merge agent review", "agent/review"]);
        fs::write(repo.join("README.md"), "# Review fixture\nLocal notes\n").unwrap();
        fs::write(review_file, review_lines("working")).unwrap();
    }

    fn resident_bytes() -> u64 {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTASKINFO, 0, info.as_mut_ptr().cast(), size)
        };
        assert_eq!(read, size);
        unsafe { info.assume_init() }.pti_resident_size
    }

    fn setup_scope_read_gate(gate: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let bin = gate.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let wrapper = bin.join("git");
        fs::write(&wrapper, r#"#!/bin/sh
case " $* " in
  *" ls-tree "*" review-lines.txt ")
    gate="$NEMOROUS_NATIVE_SCOPE_GATE"
    if [ -f "$gate/hold" ] && mkdir "$gate/reading" 2>/dev/null; then
      count=0
      while [ -f "$gate/hold" ] && [ "$count" -lt 230 ]; do
        sleep 0.02
        count=$((count + 1))
      done
    fi
    ;;
esac
exec /usr/bin/git "$@"
"#).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path))).unwrap();
        // Set up the subprocess fixture before starting the app and its workers.
        unsafe {
            std::env::set_var("PATH", paths);
            std::env::set_var("NEMOROUS_NATIVE_SCOPE_GATE", gate);
        }
    }

    fn git_log_review(app: &NSApplication, window: &NSWindow, root: &NSView, repo: &Path, evidence: &Path, scope_gate: &Path) {
        let wait = |id: &str| {
            wait_until(id, Duration::from_secs(6), || view_with_id(root, id).is_some());
            view_with_id(root, id).unwrap()
        };
        let command = NSEventModifierFlags::Command;
        wait("change-row-unstaged-README.md");
        pump(500);
        let before = resident_bytes();
        send_key(app, window, "9", "9", 25, command);
        let pane = wait("git-bottom-pane");
        wait("git-log-view-options");
        wait("git-file-view-options");
        wait("git-commit-file-config/development/application.yml");
        wait("git-commit-file-config/production/application.yml");
        git_run(repo, &["tag", "native-external-refresh.tmp"]);
        wait_until("external tag in Git log with unchanged HEAD", Duration::from_secs(6), || {
            a11y_contains(&pane, "tag: native-external-refresh.tmp")
        });
        git_run(repo, &["commit", "--allow-empty", "-m", "External revision for automatic Git log"]);
        wait_until("external commit appears in open Git log", Duration::from_secs(6), || {
            a11y_contains(&pane, "External revision for automatic Git log")
        });
        wait("git-commit-file-config/production/application.yml");
        println!("NATIVE_GIT_LOG_EXTERNAL_REFS_REFRESH PASS");
        let choose_view_option = |id, label| {
            choose_native_menu_item(label, || send_click(window, &wait(id), 1));
        };
        let tag_is_visible = || a11y_contains(&pane, "tag: native-external-refresh.tmp");
        choose_view_option("git-log-view-options", "显示分支和标签");
        wait_until("Git log refs hidden", Duration::from_secs(6), || !tag_is_visible());
        choose_view_option("git-log-view-options", "显示分支和标签");
        wait_until("Git log refs restored", Duration::from_secs(6), tag_is_visible);
        choose_view_option("git-file-view-options", "按目录分组");
        wait("git-file-directory-config/development");
        wait("git-file-directory-config/production");
        choose_view_option("git-file-view-options", "按目录分组");
        wait_until("Git log files displayed without directory groups", Duration::from_secs(6), || {
            view_with_id(root, "git-file-directory-config/development").is_none()
        });
        wait("git-commit-details");
        choose_view_option("git-file-view-options", "显示提交详情");
        wait_until("Git commit details hidden", Duration::from_secs(6), || {
            view_with_id(root, "git-commit-details").is_none()
        });
        wait("git-commit-file-config/development/application.yml");
        wait("git-commit-file-config/production/application.yml");
        choose_view_option("git-file-view-options", "显示提交详情");
        wait("git-commit-details");
        println!("NATIVE_GIT_LOG_VIEW_OPTIONS PASS");
        assert!(view_with_id(root, "git-commit-file-README.md").is_none());
        pump(500);
        let after = resident_bytes();
        save_view_png(&pane, &evidence.join("git-log.png"));

        send_click(window, &wait("git-commit-file-config/development/application.yml"), 1);
        assert!(view_with_id(root, "commit-diff-pane").is_none());
        send_key(app, window, "d", "d", 2, command);
        wait("commit-diff-pane");
        pump(300);
        let with_diff = resident_bytes();
        save_window_png(window, &evidence.join("git-log-diff.png"));
        println!("NATIVE_GIT_LOG PASS rss_bytes closed={before} opened={after} with_diff={with_diff}");

        send_click(window, &wait("diff-next-file"), 1);
        wait_until("next committed file", Duration::from_secs(6), || {
            view_with_id(root, "commit-diff-pane")
                .is_some_and(|pane| a11y_contains(&pane, "config/production/application.yml"))
        });
        send_click(window, &wait("diff-prev-file"), 1);
        wait_until("previous committed file", Duration::from_secs(6), || {
            view_with_id(root, "commit-diff-pane")
                .is_some_and(|pane| a11y_contains(&pane, "config/development/application.yml"))
        });
        println!("NATIVE_COMMIT_DIFF_FILE_NAVIGATION PASS");

        for _ in 0..6 {
            send_key(app, window, "[", "[", 33, command);
            wait_until("Back reopened the previous commit Diff", Duration::from_secs(6), || {
                view_with_id(root, "commit-diff-pane")
                    .is_some_and(|pane| a11y_contains(&pane, "config/production/application.yml"))
            });
            send_key(app, window, "]", "]", 30, command);
            wait_until("Forward reopened the original commit Diff", Duration::from_secs(6), || {
                view_with_id(root, "commit-diff-pane")
                    .is_some_and(|pane| a11y_contains(&pane, "config/development/application.yml"))
            });
        }
        println!("NATIVE_DIFF_HISTORY_NAVIGATION PASS");

        send_key(app, window, "O", "O", 31, command | NSEventModifierFlags::Shift);
        wait("review-picker-input");
        send_key(app, window, "[", "[", 33, command);
        wait("review-picker-input");
        assert!(a11y_contains(&wait("commit-diff-pane"), "config/development/application.yml"));
        send_key(app, window, "\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
        wait_until("file picker closed", Duration::from_secs(6), || {
            view_with_id(root, "review-picker-input").is_none()
        });
        println!("NATIVE_FILE_PICKER_SHORTCUT_SCOPE PASS");

        let revision_row = wait("git-commit-file-config/development/application.yml");
        choose_context_item(window, &revision_row, "打开仓库版本");
        wait_until("repository version opened read-only", Duration::from_secs(6), || {
            view_with_id(root, "code-editor-view").is_some_and(|editor| a11y_contains(&editor, "review: true"))
                && a11y_contains(root, "为只读视图")
        });
        send_key(app, window, "[", "[", 33, command);
        wait("commit-diff-pane");
        send_key(app, window, "]", "]", 30, command);
        wait_until("Forward reopened the same repository version", Duration::from_secs(6), || {
            view_with_id(root, "code-editor-view").is_some_and(|editor| a11y_contains(&editor, "review: true"))
                && a11y_contains(root, "为只读视图")
        });
        println!("NATIVE_REPOSITORY_REVISION_HISTORY PASS");

        send_key(app, window, "9", "9", 25, command);
        send_click(window, &wait("change-row-unstaged-review-lines.txt"), 1);
        send_key(app, window, "d", "d", 2, command);
        let old = wait("working-diff-old-editor");
        let new = wait("working-diff-new-editor");
        choose_view_option("diff-options-btn", "同步滚动");
        let surface = root.subviews().firstObject().expect("AppKit input surface");
        let at = center_in_window(&old);
        let move_pointer = |at| move_pointer(window, &surface, at);
        let mut top = old.bounds();
        if !old.isFlipped() {
            top.origin.y += top.size.height - 80.0;
        }
        top.size.height = 80.0;
        top.size.width -= 22.0;
        send_click(window, &wait("diff-old-pane-bar"), 1);
        let still_at_top = snapshot_png(&old, top);
        move_pointer(at);
        send_key(app, window, "", "", 121, NSEventModifierFlags::empty());
        wait_until("Page Down moved the Diff viewport", Duration::from_secs(4), || {
            snapshot_png(&old, top) != still_at_top
        });
        pump(260);
        assert!(view_with_id(root, "diff-blame-tooltip").is_none(), "Page Down left a waiting tooltip");
        send_key(app, window, "", "", 126, command);
        println!("NATIVE_DIFF_ATTRIBUTION_PENDING_SCROLL PASS");

        move_pointer(NSPoint::new(at.x + 8.0, at.y));
        let tooltip = wait("diff-blame-tooltip");
        assert!(a11y_contains(&tooltip, "Nemorous QA"));
        save_window_png(window, &evidence.join("diff-attribution.png"));
        let before = snapshot_png(&old, top);
        let mut new_top = new.bounds();
        if !new.isFlipped() {
            new_top.origin.y += new_top.size.height - 80.0;
        }
        new_top.size.height = 80.0;
        new_top.size.width -= 22.0;
        let new_before = snapshot_png(&new, new_top);
        send_wheel(window, &surface, at);
        assert!(view_with_id(root, "diff-blame-tooltip").is_none(), "attribution remained after scrolling");
        let scrolled = snapshot_png(&old, top);
        assert!(before != scrolled, "the wheel did not move the diff viewport");
        assert_ne!(snapshot_png(&new, new_top), new_before, "the companion Diff did not scroll");
        println!("NATIVE_DIFF_SYNCHRONIZED_SCROLL PASS");
        save_window_png(window, &evidence.join("diff-attribution-after-scroll.png"));
        move_pointer(NSPoint::new(at.x + 12.0, at.y));
        assert!(a11y_contains(&wait("diff-blame-tooltip"), "Nemorous QA"));
        println!("NATIVE_DIFF_ATTRIBUTION PASS");
        choose_view_option("diff-options-btn", "同步滚动");

        send_click(window, &wait("diff-jump-source-btn"), 1);
        let source = wait("code-editor-view");
        choose_context_item(window, &source, "显示整文件 Git Blame");
        wait("blame-line-1");
        let last_visible_blame_line = || {
            let mut ids = Vec::new();
            dump_ids(root, &mut ids);
            ids.iter().filter_map(|id| id.strip_prefix("blame-line-")?.parse::<usize>().ok()).max()
        };
        let before_wheel = last_visible_blame_line().unwrap();
        send_wheel(window, &surface, center_in_window(&source));
        wait_until("scrolled-out source attribution removed", Duration::from_secs(4), || {
            view_with_id(root, "blame-line-1").is_none()
        });
        wait_until("new source attribution entered the viewport", Duration::from_secs(4), || {
            last_visible_blame_line().filter(|line| *line > before_wheel).is_some_and(|line| {
                view_with_id(root, &format!("blame-line-{line}"))
                    .is_some_and(|entry| a11y_contains(&entry, "未提交的本地修改"))
            })
        });
        save_window_png(window, &evidence.join("source-attribution-after-scroll.png"));
        println!("NATIVE_SOURCE_ATTRIBUTION PASS");

        let diff_tab = wait("editor-tab-working-diff");
        move_pointer(center_in_window(&diff_tab));
        send_click(window, &diff_tab, 1);
        let restored = wait("working-diff-old-editor");
        wait_until("Diff viewport restored after leaving for source", Duration::from_secs(4), || {
            snapshot_png(&restored, top) == scrolled
        });
        println!("NATIVE_DIFF_VIEWPORT_RESTORED PASS");

        send_click(window, &wait("diff-jump-source-btn"), 1);
        let source = wait("code-editor-view");
        let original = repo.join("review-lines.txt");
        let moved = repo.join("review-lines-moved.txt");
        let content = fs::read_to_string(&original).unwrap();
        let first_line = content.lines().next().unwrap();
        fs::rename(&original, &moved).unwrap();
        let notice = wait("code-disk-state");
        assert!(a11y_contains(&notice, "磁盘文件已移动或删除"));
        assert!(a11y_contains(&source, first_line));
        fs::rename(&moved, &original).unwrap();
        wait_until("disk notice cleared after restoring the path", Duration::from_secs(6), || {
            view_with_id(root, "code-disk-state").is_none()
        });
        println!("NATIVE_OPEN_SOURCE_DISK_STATE PASS");

        send_click(window, &wait("editor-tab-working-diff"), 1);
        wait("working-diff-new-editor");
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        wait_until("working Diff shows disk read failure", Duration::from_secs(6), || {
            view_with_id(root, "diff-disk-state")
                .is_some_and(|notice| a11y_contains(&notice, "磁盘文件暂不可读"))
        });
        assert!(a11y_contains(&wait("working-diff-new-editor"), first_line));
        fs::remove_dir(&original).unwrap();
        wait_until("working Diff shows a deleted file", Duration::from_secs(6), || {
            view_with_id(root, "diff-disk-state")
                .is_some_and(|notice| a11y_contains(&notice, "磁盘文件已移动或删除"))
        });
        assert!(!a11y_contains(&wait("working-diff-new-editor"), first_line));
        fs::rename(&moved, &original).unwrap();
        wait_until("working Diff recovers after restoring the path", Duration::from_secs(6), || {
            view_with_id(root, "diff-disk-state").is_none()
                && view_with_id(root, "working-diff-new-editor")
                    .is_some_and(|editor| a11y_contains(&editor, first_line))
        });
        println!("NATIVE_OPEN_WORKTREE_DIFF_DISK_STATE PASS");

        let blob = git_run(repo, &["rev-parse", "--verify", ":review-lines.txt"]);
        let object_dir = git_run(repo, &["rev-parse", "--path-format=absolute", "--git-path", "objects"]);
        let loose_blob = Path::new(&object_dir).join(&blob[..2]).join(&blob[2..]);
        let backup = evidence.join("review-test-backup");
        let baseline_start = first_line.replacen("working", "baseline", 1);
        let refresh = || send_key(app, window, "y", "y", 16, command | NSEventModifierFlags::Option);
        fs::rename(&loose_blob, &backup).unwrap();
        refresh();
        wait_until("working Diff shows Git baseline failure", Duration::from_secs(6), || {
            view_with_id(root, "diff-disk-state")
                .is_some_and(|notice| a11y_contains(&notice, "Git 比较基准暂不可读"))
        });
        assert!(a11y_contains(&wait("working-diff-old-editor"), &baseline_start));
        assert!(a11y_contains(&wait("working-diff-new-editor"), first_line));
        fs::rename(&backup, &loose_blob).unwrap();
        refresh();
        wait_until("working Diff recovers its Git baseline", Duration::from_secs(6), || {
            view_with_id(root, "diff-disk-state").is_none()
        });
        println!("NATIVE_WORKTREE_DIFF_GIT_BASELINE_RECOVERY PASS");

        send_click(window, &wait("working-diff-new-editor"), 1);
        send_key(app, window, "x", "x", 7, NSEventModifierFlags::empty());
        wait("diff-dirty-badge");
        let binary = evidence.join("review-fixture-binary");
        fs::write(&binary, b"\0binary staged version\n").unwrap();
        let binary_blob = git_run(repo, &["hash-object", "-w", binary.to_str().unwrap()]);
        let stage = |oid: &str| {
            git_run(repo, &["update-index", "--cacheinfo", "100644", oid, "review-lines.txt"]);
        };
        stage(&binary_blob);
        wait_until("worktree draft stays visible with a binary Git baseline", Duration::from_secs(6), || {
            view_with_id(root, "diff-disk-state")
                .is_some_and(|notice| a11y_contains(&notice, "Git 比较基准为二进制或非 UTF-8 文件"))
        });
        wait("working-diff-new-editor");
        assert!(a11y_contains(&wait("diff-old-pane-header"), "上次有效版本"));
        assert!(view_with_id(root, "diff-stat-added").is_none());
        assert_eq!(fs::read_to_string(&original).unwrap(), content);
        send_key(app, window, "s", "s", 1, command);
        wait("diff-blocked-notice");
        assert_ne!(fs::read_to_string(&original).unwrap(), content);
        assert_eq!(git_run(repo, &["rev-parse", ":review-lines.txt"]), binary_blob);
        stage(&blob);
        wait("working-diff-new-editor");
        println!("NATIVE_WORKTREE_DIFF_DRAFT_WITH_BINARY_BASELINE PASS");

        let old_pane = wait("working-diff-old-editor");
        let old_point = center_in_window(&old_pane);
        move_pointer(old_point);
        assert!(a11y_contains(&wait("diff-blame-tooltip"), "Nemorous QA"));
        let previous_head = git_run(repo, &["rev-parse", "HEAD"]);
        let same_tree = git_run(repo, &["rev-parse", "HEAD^{tree}"]);
        let new_head = git_run(repo, &[
            "-c", "user.name=Alternate Author", "commit-tree", &same_tree, "-m", "Separate history",
        ]);
        git_run(repo, &["update-ref", "HEAD", &new_head, &previous_head]);
        wait_until("old attribution hidden after HEAD changed", Duration::from_secs(6), || {
            view_with_id(root, "diff-blame-tooltip").is_none()
        });
        move_pointer(NSPoint::new(old_point.x + 12.0, old_point.y));
        wait_until("moving within the word showed the new HEAD attribution", Duration::from_secs(6), || {
            view_with_id(root, "diff-blame-tooltip")
                .is_some_and(|tooltip| a11y_contains(&tooltip, "Alternate Author"))
        });
        assert_eq!(git_run(repo, &["rev-parse", ":review-lines.txt"]), blob);
        println!("NATIVE_WORKTREE_DIFF_ATTRIBUTION_FOLLOWS_HEAD PASS");

        if repo.join(".git").is_file() {
            let bytes = fs::read(&original).unwrap();
            // Updating symbolic HEAD above changed a shared ref. Detaching here
            // changes only the worktree's private HEAD file.
            git_run(repo, &["update-ref", "--no-deref", "HEAD", &previous_head]);
            wait_until("private worktree HEAD invalidated the tooltip", Duration::from_secs(6), || {
                view_with_id(root, "diff-blame-tooltip").is_none()
            });
            move_pointer(NSPoint::new(old_point.x + 24.0, old_point.y));
            wait_until("private worktree HEAD restored the previous attribution", Duration::from_secs(6), || {
                view_with_id(root, "diff-blame-tooltip")
                    .is_some_and(|tooltip| a11y_contains(&tooltip, "Nemorous QA"))
            });
            assert_eq!(fs::read(&original).unwrap(), bytes);
            assert_eq!(git_run(repo, &["rev-parse", ":review-lines.txt"]), blob);
            println!("NATIVE_LINKED_WORKTREE_PRIVATE_HEAD PASS");
        }

        let staged_fixture = evidence.join("review-fixture-staged-text");
        fs::write(&staged_fixture, content.replace("working", "staged")).unwrap();
        let staged_blob = git_run(repo, &["hash-object", "-w", staged_fixture.to_str().unwrap()]);
        stage(&staged_blob);
        wait("change-row-staged-review-lines.txt");
        wait_until("working Diff displays the new staged text", Duration::from_secs(6), || {
            view_with_id(root, "working-diff-old-editor")
                .is_some_and(|view| a11y_contains(&view, "staged"))
        });

        let hold = scope_gate.join("hold");
        fs::write(&hold, "").unwrap();
        send_click(window, &wait("change-row-staged-review-lines.txt"), 2);
        wait_until("staged Diff is waiting for Git", Duration::from_secs(4), || {
            scope_gate.join("reading").is_dir()
        });
        let toggle = wait("diff-layout-toggle");
        let point = center_in_window(&toggle);
        send_left_button(window, point, 1, true);
        pump(25);
        fs::remove_file(hold).unwrap();
        wait_until("staged Diff installed with the mouse down", Duration::from_secs(6), || {
            view_with_id(root, "diff-old-pane-bar").is_some_and(|view| a11y_contains(&view, "HEAD"))
        });
        assert!(std::ptr::eq(&*toggle, &*wait("diff-layout-toggle")));
        send_left_button(window, point, 1, false);
        let unified = wait("working-diff-unified-editor");
        assert!(a11y_contains(&unified, "+ staged"));
        assert_eq!(git_run(repo, &["rev-parse", ":review-lines.txt"]), staged_blob);
        println!("NATIVE_DIFF_LAYOUT_DURING_SCOPE_SWITCH PASS");

        let filename = "layout-viewport.txt";
        let baseline = (1..=240).map(|line| format!("baseline {line:03}\n")).collect::<String>();
        fs::write(repo.join(filename), &baseline).unwrap();
        git_run(repo, &["add", filename]);
        git_run(repo, &["commit", "--only", "-m", "Add layout viewport fixture", "--", filename]);
        let layout_commit = git_run(repo, &["rev-parse", "--short", "HEAD"]);
        fs::write(
            repo.join(filename),
            baseline.replace("baseline 140", "FIRST_LAYOUT_CHANGE").replace("baseline 220", "LATE_LAYOUT_CHANGE"),
        ).unwrap();
        send_click(window, &wait("change-row-unstaged-layout-viewport.txt"), 1);
        send_key(app, window, "d", "d", 2, command);
        let wait_for_marker = |id, marker| {
            wait_until(marker, Duration::from_secs(6), || {
                view_with_id(root, id).is_some_and(|editor| a11y_contains(&editor, marker))
            });
        };
        wait_for_marker("working-diff-unified-editor", "+ FIRST_LAYOUT_CHANGE");
        send_click(window, &wait("diff-next-hunk"), 1);
        wait_for_marker("working-diff-unified-editor", "+ LATE_LAYOUT_CHANGE");
        send_click(window, &wait("diff-layout-toggle"), 1);
        wait_for_marker("working-diff-new-editor", "LATE_LAYOUT_CHANGE");
        send_click(window, &wait("diff-layout-toggle"), 1);
        wait_for_marker("working-diff-unified-editor", "+ LATE_LAYOUT_CHANGE");
        println!("NATIVE_DIFF_LAYOUT_RETAINS_LATE_HUNK PASS");

        send_click(window, &wait("diff-layout-toggle"), 1);
        send_click(window, &wait("working-diff-new-editor"), 1);
        let path = repo.join(filename);
        let original_bytes = fs::read(&path).unwrap();
        send_key(app, window, "x", "x", 7, NSEventModifierFlags::empty());
        wait("diff-dirty-badge");
        send_click(window, &wait("diff-layout-toggle"), 1);
        wait("working-diff-unified-editor");
        send_click(window, &wait("diff-layout-toggle"), 1);
        send_click(window, &wait("working-diff-new-editor"), 1);
        let wait_clean = || {
            wait_until("native Diff undo removes the draft", Duration::from_secs(6), || {
                view_with_id(root, "diff-dirty-badge").is_none()
            });
        };
        send_key(app, window, "z", "z", 6, command);
        wait_clean();
        send_key(app, window, "Z", "z", 6, command | NSEventModifierFlags::Shift);
        wait("diff-dirty-badge");
        send_key(app, window, "z", "z", 6, command);
        wait_clean();
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        println!("NATIVE_DIFF_LAYOUT_PRESERVES_UNDO_AND_REDO PASS");

        send_key(app, window, "x", "x", 7, NSEventModifierFlags::empty());
        wait("diff-dirty-badge");
        let other = repo.join("README.md");
        let other_bytes = fs::read(&other).unwrap();
        send_click(window, &wait("change-row-unstaged-README.md"), 1);
        send_key(app, window, "d", "d", 2, command);
        wait_until("other worktree Diff", Duration::from_secs(6), || {
            view_with_id(root, "working-diff-pane")
                .is_some_and(|pane| a11y_contains(&pane, "工作区 · README.md"))
        });
        let saved_bytes = fs::read(&path).unwrap();
        assert_ne!(saved_bytes, original_bytes, "switching should save the first file");
        send_click(window, &wait("working-diff-new-editor"), 1);
        send_key(app, window, "z", "z", 6, command);
        assert!(view_with_id(root, "diff-dirty-badge").is_none());
        assert_eq!(fs::read(&other).unwrap(), other_bytes);

        send_key(app, window, "[", "[", 33, command);
        wait_until("original worktree Diff", Duration::from_secs(6), || {
            view_with_id(root, "working-diff-pane")
                .is_some_and(|pane| a11y_contains(&pane, &format!("工作区 · {filename}")))
        });
        send_click(window, &wait("working-diff-new-editor"), 1);
        send_key(app, window, "z", "z", 6, command);
        wait("diff-dirty-badge");
        assert_eq!(fs::read(&path).unwrap(), saved_bytes);
        send_key(app, window, "Z", "z", 6, command | NSEventModifierFlags::Shift);
        wait_clean();
        send_key(app, window, "z", "z", 6, command);
        wait("diff-dirty-badge");
        send_key(app, window, "s", "s", 1, command);
        wait_clean();
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        println!("NATIVE_DIFF_FILE_SWITCH_PRESERVES_UNDO_AND_REDO PASS");

        for single in [false, true] {
            send_click(window, &wait("working-diff-new-editor"), 1);
            send_key(app, window, "x", "x", 7, NSEventModifierFlags::empty());
            wait_until("initial Diff edit", Duration::from_secs(6), || {
                view_with_id(root, "diff-dirty-badge").is_some()
            });
            let diff_editor = if single {
                send_click(window, &wait("diff-layout-toggle"), 1);
                wait("working-diff-unified-editor");
                "working-diff-unified-editor"
            } else {
                "working-diff-new-editor"
            };
            send_key(app, window, "\u{F707}", "\u{F707}", 118, NSEventModifierFlags::Function);
            wait("code-editor-view");
            wait("code-dirty-badge");
            send_key(app, window, "z", "z", 6, command);
            wait_until("source undoes the Diff edit", Duration::from_secs(6), || {
                view_with_id(root, "code-dirty-badge").is_none()
            });
            send_key(app, window, "[", "[", 33, command);
            wait_until(&format!("return to {diff_editor}"), Duration::from_secs(6), || {
                view_with_id(root, diff_editor).is_some()
            });
            wait_clean();
            let editor = wait(diff_editor);
            wait_until(&format!("focus in {diff_editor}"), Duration::from_secs(6), || {
                window.firstResponder()
                    .and_then(|responder| responder.downcast::<NSView>().ok())
                    .is_some_and(|responder| responder.isDescendantOf(&editor))
            });
            send_key(app, window, "Z", "z", 6, command | NSEventModifierFlags::Shift);
            wait_until(&format!("redo in {diff_editor}"), Duration::from_secs(6), || {
                view_with_id(root, "diff-dirty-badge").is_some()
            });
            send_key(app, window, "z", "z", 6, command);
            wait_clean();
            assert_eq!(fs::read(&path).unwrap(), original_bytes);
            println!("NATIVE_RETURN_TO_DIFF_RETAINS_KEYBOARD_FOCUS single={single} PASS");
        }
        println!("NATIVE_DIFF_AND_SOURCE_SHARE_UNDO_AND_REDO PASS");

        send_click(window, &wait("change-row-unstaged-layout-viewport.txt"), 1);
        send_key(app, window, "d", "d", 2, command);
        wait_for_marker("working-diff-unified-editor", "+ FIRST_LAYOUT_CHANGE");
        let goto_late_change = || {
            send_key(app, window, "l", "l", 37, command);
            send_click(window, &wait("goto-line-input"), 1);
            for (digit, code) in [("2", 19), ("2", 19), ("0", 29)] {
                send_key(app, window, digit, digit, code, NSEventModifierFlags::empty());
            }
            send_key(app, window, "\r", "\r", 36, NSEventModifierFlags::empty());
            wait_for_marker("status-cursor-pos", "220:1");
            wait_for_marker("diff-hunk-counter", "2/2");
        };
        goto_late_change();
        send_click(window, &wait("diff-prev-hunk"), 1);
        wait_for_marker("working-diff-unified-editor", "+ FIRST_LAYOUT_CHANGE");
        goto_late_change();
        send_key(app, window, "z", "z", 6, command | NSEventModifierFlags::Option);
        wait("diff-dirty-badge");
        wait_for_marker("diff-hunk-counter", "1/1");
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        send_key(app, window, "z", "z", 6, command);
        wait_clean();
        wait_for_marker("diff-hunk-counter", "2");
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        println!("NATIVE_DIFF_CARET_NAVIGATION_AND_BLOCK_ROLLBACK PASS");

        let line_fixture = "line-staging.txt";
        let line_path = repo.join(line_fixture);
        let old_lines = "old A\nold B\ncontext\nlast\n";
        let working_lines = "new A\nnew B\ncontext\nlast\n";
        fs::write(&line_path, old_lines).unwrap();
        git_run(repo, &["add", line_fixture]);
        git_run(repo, &["commit", "--only", "-m", "Add line staging fixture", "--", line_fixture]);
        fs::write(&line_path, working_lines).unwrap();
        send_click(window, &wait("change-row-unstaged-line-staging.txt"), 1);
        send_key(app, window, "d", "d", 2, command);
        wait_for_marker("working-diff-unified-editor", "+ new B");
        let unified = wait("working-diff-unified-editor");
        choose_context_item(window, &unified, "暂存选中行");
        wait("change-row-staged-line-staging.txt");
        assert_eq!(git_run(repo, &["show", ":line-staging.txt"]), "new A\nold B\ncontext\nlast");
        assert_eq!(fs::read_to_string(&line_path).unwrap(), working_lines);

        send_click(window, &wait("change-row-staged-line-staging.txt"), 2);
        wait_until("staged line in unified Diff", Duration::from_secs(6), || {
            view_with_id(root, "working-diff-unified-editor").is_some_and(|editor| {
                a11y_contains(&editor, "+ new A") && !a11y_contains(&editor, "+ new B")
            })
        });
        choose_context_item(window, &wait("working-diff-unified-editor"), "取消暂存选中行");
        wait_until("line is unstaged", Duration::from_secs(6), || {
            view_with_id(root, "change-row-staged-line-staging.txt").is_none()
        });
        assert_eq!(git_run(repo, &["show", ":line-staging.txt"]), old_lines.trim_end());
        assert_eq!(fs::read_to_string(&line_path).unwrap(), working_lines);
        println!("NATIVE_UNIFIED_DIFF_STAGE_AND_UNSTAGE_LINE PASS");

        send_click(window, &wait("change-row-unstaged-layout-viewport.txt"), 1);
        send_key(app, window, "d", "d", 2, command);
        wait_for_marker("working-diff-unified-editor", "+ FIRST_LAYOUT_CHANGE");
        send_key(app, window, "\u{F707}", "\u{F707}", 118, NSEventModifierFlags::Function);
        let editor = wait("code-editor-view");
        choose_context_item(window, &editor, "与修订版本比较…");
        send_click(window, &wait(&format!("file-revision-{layout_commit}")), 2);
        wait_for_marker("comparison-right-editor", "FIRST_LAYOUT_CHANGE");
        wait_for_marker("comparison-change-count", "1/2");
        send_key(app, window, "\u{F70A}", "\u{F70A}", 98, NSEventModifierFlags::Function);
        wait_for_marker("comparison-right-editor", "LATE_LAYOUT_CHANGE");
        wait_for_marker("comparison-change-count", "2/2");
        println!("NATIVE_REVISION_COMPARISON_LATE_CHANGES PASS");

        send_key(app, window, "f", "f", 3, command);
        move_pointer(center_in_window(&wait("diff-find-input")));
        for (key, keycode) in [("l", 37), ("a", 0), ("t", 17), ("e", 14)] {
            send_key(app, window, key, key, keycode, NSEventModifierFlags::empty());
        }
        wait_for_marker("diff-find-matches", "1 个结果");
        send_key(app, window, "g", "g", 5, command);
        wait_for_marker("diff-find-matches", "1/1");
        send_key(app, window, "\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
        wait_until("comparison search bar closed", Duration::from_secs(6), || view_with_id(root, "diff-find-bar").is_none());
        send_key(app, window, "\u{F70A}", "\u{F70A}", 98, NSEventModifierFlags::Function | NSEventModifierFlags::Shift);
        wait_for_marker("comparison-change-count", "1/2");
        println!("NATIVE_REVISION_COMPARISON_FIND PASS");

        send_key(app, window, "w", "w", 13, command);
        wait("code-editor-view");
        let repo_id = nemorous::git::discover_workspace_repos(repo.to_str().unwrap(), &[])
            .into_iter().find(|entry| entry.is_root).unwrap().id;
        let snapshot = nemorous::local_history::LocalHistoryStore::default_store()
            .record(&repo_id, filename, "Snapshot for late changes", baseline.as_bytes())
            .unwrap();
        choose_context_item(window, &wait("code-editor-view"), "显示历史…");
        if let Some(snapshot) = snapshot {
            send_click(window, &wait(&format!("local-history-entry-{}", snapshot.id)), 1);
        }
        wait_for_marker("local-history-current", "FIRST_LAYOUT_CHANGE");
        wait_for_marker("local-history-change-count", "1 / 2");
        send_key(app, window, "\u{F70A}", "\u{F70A}", 98, NSEventModifierFlags::Function);
        wait_for_marker("local-history-current", "LATE_LAYOUT_CHANGE");
        wait_for_marker("local-history-change-count", "2 / 2");
        send_key(app, window, "\u{F70A}", "\u{F70A}", 98, NSEventModifierFlags::Function | NSEventModifierFlags::Shift);
        wait_for_marker("local-history-change-count", "1 / 2");
        wait_for_marker("local-history-current", "FIRST_LAYOUT_CHANGE");
        println!("NATIVE_LOCAL_HISTORY_LATE_CHANGES PASS");

        send_key(app, window, "\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
        let tail_file = "native-tail.txt";
        let tail_path = repo.join(tail_file);
        let short = baseline.replace("baseline 240\n", "tail 🐈");
        let long = short.replace("baseline 140", "NATIVE_EARLIER_CHANGE") + "\nNATIVE_APPENDED_TAIL\n";
        fs::write(&tail_path, &short).unwrap();
        git_run(repo, &["add", tail_file]);
        git_run(repo, &["commit", "--only", "-m", "Add tail navigation fixture", "--", tail_file]);
        fs::write(&tail_path, &long).unwrap();
        send_click(window, &wait("change-row-unstaged-native-tail.txt"), 1);
        send_key(app, window, "d", "d", 2, command);
        wait_for_marker("working-diff-unified-editor", "+ NATIVE_EARLIER_CHANGE");
        send_click(window, &wait("diff-layout-toggle"), 1);
        wait_for_marker("working-diff-new-editor", "NATIVE_EARLIER_CHANGE");
        choose_view_option("diff-options-btn", "忽略首尾空白");
        send_click(window, &wait("diff-old-pane-bar"), 1);
        send_key(app, window, "\u{F70A}", "\u{F70A}", 98, NSEventModifierFlags::Function);
        wait_for_marker("working-diff-new-editor", "NATIVE_APPENDED_TAIL");
        wait_for_marker("diff-hunk-counter", "2/2");
        wait_for_marker("status-cursor-pos", "240:8");
        send_click(window, &wait("diff-layout-toggle"), 1);
        wait_for_marker("working-diff-unified-editor", "+ NATIVE_APPENDED_TAIL");
        wait_for_marker("diff-hunk-counter", "2/2");
        send_click(window, &wait("diff-layout-toggle"), 1);
        wait_for_marker("working-diff-new-editor", "NATIVE_APPENDED_TAIL");
        wait_for_marker("diff-hunk-counter", "2/2");
        send_key(app, window, "\u{F70A}", "\u{F70A}", 98, NSEventModifierFlags::Function | NSEventModifierFlags::Shift);
        wait_for_marker("working-diff-new-editor", "NATIVE_EARLIER_CHANGE");
        wait_for_marker("diff-hunk-counter", "1/2");
        assert_eq!(fs::read_to_string(&tail_path).unwrap(), long);
        println!("NATIVE_SPLIT_DIFF_TAIL_HUNK PASS");

        send_key(app, window, "\u{F707}", "\u{F707}", 118, NSEventModifierFlags::Function);
        let source = wait("code-editor-view");
        fs::write(repo.join("a-native-scope.txt"), "NATIVE_SCOPE_FIRST\n").unwrap();
        fs::write(repo.join("z-native-scope.txt"), "NATIVE_SCOPE_LAST\n").unwrap();
        git_run(repo, &["add", "a-native-scope.txt", tail_file, "z-native-scope.txt"]);
        git_run(repo, &["commit", "--only", "-m", "Review three files in native history", "--", "a-native-scope.txt", tail_file, "z-native-scope.txt"]);
        let history_commit = git_run(repo, &["rev-parse", "--short", "HEAD"]);
        choose_context_item(window, &source, "查看文件历史");
        wait("git-file-history");
        send_click(window, &wait(&format!("git-commit-row-{history_commit}")), 1);
        wait("git-commit-file-native-tail.txt");
        choose_view_option("git-file-view-options", "显示所有受影响的文件");
        send_click(window, &wait("git-commit-file-a-native-scope.txt"), 1);
        send_key(app, window, "d", "d", 2, command);
        wait_for_marker("commit-diff-new-editor", "NATIVE_SCOPE_FIRST");
        choose_view_option("git-file-view-options", "显示所有受影响的文件");
        wait_until("file history shows only the tracked file", Duration::from_secs(6), || {
            view_with_id(root, "git-commit-file-a-native-scope.txt").is_none()
                && view_with_id(root, "git-commit-file-native-tail.txt").is_some()
        });
        send_click(window, &wait("diff-next-file"), 1);
        wait_for_marker("commit-diff-new-editor", "NATIVE_EARLIER_CHANGE");
        send_click(window, &wait("diff-next-file"), 1);
        wait_for_marker("commit-diff-new-editor", "NATIVE_SCOPE_LAST");
        send_click(window, &wait("git-show-diff"), 1);
        wait_for_marker("commit-diff-new-editor", "NATIVE_EARLIER_CHANGE");
        println!("NATIVE_FILE_HISTORY_SCOPE_NAVIGATION PASS");

        send_key(app, window, "f", "f", 3, command);
        wait("diff-find-input");
        for (key, keycode) in [("n", 45), ("a", 0), ("t", 17)] {
            send_key(app, window, key, key, keycode, NSEventModifierFlags::empty());
        }
        wait_for_marker("diff-find-matches", "2 个结果");
        send_click(window, &wait("diff-find-case"), 1);
        wait_for_marker("diff-find-matches", "0 个结果");
        send_key(app, window, "f", "f", 3, command);
        for (upper, lower, keycode) in [("N", "n", 45), ("A", "a", 0), ("T", "t", 17)] {
            send_key(app, window, upper, lower, keycode, NSEventModifierFlags::Shift);
        }
        wait_for_marker("diff-find-matches", "2 个结果");
        move_pointer(center_in_window(&wait("diff-find-input")));
        wait_until("search option tooltip hidden", Duration::from_secs(6), || view_with_id(root, "toolbar-tooltip").is_none());
        send_key(app, window, "\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
        wait_until("Diff search bar closed", Duration::from_secs(6), || view_with_id(root, "diff-find-bar").is_none());
        println!("NATIVE_DIFF_FIND_KEYBOARD_AND_MODE PASS");
    }

    fn crisp_screen_number(screen: &NSScreen) -> Option<u32> {
        let number = screen.deviceDescription().objectForKey(&NSString::from_str("NSScreenNumber"))?;
        Some(number.downcast::<NSNumber>().ok()?.unsignedIntValue())
    }

    const CRISP_FIRST_AUTHOR_DATE: &str = "2024-03-04T09:12:00+00:00";
    const CRISP_SECOND_AUTHOR_DATE: &str = "2025-06-07T16:43:00+00:00";
    const CRISP_SECOND_SUBJECT: &str =
        "Crisp later rows keep their full commit subject visible when the Git log is narrow";
    const CRISP_TOOL_LONG_NAME: &str = "000-very-long-filename-for-checking-horizontal-scrolling-in-project-and-local-changes-with-a-distinct-ending.txt";

    fn check_crisp_tool_scroll(
        window: &NSWindow,
        root: &NSView,
        input: &NSView,
        evidence: &Path,
        viewport: &str,
        header: &str,
        row: &str,
    ) {
        wait_until(row, Duration::from_secs(6), || view_with_id(root, row).is_some());
        let viewport_view = view_with_id(root, viewport).unwrap();
        let header = view_with_id(root, header).unwrap();
        let point = center_in_window(&viewport_view);
        let header_before = center_in_window(&header);
        let scrollbar = viewport_view.accessibilityHorizontalScrollBar()
            .and_then(|bar| bar.downcast::<NSAccessibilityElement>().ok())
            .expect("horizontal accessibility scrollbar");
        // NSH paints wheel changes before it commits AppKit's NSView geometry.
        let offset = || {
            scrollbar.accessibilityValue()
                .and_then(|value| value.downcast::<NSNumber>().ok())
                .expect("horizontal scrollbar position")
                .doubleValue()
        };
        let row_y = || view_with_id(root, row).map(|view| center_in_window(&view).y);
        dispatch_wheel_delta(window, input, point, -140, 0);
        wait_until("sidebar starts at the filename beginning", Duration::from_secs(3), || offset() <= 0.01);
        let initial_y = row_y().unwrap();
        dispatch_wheel_delta(window, input, point, 140, 0);
        wait_until("sidebar scrolled to the filename ending", Duration::from_secs(3), || offset() >= 0.99);
        assert!((row_y().unwrap() - initial_y).abs() < 1.0);
        assert_eq!(center_in_window(&header), header_before);
        save_window_png(window, &evidence.join(format!("{viewport}-end.png")));
        dispatch_wheel_delta(window, input, point, -140, 0);
        wait_until("sidebar returned to the start", Duration::from_secs(3), || offset() <= 0.01);
        save_window_png(window, &evidence.join(format!("{viewport}-start.png")));
    }

    fn check_crisp_diff_attribution(
        window: &NSWindow,
        root: &NSView,
        input: &NSView,
        fixture: &Path,
        is_commit: bool,
    ) {
        let (editor_id, name, wheel_steps) = if is_commit {
            ("commit-diff-unified-editor", "commit-attribution", [36, 36, 36])
        } else {
            ("working-diff-old-editor", "diff-attribution", [12, 12, 12])
        };
        let tip = "diff-blame-tooltip";
        wait_until(editor_id, Duration::from_secs(8), || view_with_id(root, editor_id).is_some());
        let editor = view_with_id(root, editor_id).unwrap();
        let mut sample = editor.bounds();
        if !editor.isFlipped() {
            sample.origin.y += sample.size.height - 110.0;
        }
        sample.size.height = 110.0;
        sample.size.width -= 22.0;
        let point = editor.convertPoint_toView(NSPoint::new(
            sample.origin.x + 148.0,
            sample.origin.y + 55.0,
        ), None);

        let check_tip = |author, date, subject, point: NSPoint| {
            let date = chrono::DateTime::parse_from_rfc3339(date).unwrap()
                .with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M");
            let title = format!("{author} · {date}");
            wait_until(&title, Duration::from_secs(5), || view_with_id(root, tip).is_some());
            let hint = view_with_id(root, tip).unwrap();
            assert!(a11y_contains(&hint, &title), "expected {title}, got {}", view_a11y_text(&hint));
            assert!(a11y_contains(&hint, subject), "missing commit subject {subject}");
            assert!((center_in_window(&hint).y - point.y).abs() < 80.0, "attribution detached from hovered row");
        };

        move_pointer(window, input, point);
        check_tip("Crisp First Author", CRISP_FIRST_AUTHOR_DATE, "Crisp initial rows", point);
        save_window_png(window, &fixture.join(format!("{name}-before-wheel.png")));
        let before = snapshot_png(&editor, sample);
        dispatch_wheel(window, input, point);
        wait_until("the first wheel tick moved the viewport", Duration::from_millis(800), || {
            if snapshot_png(&editor, sample) == before {
                return false;
            }
            assert!(view_with_id(root, tip).is_none(), "the first scrolled frame retained attribution");
            true
        });
        for dy in wheel_steps {
            dispatch_wheel_delta(window, input, point, 0, dy);
            pump(70);
            assert!(view_with_id(root, tip).is_none(), "attribution returned during scrolling");
        }
        move_pointer(window, input, point);
        pump(550);
        assert!(view_with_id(root, tip).is_none(), "attribution returned under the stationary pointer");
        save_window_png(window, &fixture.join(format!("{name}-after-wheel.png")));

        let moved = NSPoint::new(point.x + 8.0, point.y);
        move_pointer(window, input, moved);
        check_tip("Crisp Second Author", CRISP_SECOND_AUTHOR_DATE, CRISP_SECOND_SUBJECT, moved);
        save_window_png(window, &fixture.join(format!("{name}-after-hover.png")));
    }

    fn run_on_crisp(root_dir: &Path) {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGDisplayVendorNumber(display: u32) -> u32;
            fn CGDisplayIsActive(display: u32) -> u32;
            fn CGDisplayIsMain(display: u32) -> u32;
            fn CGDisplayIsInMirrorSet(display: u32) -> u32;
        }

        let mtm = MainThreadMarker::new().expect("must run on main thread");
        let offscreen_diagnostics = std::env::var_os("NEMOROUS_CRISP_OFFSCREEN_DIAGNOSTICS").is_some();
        let (screen, number) = NSScreen::screens(mtm).iter().find_map(|screen| {
            let number = crisp_screen_number(&screen)?;
            // Crisp stamps its virtual display descriptors with this vendor ID.
            let eligible = unsafe {
                CGDisplayVendorNumber(number) == 0xEEEE
                    && (offscreen_diagnostics || CGDisplayIsActive(number) != 0)
                    && CGDisplayIsMain(number) == 0
                    && CGDisplayIsInMirrorSet(number) == 0
            };
            eligible.then_some((screen, number))
        }).expect("an active extended Crisp virtual display is required");
        let area = screen.visibleFrame();
        let size = NSSize::new(1280.0, 800.0);
        assert!(area.size.width >= size.width + 40.0 && area.size.height >= size.height + 80.0);

        // NSH can catch a panic on its engine thread and keep rendering.
        let runtime_panicked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let record_panic = runtime_panicked.clone();
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record_panic.store(true, std::sync::atomic::Ordering::Relaxed);
            previous_hook(info);
        }));

        let fixture = root_dir.join(format!("build/tmp-ab/native-crisp-{}", std::process::id()));
        let changes = std::env::var_os("NEMOROUS_NATIVE_CRISP_CHANGES").is_some();
        let multi_workspace = fixture.join("workspace");
        let repo = if changes { multi_workspace.join("primary-review") } else { fixture.join("repo") };
        let linked = std::env::var_os("NEMOROUS_NATIVE_LINKED_WORKTREE").is_some();
        let tool_width = std::env::var("NEMOROUS_NATIVE_LEFT_TOOL_WIDTH").ok()
            .map(|width| width.parse::<f32>().expect("left tool window width"));
        let primary = if linked { fixture.join("primary") } else { repo.clone() };
        let ux = std::env::var_os("NEMOROUS_NATIVE_UX").is_some();
        let menus = std::env::var_os("NEMOROUS_NATIVE_CRISP_MENUS").is_some();
        let empty_recent = std::env::var_os("NEMOROUS_NATIVE_CRISP_MENU_EMPTY").is_some();
        let startup_repo = std::env::var_os("NEMOROUS_CRISP_STARTUP_REPO").map(PathBuf::from);
        let workspace = startup_repo.as_deref().unwrap_or(if changes { &multi_workspace } else { &repo });
        let path = repo.join("history.txt");
        let attribution_file = "attribution.txt";
        let committed_file = "committed-attribution.txt";
        fs::create_dir_all(&primary).unwrap();
        let baseline: String = (1..=250).map(|line| format!("baseline {line:03}\n")).collect();
        let modified = baseline.replace("baseline 115", "CRISP_FIRST_CHANGE").replace("baseline 225", "CRISP_LAST_CHANGE");
        fs::write(primary.join("history.txt"), &baseline).unwrap();
        let attribution_before: String = (1..=150).map(|line| format!("Original attribution line {line:03}\n")).collect();
        let attribution_after: String = (1..=150).map(|line| {
            let label = if line < 19 { "Original" } else { "Reviewed" };
            format!("{label} attribution line {line:03}\n")
        }).collect();
        fs::write(primary.join(attribution_file), attribution_before).unwrap();
        // No shared lines in this file. The top of its unified Diff is all
        // deletions and the bottom is all additions, even with the log open.
        let committed_before: String = (1..=40).map(|line| format!("Parent revision line {line:03}\n")).collect();
        let committed_after: String = (1..=120).map(|line| format!("Selected revision line {line:03}\n")).collect();
        fs::write(primary.join(committed_file), committed_before).unwrap();
        if tool_width.is_some() {
            write_left_tool_files(&primary);
        }
        if ux {
            let java = format!(
                "package example;\n\nfinal class APreview {{\n{}    String label() {{\n        String attribution = \"中文示例\";\n        return attribution.strip();\n    }}\n}}\n",
                "    // Context above the selected result\n".repeat(70),
            );
            fs::write(primary.join("APreview.java"), java).unwrap();
        }
        git_run(&primary, &["init", "-b", "main"]);
        git_run(&primary, &["add", "."]);
        git_run(&primary, &["-c", "user.name=Crisp First Author", "-c", "user.email=first@example.invalid", "-c", "commit.gpgsign=false", "commit", "--date", CRISP_FIRST_AUTHOR_DATE, "-m", "Crisp initial rows"]);
        fs::write(primary.join(attribution_file), &attribution_after).unwrap();
        fs::write(primary.join(committed_file), committed_after).unwrap();
        git_run(&primary, &["-c", "user.name=Crisp Second Author", "-c", "user.email=second@example.invalid", "-c", "commit.gpgsign=false", "commit", "--date", CRISP_SECOND_AUTHOR_DATE, "-m", CRISP_SECOND_SUBJECT, "--", attribution_file, committed_file]);
        let second_commit = git_run(&primary, &["rev-parse", "--short", "HEAD"]);
        let first_commit = git_run(&primary, &["rev-parse", "--short", "HEAD^"]);
        if linked {
            git_run(&primary, &["worktree", "add", "-b", "crisp-review", repo.to_str().unwrap(), "HEAD"]);
        }
        if ux {
            fs::write(&path, baseline.replace("baseline 115", "CRISP_STAGED_CHANGE")).unwrap();
            git_run(&repo, &["add", "history.txt"]);
        }
        fs::write(&path, &modified).unwrap();
        let attribution_working = attribution_after
            .replace("Original attribution line 003", "Uncommitted attribution line 003")
            .replace("Reviewed attribution line", "Working attribution line");
        fs::write(repo.join(attribution_file), attribution_working).unwrap();
        let other_repo = multi_workspace.join("other-review");
        if changes {
            git_run(&repo, &["restore", "--", "history.txt", attribution_file]);
            fs::create_dir_all(&other_repo).unwrap();
            git_run(&other_repo, &["init", "-b", "main"]);
            for (root, suffix) in [(&repo, "main"), (&other_repo, "other")] {
                git_run(root, &["config", "user.name", "Crisp Fixture"]);
                git_run(root, &["config", "user.email", "crisp-fixture@example.invalid"]);
                git_run(root, &["config", "commit.gpgsign", "false"]);
                if suffix == "other" {
                    git_run(root, &["commit", "--allow-empty", "-m", "Other fixture baseline"]);
                }
                fs::create_dir_all(root.join("checks")).unwrap();
                for stem in ["match", "keep"] {
                    fs::write(root.join(format!("checks/{stem}-{suffix}.txt")), "Fixture change\n").unwrap();
                }
            }
            fs::write(repo.join("checks/staged-main.txt"), "Staged fixture\n").unwrap();
            git_run(&repo, &["add", "checks/staged-main.txt"]);
        }
        let config = fixture.join("config.json");
        let mut settings = serde_json::json!({
            "workspace_path": workspace.to_string_lossy(),
            "theme": "darcula",
            "change_diff_preview": false,
            "diff_single_column": false,
            "auto_update": false
        });
        let recent = fixture.join("project-with-a-long-name");
        if menus && !empty_recent {
            fs::create_dir_all(&recent).unwrap();
            fs::write(recent.join("README.md"), "Recently opened project\n").unwrap();
            settings["recent_workspaces"] = serde_json::json!([recent]);
        }
        if changes { settings["changes_flat"] = false.into(); }
        if let Some(width) = tool_width {
            fs::write(repo.join("src/nested/tracked.txt"), "first\nworking edit\nlast\n").unwrap();
            settings["commit_left_width"] = width.into();
            settings["code_left_width"] = width.into();
            settings["changes_flat"] = true.into();
        }
        fs::write(&config, settings.to_string()).unwrap();
        // Override configuration before mounting the test app and its workers.
        unsafe {
            std::env::set_var("NEMOROUS_CONFIG", &config);
            if menus { std::env::remove_var("NEMOROUS_WORKSPACE"); }
            else { std::env::set_var("NEMOROUS_WORKSPACE", workspace); }
        }
        let root_repo = nemorous::git::discover_workspace_repos(repo.to_str().unwrap(), &[])
            .into_iter().find(|entry| entry.is_root).unwrap();
        let snapshot = nemorous::local_history::LocalHistoryStore::default_store()
            .record(&root_repo.id, "history.txt", "Crisp history snapshot", baseline.as_bytes())
            .unwrap().expect("record a distinct baseline snapshot");
        let snapshot_row = format!("local-history-entry-{}", snapshot.id);

        let app = NSApplication::sharedApplication(mtm);
        assert!(app.setActivationPolicy(NSApplicationActivationPolicy::Accessory));
        app.finishLaunching();
        assert!(!app.isActive(), "the Crisp test app must remain in the background");
        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            NSRect::new(NSPoint::new(area.origin.x + 20.0, area.origin.y + 20.0), size),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe { window.setReleasedWhenClosed(false) };
        window.setBecomesKeyOnlyIfNeeded(true);
        window.setHidesOnDeactivate(false);
        window.setTitle(&NSString::from_str("Nemorous Crisp 测试"));
        window.setFrameOrigin(NSPoint::new(area.origin.x + 20.0, area.origin.y + 20.0));

        let check_window = || {
            assert!(!app.isActive() && !window.isKeyWindow(), "background test changed keyboard focus");
            assert!(unsafe { (offscreen_diagnostics || CGDisplayIsActive(number) != 0) && CGDisplayIsMain(number) == 0 && CGDisplayIsInMirrorSet(number) == 0 });
            if offscreen_diagnostics {
                assert!(!window.isVisible(), "offscreen diagnostic became visible");
            }
            assert_eq!(window.screen().as_deref().and_then(crisp_screen_number), Some(number));
            let frame = window.frame();
            assert!(frame.origin.x >= area.origin.x && frame.origin.y >= area.origin.y);
            assert!(frame.origin.x + frame.size.width <= area.origin.x + area.size.width);
            assert!(frame.origin.y + frame.size.height <= area.origin.y + area.size.height);
        };
        check_window();

        let mut options = platform::AppOptions::default();
        options.title = "Nemorous Crisp Test".to_owned();
        options.size = (1280, 800);
        let startup = Instant::now();
        let surface = platform::AppView::new(mtm, options);
        window.setContentView(Some(&surface.view()));
        assert!(surface.suppress_background_test_cursor());
        surface.load(platform::App::Rust(platform::Root::of::<nemorous::App>()));
        check_window();
        if !offscreen_diagnostics {
            window.orderFrontRegardless();
        }
        check_window();
        let root = surface.view();
        let input_surface = root.subviews().firstObject().expect("AppKit input surface");
        let click = |id: &str, count| {
            wait_until(id, Duration::from_secs(12), || view_with_id(&root, id).is_some());
            check_window();
            let target = view_with_id(&root, id).unwrap();
            // A non-clipping NSView can report more than its own bounds as visible.
            let target_area = NSIntersectionRect(target.bounds(), target.visibleRect());
            let mut visible = target.convertRect_toView(target_area, None);
            let viewport = if id.starts_with("change-row-") {
                view_with_id(&root, "changes-tree-scroll")
            } else if id.starts_with("project-file-") || id.starts_with("project-dir-") {
                view_with_id(&root, "project-tree-scroll")
            } else {
                None
            };
            if let Some(viewport) = viewport {
                let clip = viewport.convertRect_toView(viewport.bounds(), None);
                visible = NSIntersectionRect(visible, clip);
            }
            assert!(visible.size.width > 0.0 && visible.size.height > 0.0, "{id} is clipped");
            let point = NSPoint::new(
                visible.origin.x + visible.size.width * 0.5,
                visible.origin.y + visible.size.height * 0.5,
            );
            for clicks in 1..=count {
                input_surface.mouseDown(&left_button_event(&window, point, clicks, true));
                pump(25);
                input_surface.mouseUp(&left_button_event(&window, point, clicks, false));
                pump(40);
            }
            check_window();
        };
        let expect = |id: &str, text: &str| {
            wait_until(&format!("{id} shows {text}"), Duration::from_secs(8), || {
                view_with_id(&root, id).is_some_and(|view| a11y_contains(&view, text))
            });
            check_window();
        };
        let key = |chars: &str, ignoring: &str, code, modifiers| {
            check_window();
            let chars = NSString::from_str(chars);
            let ignoring = NSString::from_str(ignoring);
            let now = NSProcessInfo::processInfo().systemUptime();
            for (kind, offset) in [(NSEventType::KeyDown, 0.0), (NSEventType::KeyUp, 0.01)] {
                let event = make_key_event(&window, kind, modifiers, now + offset, &chars, &ignoring, code);
                let handled = surface.dispatch_background_test_key(&event);
                if kind == NSEventType::KeyDown {
                    assert!(handled, "Crisp local key-down dispatch failed for {code}");
                }
            }
            pump(60);
            check_window();
        };
        let clean = |id| {
            wait_until(&format!("{id} removed"), Duration::from_secs(6), || view_with_id(&root, id).is_none());
        };
        let editor_has_responder = |id| {
            view_with_id(&root, id).is_some_and(|editor| {
                window.firstResponder()
                    .and_then(|responder| responder.downcast::<NSView>().ok())
                    .is_some_and(|responder| responder.isDescendantOf(&editor))
            })
        };

        if changes {
            let first = "change-row-unstaged-checks/match-main.txt-check";
            let first_keep = "change-row-unstaged-checks/keep-main.txt-check";
            let other = "change-row-unstaged-checks/match-other.txt-check";
            let other_keep = "change-row-unstaged-checks/keep-other.txt-check";
            let staged = "change-row-staged-checks/staged-main.txt-check";
            let repo_ids = nemorous::git::discover_workspace_repos(&workspace.to_string_lossy(), &[]);
            let repo_id = |path: &Path| repo_ids.iter().find(|entry| Path::new(&entry.path) == path).unwrap().id.as_str();
            let other_group = format!("change-repo-check-unstaged::{}", repo_id(&other_repo));
            let first_directory = format!("change-dir-check-unstaged::{}::checks", repo_id(&repo));
            let checked = |id: &str, expected: bool| {
                wait_until(&format!("{id} checked={expected}"), Duration::from_secs(5), || {
                    view_with_id(&root, id).and_then(|view| view.accessibilityValue())
                        .and_then(|value| value.downcast::<NSNumber>().ok())
                        .is_some_and(|value| value.integerValue() == i64::from(expected) as isize)
                });
                check_window();
            };
            checked(first, true);
            let changes_visible_ms = startup.elapsed().as_millis();
            checked(other, true);
            checked(staged, true);
            for id in ["changes-header-toolbar-row", "changes-section-staged", "changes-section-unstaged"] {
                let row = view_with_id(&root, id).unwrap();
                assert!(row.subviews().iter().all(|child| {
                    child.accessibilityRole().is_none_or(|role| role.to_string() != "AXCheckBox")
                }), "unexpected global checkbox in {id}");
            }
            save_window_png(&window, &fixture.join("commit-multi-repo.png"));
            click(&other_group, 1);
            checked(other, false);
            checked(other_keep, false);
            checked(first, true);
            click(&other_group, 1);
            checked(other, true);
            checked(other_keep, true);

            click("change-row-unstaged-checks/match-main.txt", 1);
            key("f", "f", 3, NSEventModifierFlags::Command);
            for (ch, code) in [("m", 46), ("a", 0), ("t", 17), ("c", 8), ("h", 4)] {
                key(ch, ch, code, NSEventModifierFlags::empty());
            }
            expect("changes-filter-input", "match");
            clean(other_keep);
            click(&other_group, 1);
            checked(other, false);
            save_window_png(&window, &fixture.join("commit-filtered.png"));
            key("\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
            clean("changes-filter-input");
            checked(other_keep, true);
            checked(other, false);

            click(&first_directory, 1);
            checked(first, false);
            checked(first_keep, false);
            checked(staged, true);
            click(first, 1);
            click(staged, 1);
            click(other_keep, 1);
            checked(first, true);
            checked(staged, false);
            checked(other_keep, false);

            let other_before = git_run(&other_repo, &["rev-parse", "HEAD"]);
            click("git-commit-message", 1);
            key("v", "v", 9, NSEventModifierFlags::empty());
            expect("git-commit-message", "v");
            click("git-commit-submit", 1);
            wait_until("selected file was committed", Duration::from_secs(8), || {
                git_run(&repo, &["log", "-1", "--format=%s"]) == "v"
            });
            assert_eq!(git_run(&repo, &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]), "checks/match-main.txt");
            assert_eq!(git_run(&repo, &["diff", "--cached", "--name-only"]), "checks/staged-main.txt");
            assert_eq!(git_run(&other_repo, &["rev-parse", "HEAD"]), other_before);
            check_window();
            window.close();
            assert!(!runtime_panicked.load(std::sync::atomic::Ordering::Relaxed));
            println!("NATIVE_CRISP_CHANGES PASS display={number} changes_visible_ms={changes_visible_ms} evidence={}", fixture.display());
            return;
        }

        if startup_repo.is_some() {
            wait_until("project switcher is rendered", Duration::from_secs(12), || {
                view_with_id(&root, "top-workspace-button").is_some()
            });
            key("1", "1", 18, NSEventModifierFlags::Command);
            wait_until("README.md is visible in the Project tree", Duration::from_secs(20), || {
                let row = view_with_id(&root, "project-file-README.md");
                let tree = view_with_id(&root, "project-tree-scroll");
                row.zip(tree).is_some_and(|(row, tree)| {
                    let area = NSIntersectionRect(row.convertRect_toView(row.visibleRect(), None), tree.convertRect_toView(tree.bounds(), None));
                    area.size.width > 0.0 && area.size.height > 0.0
                })
            });
            let project_visible_ms = startup.elapsed().as_millis();
            check_window();
            window.close();
            assert!(!runtime_panicked.load(std::sync::atomic::Ordering::Relaxed));
            println!("NATIVE_CRISP_STARTUP PASS display={number} project_visible_ms={project_visible_ms}");
            return;
        }

        if menus {
            wait_until("workspace renders a change row", Duration::from_secs(8), || {
                view_with_id(&root, "change-row-unstaged-history.txt").is_some()
            });
            let menu = |title: &str| {
                app.mainMenu().unwrap().itemArray().iter()
                    .find(|item| item.title().to_string() == title)
                    .and_then(|item| item.submenu())
                    .unwrap_or_else(|| panic!("missing native {title} menu"))
            };
            let menu_item = |menu: &platform::objc2_app_kit::NSMenu, title: &str| {
                menu.itemArray().iter().find(|item| item.title().to_string() == title)
                    .unwrap_or_else(|| panic!("missing native menu item {title}"))
            };
            let invoke = |menu: &platform::objc2_app_kit::NSMenu, title: &str| {
                check_window();
                let item = menu_item(menu, title);
                assert!(item.tag() > 0 && item.target().is_some(), "{title} must have an application callback");
                assert!(unsafe { app.sendAction_to_from(item.action().unwrap(), item.target().as_deref(), Some(&*item)) });
            };
            if empty_recent {
                let item = menu_item(&menu("文件"), "最近项目");
                assert!(item.submenu().is_none() && item.target().is_none());
                check_window();
                window.close();
                println!("NATIVE_CRISP_EMPTY_RECENTS PASS display={number}");
                return;
            }
            key("1", "1", 18, NSEventModifierFlags::Command);
            wait_until("Project renders a real file row", Duration::from_secs(8), || {
                view_with_id(&root, "project-file-history.txt").is_some_and(|row| {
                    let visible = row.visibleRect();
                    visible.size.width > 0.0 && visible.size.height > 0.0
                })
            });
            let project_visible_ms = startup.elapsed().as_millis();
            let opened = Instant::now();
            click("top-workspace-button", 1);
            wait_until("project menu renders recent entries", Duration::from_secs(4), || {
                a11y_contains(&root, "最近项目")
            });
            let popover_ms = opened.elapsed().as_millis();
            save_window_png(&window, &fixture.join("project-menu.png"));
            key("\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
            wait_until("Escape closes the project menu", Duration::from_secs(4), || {
                !a11y_contains(&root, "最近项目")
            });
            click("top-workspace-button", 1);
            key("\u{f701}", "\u{f701}", 125, NSEventModifierFlags::Function);
            key("p", "p", 35, NSEventModifierFlags::empty());
            wait_until("typing the project name focuses that row", Duration::from_secs(4), || {
                window.firstResponder().and_then(|view| view.downcast::<NSView>().ok())
                    .and_then(|view| view.accessibilityLabel())
                    .is_some_and(|label| label.to_string() == "project-with-a-long-name")
            });
            save_window_png(&window, &fixture.join("project-menu-keyboard.png"));
            key("\r", "\r", 36, NSEventModifierFlags::empty());
            expect("git-text-prompt", "project-with-a-long-name");
            key("\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
            clean("git-text-prompt");

            let file = menu("文件");
            let git = menu("Git");
            let titles = |menu: &platform::objc2_app_kit::NSMenu| {
                menu.itemArray().iter().map(|item| item.title().to_string()).collect::<Vec<_>>()
            };
            let file_titles = titles(&file);
            let git_titles = titles(&git);
            for title in ["新建", "最近项目", "保存全部", "关闭项目"] {
                assert!(file_titles.iter().any(|item| item == title), "missing File > {title}");
            }
            for title in ["提交…", "显示 Git 日志", "分支…", "推送…"] {
                assert!(git_titles.iter().any(|item| item == title), "missing Git > {title}");
            }
            invoke(&git, "显示 Git 日志");
            wait_until("native Git menu displays the Log panel", Duration::from_secs(4), || {
                view_with_id(&root, "git-bottom-pane").is_some()
            });
            wait_until("native Git menu opens commit history", Duration::from_secs(6), || {
                view_with_id(&root, &format!("git-commit-row-{second_commit}")).is_some()
            });
            key("9", "9", 25, NSEventModifierFlags::Command);
            click("project-file-history.txt", 2);
            expect("code-editor-view", "baseline 001");
            click("code-editor-view", 1);
            wait_until("document has the keyboard focus", Duration::from_secs(4), || editor_has_responder("code-editor-view"));
            key("v", "v", 9, NSEventModifierFlags::empty());
            wait_until("document shows unsaved edits", Duration::from_secs(4), || view_with_id(&root, "code-dirty-badge").is_some());
            invoke(&menu("文件"), "保存全部");
            clean("code-dirty-badge");
            assert_ne!(fs::read_to_string(&path).unwrap(), modified);
            assert!(view_with_id(&root, "toast-notification").is_none());
            fs::write(&path, "Crisp File reload fixture\n").unwrap();
            invoke(&menu("文件"), "从磁盘重新加载全部");
            expect("code-editor-view", "Crisp File reload fixture");
            let history = menu_item(&menu("文件"), "本地历史").submenu().unwrap();
            invoke(&history, "显示历史…");
            expect("local-history-modal", "history.txt");
            click("local-history-close", 1);
            clean("local-history-modal");

            let recent = menu_item(&menu("文件"), "最近项目").submenu().unwrap();
            invoke(&recent, "project-with-a-long-name");
            expect("git-text-prompt", "project-with-a-long-name");
            click("git-text-prompt-submit", 1);
            expect("top-workspace-button", "project-with-a-long-name");
            expect("project-file-README.md", "README.md");
            let recent = menu_item(&menu("文件"), "最近项目").submenu().unwrap();
            assert!(titles(&recent).iter().any(|item| item == "repo"), "the startup project must stay in history after switching");

            invoke(&menu("文件"), "关闭项目");
            expect("top-workspace-button", "选择项目");
            check_window();
            window.close();
            assert!(!runtime_panicked.load(std::sync::atomic::Ordering::Relaxed));
            println!("NATIVE_CRISP_MENUS PASS display={number} project_visible_ms={project_visible_ms} popover_ms={popover_ms} file={file_titles:?} git={git_titles:?} evidence={}", fixture.display());
            return;
        }

        if ux {
            wait_until("workspace renders a change row", Duration::from_secs(8), || {
                view_with_id(&root, "change-row-unstaged-history.txt").is_some()
            });
            let change_tree_ms = startup.elapsed().as_millis();
            check_window();
            key("1", "1", 18, NSEventModifierFlags::Command);
            wait_until("Project renders a real file row", Duration::from_secs(8), || {
                view_with_id(&root, "project-file-history.txt").is_some()
            });
            let project_visible_ms = startup.elapsed().as_millis();
            assert!(view_with_id(&root, "project-filter-input").is_none());
            key("f", "f", 3, NSEventModifierFlags::Command);
            wait_until("Project filter opens", Duration::from_secs(4), || {
                view_with_id(&root, "project-filter-input").is_some()
            });
            key("\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
            clean("project-filter-input");
            click("project-file-history.txt", 2);
            expect("code-editor-view", "baseline 001");
            key("f", "f", 3, NSEventModifierFlags::Command);
            wait_until("source search bar opens", Duration::from_secs(4), || {
                view_with_id(&root, "code-find-bar").is_some()
            });
            key("\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
            clean("code-find-bar");
            save_window_png(&window, &fixture.join("source-tabs.png"));
            let fonts = || nemorous::config::load_from(&config).unwrap();
            let code = view_with_id(&root, "code-editor-view").unwrap();
            pinch(mtm, &input_surface, center_in_window(&code));
            wait_until("source pinch saves its area only", Duration::from_secs(4), || {
                let cfg = fonts();
                cfg.editor_font_size == 17.0 && cfg.diff_font_size == 14.0
            });
            check_window();

            key("F", "f", 3, NSEventModifierFlags::Command | NSEventModifierFlags::Shift);
            wait_until("workspace search input", Duration::from_secs(4), || {
                view_with_id(&root, "content-search-input").is_some()
            });
            for (character, code) in [("a", 0), ("t", 17), ("t", 17), ("r", 15), ("i", 34), ("b", 11), ("u", 32), ("t", 17), ("i", 34), ("o", 31), ("n", 45)] {
                key(character, character, code, NSEventModifierFlags::empty());
            }
            let query = Instant::now();
            expect("content-search-input", "attribution");
            assert!(!a11y_contains(&view_with_id(&root, "code-editor-view").unwrap(), "abaseline"));
            wait_until("workspace search result", Duration::from_secs(6), || {
                view_with_id(&root, "content-search-hit-0").is_some()
            });
            let result_ms = query.elapsed().as_millis();
            expect("search-preview-editor", "attribution");
            expect("search-preview-editor", "中文示例");
            let preview_ms = query.elapsed().as_millis().saturating_sub(result_ms);
            save_window_png(&window, &fixture.join("search-preview.png"));
            click("content-search-pin", 1);
            wait_until("pinned search result", Duration::from_secs(4), || {
                view_with_id(&root, "find-results-hit-0").is_some()
            });
            click("find-results-hit-0", 1);
            expect("search-preview-editor", "attribution");
            assert!(view_with_id(&root, "editor-tab-file-history.txt").is_some());
            save_window_png(&window, &fixture.join("pinned-search.png"));
            click("find-results-hit-0", 2);
            expect("code-editor-view", "attribution");
            click("find-results-hide", 1);

            let open_history = || {
                choose_context_item(&window, &view_with_id(&root, "code-editor-view").unwrap(), "显示历史…");
                wait_until("local history opens", Duration::from_secs(4), || {
                    view_with_id(&root, "local-history-modal").is_some()
                });
            };
            open_history();
            expect("local-history-empty", "暂无快照");
            assert!(view_with_id(&root, "local-history-compare").is_none());
            assert!(view_with_id(&root, "local-history-current").is_none());
            save_window_png(&window, &fixture.join("history-empty.png"));
            click("local-history-close", 1);
            clean("local-history-modal");

            let original_frame = window.frame();
            let mut narrow = original_frame;
            narrow.size.width = 940.0;
            narrow.size.height = 640.0;
            window.setFrame_display(narrow, true);
            check_window();
            click("project-file-history.txt", 2);
            expect("code-editor-view", "baseline 001");
            open_history();
            expect("local-history-compare", "查看快照");
            expect("local-history-current", "baseline");
            save_window_png(&window, &fixture.join("history-controls-narrow.png"));
            click("local-history-close", 1);
            clean("local-history-modal");

            click("rail-settings-btn", 1);
            click("settings-keymap", 1);
            expect("keymap-close", "关闭");
            save_window_png(&window, &fixture.join("keymap-controls-narrow.png"));
            key("\u{1b}", "\u{1b}", 53, NSEventModifierFlags::empty());
            window.setFrame_display(original_frame, true);
            check_window();

            key("0", "0", 29, NSEventModifierFlags::Command);
            click("change-row-staged-history.txt", 2);
            expect("working-diff-new-editor", "CRISP_STAGED_CHANGE");
            expect("diff-old-pane-bar", "HEAD");
            click("change-row-unstaged-history.txt", 2);
            expect("working-diff-new-editor", "CRISP_FIRST_CHANGE");
            assert!(view_with_id(&root, "diff-header-path").is_none());
            assert!(view_with_id(&root, "diff-details").is_some());
            for control in ["diff-scope-staged", "diff-scope-unstaged", "diff-save-btn"] {
                assert!(view_with_id(&root, control).is_none(), "{control} must be absent");
            }
            assert!(view_with_id(&root, "diff-dirty-badge").is_none());
            let diff = view_with_id(&root, "working-diff-new-editor").unwrap();
            pinch(mtm, &input_surface, center_in_window(&diff));
            wait_until("Diff pinch saves its area only", Duration::from_secs(4), || {
                let cfg = fonts();
                cfg.editor_font_size == 17.0 && cfg.diff_font_size == 17.0
            });
            save_window_png(&window, &fixture.join("compact-diff.png"));
            click("working-diff-new-editor", 1);
            wait_until("working Diff has keyboard focus", Duration::from_secs(4), || {
                editor_has_responder("working-diff-new-editor")
            });
            key("x", "x", 7, NSEventModifierFlags::empty());
            wait_until("Diff tab marks the draft", Duration::from_secs(4), || {
                view_with_id(&root, "diff-dirty-badge").is_some()
            });
            key("s", "s", 1, NSEventModifierFlags::Command);
            clean("diff-dirty-badge");
            assert!(view_with_id(&root, "toast-notification").is_none());
            assert_ne!(fs::read_to_string(&path).unwrap(), modified);
            assert!(git_run(&repo, &["show", ":history.txt"]).contains("CRISP_STAGED_CHANGE"));
            check_window();
            window.close();
            assert!(!runtime_panicked.load(std::sync::atomic::Ordering::Relaxed));
            println!("NATIVE_CRISP_UX PASS display={number} change_tree_ms={change_tree_ms} project_visible_ms={project_visible_ms} result_ms={result_ms} preview_after_result_ms={preview_ms} evidence={}", fixture.display());
            return;
        }

        if let Some(width) = tool_width {
            let hover = |id: &str| {
                wait_until(id, Duration::from_secs(6), || view_with_id(&root, id).is_some());
                check_window();
                move_pointer(&window, &input_surface, center_in_window(&view_with_id(&root, id).unwrap()));
                pump(120);
                check_window();
            };
            let command = |label: &str, code: u16| key(label, label, code, NSEventModifierFlags::Command);
            verify_left_tool_windows(&window, &root, &fixture, width, |id| click(id, 1), hover, command);
            fs::write(repo.join(CRISP_TOOL_LONG_NAME), "long filename\n").unwrap();
            command("0", 29);
            click("changes-refresh-btn", 1);
            check_crisp_tool_scroll(&window, &root, &input_surface, &fixture,
                "changes-tree-scroll", "changes-header-title-row", &format!("change-row-unstaged-{CRISP_TOOL_LONG_NAME}"));
            check_window();
            command("1", 18);
            check_crisp_tool_scroll(&window, &root, &input_surface, &fixture,
                "project-tree-scroll", "project-header-title-row", &format!("project-file-{CRISP_TOOL_LONG_NAME}"));
            check_window();
            window.close();
            assert!(!app.isActive());
            assert!(!runtime_panicked.load(std::sync::atomic::Ordering::Relaxed), "Crisp runtime panicked");
            println!("NATIVE_CRISP_BACKGROUND_LEFT_TOOLS PASS display={number} width={width} offscreen={offscreen_diagnostics}");
            return;
        }

        click("change-row-unstaged-attribution.txt", 2);
        check_window();
        check_crisp_diff_attribution(&window, &root, &input_surface, &fixture, false);
        check_window();
        println!("NATIVE_CRISP_BACKGROUND_DIFF_ATTRIBUTION PASS display={number} linked={linked}");

        // Open the commit in unified mode so no split viewport has to be
        // mapped to the selected revision's first added row.
        click("diff-layout-toggle", 1);
        key("9", "9", 25, NSEventModifierFlags::Command);
        click(&format!("git-commit-row-{second_commit}"), 1);
        click(&format!("git-commit-file-{committed_file}"), 2);
        expect("commit-diff-unified-editor", "- Parent revision line 001");
        check_crisp_diff_attribution(&window, &root, &input_surface, &fixture, true);
        for (hash, date) in [(&first_commit, CRISP_FIRST_AUTHOR_DATE), (&second_commit, CRISP_SECOND_AUTHOR_DATE)] {
            let local_date = chrono::DateTime::parse_from_rfc3339(date).unwrap()
                .with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string();
            expect(&format!("git-commit-row-{hash}"), &local_date);
        }
        check_window();
        println!("NATIVE_CRISP_BACKGROUND_COMMIT_ATTRIBUTION PASS display={number} linked={linked}");
        click("diff-layout-toggle", 1);
        key("9", "9", 25, NSEventModifierFlags::Command);
        clean("git-log-title");

        click("change-row-unstaged-history.txt", 2);
        // Both files use the same editor IDs while an async Diff read is pending.
        expect("working-diff-new-editor", "CRISP_FIRST_CHANGE");
        if linked {
            let command = NSEventModifierFlags::Command;
            let diff = "working-diff-new-editor";
            click(diff, 1);
            wait_until("Crisp history Diff has its editor responder", Duration::from_secs(6), || editor_has_responder(diff));
            key("x", "x", 7, NSEventModifierFlags::empty());
            wait_until("initial Crisp Diff edit", Duration::from_secs(6), || view_with_id(&root, "diff-dirty-badge").is_some());
            key("s", "s", 1, command);
            clean("diff-dirty-badge");
            assert_ne!(fs::read_to_string(&path).unwrap(), modified);
            click(diff, 1);
            key("z", "z", 6, command);
            wait_until("Crisp undo after saving", Duration::from_secs(6), || view_with_id(&root, "diff-dirty-badge").is_some());
            key("s", "s", 1, command);
            clean("diff-dirty-badge");
            assert_eq!(fs::read_to_string(&path).unwrap(), modified);

            for diff in [diff, "working-diff-unified-editor"] {
                if diff == "working-diff-unified-editor" {
                    click("diff-layout-toggle", 1);
                    // The old split editor is gone. Allow a full caret blink
                    // period so a timer left behind fails this regression.
                    pump(650);
                }
                for attempt in 1..=3 {
                    click(diff, 1);
                    key("x", "x", 7, NSEventModifierFlags::empty());
                    wait_until("Crisp Diff draft", Duration::from_secs(6), || view_with_id(&root, "diff-dirty-badge").is_some());
                    key("\u{F707}", "\u{F707}", 118, NSEventModifierFlags::Function);
                    wait_until("Crisp source has the Diff draft", Duration::from_secs(6), || view_with_id(&root, "code-dirty-badge").is_some());
                    key("z", "z", 6, command);
                    clean("code-dirty-badge");
                    key("[", "[", 33, command);
                    wait_until("Crisp Diff ready for redo", Duration::from_secs(6), || editor_has_responder(diff));
                    key("Z", "z", 6, command | NSEventModifierFlags::Shift);
                    wait_until(&format!("Crisp {diff} redo {attempt}"), Duration::from_secs(6), || view_with_id(&root, "diff-dirty-badge").is_some());
                    key("z", "z", 6, command);
                    clean("diff-dirty-badge");
                    assert_eq!(fs::read_to_string(&path).unwrap(), modified);
                }
            }
            println!("NATIVE_CRISP_BACKGROUND_DIFF_REDO PASS display={number} linked={linked}");
        }

        click("diff-jump-source-btn", 1);
        choose_context_item(&window, &view_with_id(&root, "code-editor-view").unwrap(), "显示历史…");
        click(&snapshot_row, 1);
        expect("local-history-current", "CRISP_FIRST_CHANGE");
        expect("local-history-change-count", "1 / 2");
        click("local-history-hunk-next", 1);
        expect("local-history-current", "CRISP_LAST_CHANGE");
        expect("local-history-change-count", "2 / 2");
        click("local-history-restore-hunk", 1);
        expect("local-history-modal", "已在编辑器恢复此变更块");
        expect("local-history-change-count", "1 / 1");
        assert_eq!(fs::read_to_string(&path).unwrap(), modified);
        save_window_png(&window, &fixture.join("local-history.png"));
        println!("NATIVE_CRISP_BACKGROUND_HISTORY PASS display={number}");

        click("local-history-close", 1);
        clean("local-history-modal");
        let compact = NSSize::new(640.0, 500.0);
        window.setContentSize(compact);
        wait_until("compact Git window size", Duration::from_secs(6), || root.bounds().size == compact);
        check_window();
        key("9", "9", 25, NSEventModifierFlags::Command);
        let row_id = format!("git-commit-row-{second_commit}");
        expect(&row_id, CRISP_SECOND_SUBJECT);
        save_window_png(&window, &fixture.join("git-log-compact-before-hover.png"));
        let row = view_with_id(&root, &row_id).unwrap();
        let subject = view_with_id(&row, "git-commit-subject").expect("commit subject");
        let list = view_with_id(&root, "git-commit-list-scroll").expect("commit list");
        let bounds = |view: &NSView| view.convertRect_toView(view.bounds(), None);
        wait_until("compact commit list fits the resized window", Duration::from_secs(6), || {
            let area = bounds(&list);
            area.origin.x >= 0.0 && area.origin.x + area.size.width <= compact.width
                && area.origin.y >= 0.0 && area.origin.y + area.size.height <= compact.height
        });
        let title = view_with_id(&root, "git-log-title").expect("Git tool window title");
        move_pointer(&window, &input_surface, center_in_window(&title));
        pump(30);
        clean("toolbar-tooltip");
        let visible = NSIntersectionRect(subject.bounds(), subject.visibleRect());
        let hit = NSIntersectionRect(subject.convertRect_toView(visible, None), bounds(&list));
        assert!(hit.size.width > 0.0 && hit.size.height > 0.0, "commit subject is clipped: {hit:?}");
        let point = NSPoint::new(
            hit.origin.x + hit.size.width * 0.5,
            hit.origin.y + hit.size.height * 0.5,
        );
        move_pointer(&window, &input_surface, point);
        let tooltip_matches = || view_with_id(&root, "toolbar-tooltip").is_some_and(|tip| {
            a11y_contains(&tip, CRISP_SECOND_SUBJECT) && a11y_contains(&tip, &second_commit)
        });
        let deadline = Instant::now() + Duration::from_secs(8);
        while !tooltip_matches() && Instant::now() < deadline {
            pump(15);
        }
        if !tooltip_matches() {
            let tip = view_with_id(&root, "toolbar-tooltip").map(|tip| view_a11y_text(&tip));
            save_window_png(&window, &fixture.join("git-log-compact-hover-timeout.png"));
            panic!("compact Git subject tooltip missing: point={point:?}, subject={hit:?}, tooltip={tip:?}");
        }
        check_window();
        save_window_png(&window, &fixture.join("git-log-commit-subject-tooltip.png"));
        for (id, label) in [
            ("git-log-view-options", "提交列表视图选项"),
            ("git-file-view-options", "变更文件视图选项"),
        ] {
            wait_until(id, Duration::from_secs(6), || view_with_id(&root, id).is_some());
            let target = view_with_id(&root, id).unwrap();
            move_pointer(&window, &input_surface, center_in_window(&target));
            expect("toolbar-tooltip", label);
        }
        save_window_png(&window, &fixture.join("git-log-small-window.png"));
        check_window();
        println!("NATIVE_CRISP_BACKGROUND_SMALL_GIT_LOG PASS display={number} linked={linked}");
        window.close();
        assert!(!app.isActive());
        assert!(!runtime_panicked.load(std::sync::atomic::Ordering::Relaxed), "Crisp runtime panicked");
    }

    pub fn run() {
        let _ = env_logger::try_init();
        let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        if std::env::var_os("NEMOROUS_NATIVE_CRISP").is_some() {
            return run_on_crisp(&root_dir);
        }
        let baseline_repo = root_dir.join("build/migration-handoff/baseline/repo");
        let commit_panel = std::env::var_os("NEMOROUS_NATIVE_COMMIT_PANEL").is_some();
        let git_log = std::env::var_os("NEMOROUS_NATIVE_GIT_LOG").is_some();
        let linked_worktree = git_log && std::env::var_os("NEMOROUS_NATIVE_LINKED_WORKTREE").is_some();
        let left_tools = if commit_panel {
            Some(350.0)
        } else {
            std::env::var("NEMOROUS_NATIVE_LEFT_TOOL_WIDTH").ok()
                .map(|width| width.parse::<f32>().expect("left tool window width"))
        };
        let darcula = std::env::var_os("NEMOROUS_NATIVE_DARCULA").is_some();
        assert!(
            git_log || darcula || left_tools.is_some() || baseline_repo.exists(),
            "baseline repo fixture missing: {}",
            baseline_repo.display()
        );

        let batch2 = std::env::var_os("NEMOROUS_NATIVE_BATCH2").is_some();
        let batch3 = std::env::var_os("NEMOROUS_NATIVE_BATCH3").is_some();
        let batch4 = std::env::var_os("NEMOROUS_NATIVE_BATCH4").is_some();
        let batch5 = std::env::var_os("NEMOROUS_NATIVE_BATCH5").is_some();
        let batch6 = std::env::var_os("NEMOROUS_NATIVE_BATCH6").is_some();
        let cleanup = std::env::var_os("NEMOROUS_NATIVE_REVIEW_CLEANUP").is_some();
        let work_dir = if git_log {
            root_dir.join(format!("build/tmp-ab/native-git-log-{}", std::process::id()))
        } else if darcula {
            root_dir.join(format!("build/tmp-ab/native-darcula-{}", std::process::id()))
        } else if left_tools.is_some() {
            root_dir.join(format!("build/tmp-ab/native-left-tools-{}", std::process::id()))
        } else if cleanup {
            root_dir.join(format!("build/tmp-ab/native-review-cleanup-{}", std::process::id()))
        } else if batch6 {
            root_dir.join(format!("build/tmp-ab/native-batch6-{}", std::process::id()))
        } else if batch5 {
            root_dir.join(format!("build/tmp-ab/native-batch5-{}", std::process::id()))
        } else if batch4 {
            root_dir.join(format!("build/tmp-ab/native-batch4-{}", std::process::id()))
        } else if batch3 {
            root_dir.join(format!("build/tmp-ab/native-batch3-{}", std::process::id()))
        } else if batch2 {
            root_dir.join(format!("build/tmp-ab/native-batch2-{}", std::process::id()))
        } else {
            let dir = root_dir.join("build/tmp-ab/native-macos-ab");
            let _ = fs::remove_dir_all(&dir);
            dir
        };
        let repo_copy = work_dir.join("repo");
        let scope_gate = work_dir.join("diff-scope-gate");
        if git_log {
            setup_scope_read_gate(&scope_gate);
        }
        if linked_worktree {
            let primary = work_dir.join("primary");
            git_log_fixture(&primary);
            git_run(&primary, &["worktree", "add", "-b", "agent/linked", repo_copy.to_str().unwrap(), "HEAD"]);
            for name in ["README.md", "review-lines.txt"] {
                fs::copy(primary.join(name), repo_copy.join(name)).unwrap();
            }
        } else if git_log {
            git_log_fixture(&repo_copy);
        } else if darcula {
            let base = r#"package demo;

/**
 * Retry settings for queued messages.
 * @since 1.0
 */
@Deprecated
public final class RetryPolicy implements java.io.Serializable {
    private static final long serialVersionUID = 1L;
    private static final int MAX_ATTEMPTS = 3;
    private static final int DELAY_MS = 100;

    // Retry delay grows with the attempt number.
    public int delay(int attempt) {
        return DELAY_MS * (attempt + 1);
    }

    @Override
    public String toString() {
        return "retry";
    }
}
"#;
            let retry_method = concat!(
                "    public boolean shouldRetry(int attempt) {\n",
                "        return attempt < MAX_ATTEMPTS;\n",
                "    }\n\n",
                "    @Override",
            );
            let working = base
                .replace("MAX_ATTEMPTS = 3", "MAX_ATTEMPTS = 5")
                .replace("DELAY_MS = 100", "DELAY_MS = 250")
                .replace("    // Retry delay grows with the attempt number.\n", "")
                .replace("    @Override", retry_method);
            fs::create_dir_all(repo_copy.join("src")).unwrap();
            let file = repo_copy.join("src/RetryPolicy.java");
            fs::write(&file, base).unwrap();
            fs::write(repo_copy.join("README.md"), "# Retry policy\n").unwrap();
            git_run(&repo_copy, &["init", "-b", "main"]);
            git_run(&repo_copy, &["add", "."]);
            git_run(
                &repo_copy,
                &[
                    "-c",
                    "user.name=Nemorous QA",
                    "-c",
                    "user.email=native@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    "Baseline retry policy",
                ],
            );
            fs::write(file, working).unwrap();
        } else if left_tools.is_some() {
            write_left_tool_files(&repo_copy);
            git_run(&repo_copy, &["init", "-b", "main"]);
            git_run(&repo_copy, &["add", "."]);
            git_run(&repo_copy, &["-c", "user.name=Nemorous QA", "-c", "user.email=native@example.invalid",
                "-c", "commit.gpgsign=false", "commit", "-m", "Left tool window fixture"]);
            fs::write(repo_copy.join("src/nested/tracked.txt"), "first\nworking edit\nlast\n").unwrap();
            if commit_panel {
                let long_path = repo_copy.join(LONG_CHANGE_PATH);
                fs::create_dir_all(long_path.parent().unwrap()).unwrap();
                fs::write(long_path, "review: true\n").unwrap();
                for directory in ["config/development", "config/production"] {
                    let path = repo_copy.join(directory);
                    fs::create_dir_all(&path).unwrap();
                    fs::write(path.join("application.yml"), "fixture: true\n").unwrap();
                }
                for i in 0..28 {
                    fs::write(repo_copy.join(format!("docs/change-{i:02}.txt")), "changed\n").unwrap();
                }
            }
        } else {
            copy_dir_all(&baseline_repo, &repo_copy);
        }
        if batch3 {
            let context = (1..20).map(|line| format!("context {line}\n")).collect::<String>();
            let file = repo_copy.join("batch3-ticket.txt");
            fs::write(&file, format!("original first\n{context}original last\n")).unwrap();
            git_run(&repo_copy, &["add", "batch3-ticket.txt"]);
            git_run(&repo_copy, &["-c", "user.email=native@example.invalid", "-c", "user.name=Nemorous QA", "-c", "commit.gpgsign=false", "commit", "-m", "Batch 3 fixture"]);
            git_run(&repo_copy, &["branch", "topic"]);
            fs::write(&file, format!("edited first\n{context}edited last\n")).unwrap();
        }

        if batch4 {
            git_run(&repo_copy, &["config", "user.email", "native@example.invalid"]);
            git_run(&repo_copy, &["config", "user.name", "Nemorous QA"]);
            git_run(&repo_copy, &["config", "commit.gpgsign", "false"]);
            let file = repo_copy.join("conflict.txt");
            fs::write(&file, "base line\n").unwrap();
            git_run(&repo_copy, &["add", "conflict.txt"]);
            git_run(&repo_copy, &["commit", "-m", "conflict fixture base"]);
            git_run(&repo_copy, &["switch", "-c", "topic"]);
            fs::write(&file, "source line\n").unwrap();
            git_run(&repo_copy, &["add", "conflict.txt"]);
            git_run(&repo_copy, &["commit", "-m", "source"]);
            git_run(&repo_copy, &["switch", "main"]);
            fs::write(&file, "target line\n").unwrap();
            git_run(&repo_copy, &["add", "conflict.txt"]);
            git_run(&repo_copy, &["commit", "-m", "target"]);
            let conflict = Command::new("git").current_dir(&repo_copy)
                .args(["-c", "merge.conflictStyle=diff3", "merge", "--no-edit", "topic"])
                .output().unwrap();
            assert!(!conflict.status.success());
            assert!(repo_copy.join(".git/MERGE_HEAD").exists());
            copy_dir_all(&repo_copy, &work_dir.join("conflict-initial"));
        }

        let cfg_path = work_dir.join("config-new.json");
        if batch5 || batch6 {
            copy_dir_all(&repo_copy, &work_dir.join("initial-repo"));
        }
        let mut cfg_json = serde_json::json!({
            "workspace_path": repo_copy.to_string_lossy(),
            "theme": "light",
            "change_diff_preview": false,
            "editor_font_size": 14.0
        });
        if darcula {
            cfg_json["theme"] = "darcula".into();
            cfg_json["commit_left_width"] = 290.0.into();
            cfg_json["code_left_width"] = 290.0.into();
            cfg_json["auto_update"] = false.into();
            cfg_json["diff_side_by_side"] = true.into();
        }
        if let Some(width) = left_tools {
            cfg_json["commit_left_width"] = width.into();
            cfg_json["code_left_width"] = width.into();
            cfg_json["auto_update"] = false.into();
        }
        if commit_panel {
            cfg_json["theme"] = "darcula".into();
            cfg_json["changes_flat"] = true.into();
        }
        if git_log {
            cfg_json["theme"] = "darcula".into();
            cfg_json["auto_update"] = false.into();
            cfg_json["git_panel_height"] = 330.0.into();
            cfg_json["editor_soft_wrap"] = false.into();
        }
        if batch6 {
            cfg_json["auto_update"] = false.into();
            cfg_json["app_icon"] = "aero".into();
            cfg_json["zoom"] = 1.0.into();
            cfg_json["diff_side_by_side"] = true.into();
        }
        if batch5 {
            let desktop_path = std::env::var_os("NEMOROUS_NATIVE_DESKTOP_PATH").is_some();
            let (program, args) = if desktop_path {
                ("rg", "--version")
            } else {
                ("/bin/sh", "-c 'printf NATIVE_RUN_OUTPUT; exit 7'")
            };
            cfg_json["run_configs"] = serde_json::json!([{
                "id": "native-run", "name": "Native Run fixture", "kind": "run", "command": program,
                "args": args
            }]);
        }
        fs::write(&cfg_path, serde_json::to_string_pretty(&cfg_json).unwrap()).unwrap();

        unsafe {
            std::env::set_var("NEMOROUS_CONFIG", &cfg_path);
            std::env::set_var("NEMOROUS_WORKSPACE", &repo_copy);
        }

        let mtm = MainThreadMarker::new().expect("must run on main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.finishLaunching();
        let _gesture_monitor = nemorous::macos_gestures::install().expect("local gesture monitor");

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(80.0, 80.0), NSSize::new(1440.0, 900.0)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(&NSString::from_str("Nemorous"));

        let mut options = platform::AppOptions::default();
        options.title = "Nemorous".to_string();
        options.size = (1440, 900);
        let surface = platform::AppView::new(mtm, options);
        window.setContentView(Some(&surface.view()));
        surface.load(platform::App::Rust(platform::Root::of::<nemorous::App>()));
        window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        // Drain initial AppKit launch/activation events and wait briefly for foreground key status
        let act_deadline = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < act_deadline {
            if let Some(ev) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                platform::objc2_app_kit::NSEventMask::Any,
                Some(&NSDate::dateWithTimeIntervalSinceNow(0.02)),
                unsafe { platform::objc2_foundation::NSDefaultRunLoopMode },
                true,
            ) {
                app.sendEvent(&ev);
            }
            if window.isKeyWindow() && app.isActive() {
                break;
            }
            window.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }

        let root = surface.view();
        if git_log {
            let evidence = if linked_worktree { "build/linked-worktree-review" } else { "build/git-log-review" };
            git_log_review(&app, &window, &root, &repo_copy, &root_dir.join(evidence), &scope_gate);
            window.close();
            pump(150);
            return;
        }
        if commit_panel {
            commit_panel_review(&app, &window, &root, &cfg_path, &root_dir.join("build/commit-panel-review"));
            window.close();
            pump(150);
            return;
        }
        if darcula {
            let evidence = root_dir.join("build/darcula-review");
            darcula_review(&app, &window, &root, &cfg_path, &evidence);
            window.close();
            pump(150);
            return;
        }
        if let Some(width) = left_tools {
            if width < 150.0 {
                let narrow = NSRect::new(NSPoint::new(80.0, 80.0), NSSize::new(1040.0, 660.0));
                window.setFrame_display(window.frameRectForContentRect(narrow), true);
            }
            let evidence = root_dir.join("build/left-tool-window-evidence");
            left_tool_windows(&app, &window, &root, &evidence, width);
            window.close();
            pump(150);
            return;
        }
        if cleanup {
            let evidence = root_dir.join("build/review-cleanup-evidence");
            review_cleanup(&app, &window, &root, &evidence);
            window.close();
            pump(150);
            return;
        }
        if batch6 {
            let evidence = root_dir.join("build/migration-evidence/batch-6");
            batch6_settings(&app,&window,&root,&repo_copy,&evidence);
            window.close();
            pump(150);
            return;
        }
        if batch5 {
            let evidence = root_dir.join("build/migration-evidence/batch-5");
            batch5_workbench(&app, &window, &root, &repo_copy, &evidence);
            window.close();
            pump(200);
            return;
        }
        if batch4 {
            let evidence = root_dir.join("build/migration-evidence/batch-4");
            batch4_resolver(&app, &window, &root, &repo_copy, &evidence);
            window.close();
            pump(150);
            return;
        }
        if batch3 {
            let evidence = root_dir.join("build/migration-evidence/batch-3");
            batch3_stage_and_branch(&app, &window, &root, &repo_copy, &evidence);
            window.close();
            pump(150);
            println!("NATIVE_BATCH3_GIT PASS");
            return;
        }
        if batch2 {
            wait_until("workspace file loaded", Duration::from_secs(6), || {
                view_with_id(&root, "change-row-unstaged-tracked.txt").is_some()
            });
            let evidence = root_dir.join("build/migration-evidence/batch-2");
            batch2_search_and_replace(&app, &window, &root, &repo_copy, &evidence);
            batch2_project_and_keyboard(&app, &window, &root, &evidence);
            window.close();
            pump(150);
            println!("NATIVE_BATCH2_SEARCH_REPLACE PASS");
            return;
        }
        let pb_usable = probe_pasteboard_usable();
        println!(
            "WINDOW_STATE visible={} key={} main={} active={} pasteboard_usable={}",
            window.isVisible(),
            window.isKeyWindow(),
            window.isMainWindow(),
            app.isActive(),
            pb_usable
        );

        // 1. 等待同仓基线副本加载完成（tracked.txt, agent-change.txt, new.txt）
        wait_until("tracked.txt in Commit tree", Duration::from_secs(6), || {
            view_with_id(&root, "change-row-unstaged-tracked.txt").is_some()
        });
        assert!(view_with_id(&root, "change-row-unstaged-agent-change.txt").is_some());
        assert!(view_with_id(&root, "change-row-unstaged-new.txt").is_some());
        println!("STEP1_BASELINE_LOAD ok (tracked.txt, agent-change.txt, new.txt)");

        // 2. 模拟外部 Agent 写盘，验证真实 FSEvents 监听刷新进入真实 AppKit 视图树
        let t_agent = Instant::now();
        fs::write(
            repo_copy.join("agent-live-ab.txt"),
            "agent live line 1\nagent live line 2\n",
        )
        .unwrap();
        wait_until(
            "agent-live-ab.txt auto-refreshed into native Commit view",
            Duration::from_secs(6),
            || view_with_id(&root, "change-row-unstaged-agent-live-ab.txt").is_some(),
        );
        let agent_refresh_ms = t_agent.elapsed().as_secs_f64() * 1000.0;
        println!("STEP2_AGENT_WATCHER ok elapsed_ms={agent_refresh_ms:.2}");

        // 3. 单击 tracked.txt：只改选不打开 Diff
        let tracked_row = view_with_id(&root, "change-row-unstaged-tracked.txt").unwrap();
        send_click(&window, &tracked_row, 1);
        assert!(
            view_with_id(&root, "working-diff-pane").is_none(),
            "single click with preview=false must NOT open working diff"
        );
        println!("STEP3_SINGLE_CLICK_NO_DIFF ok");

        // 3.5 验证真实 AppKit 原生菜单「编辑 > 复制」焦点感知切换 + Commit 树/输入框剪贴板路由
        let tree_copy_item = find_edit_copy_menu_item(&app).expect("Edit > Copy menu item");
        assert!(
            tree_copy_item.target().is_some(),
            "Edit > Copy in Commit tree must route to app focus handler"
        );
        if pb_usable {
            pasteboard_set("__clear__");
            assert!(
                trigger_edit_copy_menu(&app),
                "Edit > Copy menu action should fire in Commit tree"
            );
            assert_eq!(
                pasteboard_get().as_deref(),
                Some("tracked.txt"),
                "native Edit > Copy on tracked Commit row must copy basename"
            );
            // 未跟踪文件按 ⌘C 复制 display_path
            let new_row_copy = view_with_id(&root, "change-row-unstaged-new.txt").unwrap();
            send_click(&window, &new_row_copy, 1);
            send_key(&app, &window, "c", "c", 8, NSEventModifierFlags::Command);
            let expected_untracked =
                nemorous::paths::display_path(&repo_copy.join("new.txt").to_string_lossy());
            assert_eq!(
                pasteboard_get().as_deref(),
                Some(expected_untracked.as_str()),
                "Cmd+C on untracked Commit row must copy display_path"
            );
        }
        // 点击筛选 <input> 后，原生菜单「编辑 > 复制」必须切换回标准 MenuRole::Copy (sel!(copy:), target=None) 绝不劫持输入框
        send_key(&app, &window, "f", "f", 3, NSEventModifierFlags::Command);
        wait_until("Commit filter opens", Duration::from_secs(4), || {
            view_with_id(&root, "changes-filter-input").is_some()
        });
        let filter_input = view_with_id(&root, "changes-filter-input").unwrap();
        send_click(&window, &filter_input, 1);
        wait_until("Edit > Copy switches to native role copy: on <input> focus", Duration::from_secs(3), || {
            find_edit_copy_menu_item(&app)
                .is_some_and(|it| it.target().is_none() && it.action() == Some(sel!(copy:)))
        });
        // 切回 tracked.txt，恢复树焦点
        let tracked_row_again = view_with_id(&root, "change-row-unstaged-tracked.txt").unwrap();
        send_click(&window, &tracked_row_again, 1);
        wait_until("Edit > Copy restores focus-routed target on Commit tree", Duration::from_secs(3), || {
            find_edit_copy_menu_item(&app).is_some_and(|it| it.target().is_some())
        });
        println!("STEP3_5_NATIVE_MENU_AND_CLIPBOARD_ROUTING ok pb_usable={pb_usable}");

        // 4. 真实 AppKit NSEvent 发送 ⌘D -> 打开 tracked.txt 双栏 Diff
        send_key(&app, &window, "d", "d", 2, NSEventModifierFlags::Command);
        wait_until("working-diff-pane after Cmd+D", Duration::from_secs(4), || {
            view_with_id(&root, "working-diff-pane").is_some()
        });
        println!("STEP4_CMD_D_OPEN_DIFF ok");

        let evidence_dir = root_dir.join("build/migration-evidence/batch-1");
        save_window_png(&window, &evidence_dir.join("new-1440x900-commit-diff.png"));

        // 5. 真实 AppKit NSEvent 发送 F4 -> 跳转到源码编辑器
        send_key(
            &app,
            &window,
            "\u{F707}",
            "\u{F707}",
            118,
            NSEventModifierFlags::Function,
        );
        wait_until("code-editor-pane after F4", Duration::from_secs(4), || {
            view_with_id(&root, "code-editor-pane").is_some()
        });
        println!("STEP5_F4_TO_SOURCE ok");
        save_window_png(&window, &evidence_dir.join("new-1440x900-source-f4.png"));

        // 6. 真实 AppKit NSEvent 发送 ⌘[ -> 返回原工作区 Diff
        send_key(&app, &window, "[", "[", 33, NSEventModifierFlags::Command);
        wait_until("working-diff-pane after Cmd+[", Duration::from_secs(4), || {
            view_with_id(&root, "working-diff-pane").is_some()
        });
        println!("STEP6_CMD_BRACKET_RETURN ok");

        // 7. 真实双击 new.txt -> 切换打开 new.txt Diff，并断言行计数为 +1 / -0（无末尾空行多计）
        let new_row = view_with_id(&root, "change-row-unstaged-new.txt").unwrap();
        send_click(&window, &new_row, 2);
        wait_until("Diff tab switches to new.txt", Duration::from_secs(4), || {
            view_with_id(&root, "working-diff-pane").is_some()
                && view_with_id(&root, "editor-tab-working-diff")
                    .is_some_and(|v| a11y_contains(&v, "new.txt"))
        });
        assert!(view_with_id(&root, "diff-details").is_some());
        println!("STEP7_DOUBLE_CLICK_NEW_TXT ok");

        // 8. 调整真实窗口到 1040×660 验证窄栏布局并截屏
        let narrow_rect = NSRect::new(NSPoint::new(80.0, 80.0), NSSize::new(1040.0, 660.0));
        let frame = window.frameRectForContentRect(narrow_rect);
        window.setFrame_display(frame, true);
        pump(250);
        let title_row = view_with_id(&root, "changes-header-title-row").expect("title row");
        let toolbar_row = view_with_id(&root, "changes-header-toolbar-row").expect("toolbar row");
        let title_f = title_row.convertRect_toView(title_row.bounds(), None);
        let tool_f = toolbar_row.convertRect_toView(toolbar_row.bounds(), None);
        println!(
            "STEP8_NARROW_1040x660 title_y={:.1}..{:.1} toolbar_y={:.1}..{:.1}",
            title_f.origin.y,
            title_f.origin.y + title_f.size.height,
            tool_f.origin.y,
            tool_f.origin.y + tool_f.size.height
        );
        save_window_png(&window, &evidence_dir.join("new-1040x660-narrow.png"));

        // 9. 真实 ⌘W 关闭 Diff，确认 Commit 树行保留
        send_key(&app, &window, "w", "w", 13, NSEventModifierFlags::Command);
        wait_until("working-diff-pane gone after Cmd+W", Duration::from_secs(4), || {
            view_with_id(&root, "working-diff-pane").is_none()
        });
        assert!(view_with_id(&root, "change-row-unstaged-new.txt").is_some());
        println!("STEP9_CMD_W_CLOSE_DIFF ok");

        // 10. 切回 1440×900：验证真实前台窗口轻编辑受检保存 + 外部改盘冲突弹窗不覆写
        let wide_rect = NSRect::new(NSPoint::new(80.0, 80.0), NSSize::new(1440.0, 900.0));
        let wide_frame = window.frameRectForContentRect(wide_rect);
        window.setFrame_display(wide_frame, true);
        pump(200);
        let tracked_row2 = view_with_id(&root, "change-row-unstaged-tracked.txt").unwrap();
        send_click(&window, &tracked_row2, 2);
        wait_until("working-diff-new-editor ready", Duration::from_secs(4), || {
            view_with_id(&root, "working-diff-new-editor").is_some()
        });
        let right_ed = view_with_id(&root, "working-diff-new-editor").unwrap();
        send_click(&window, &right_ed, 1);
        send_key(&app, &window, "Z", "z", 6, NSEventModifierFlags::Shift);
        send_key(&app, &window, "Q", "q", 12, NSEventModifierFlags::Shift);
        wait_until("diff-dirty-badge after native typing", Duration::from_secs(4), || {
            view_with_id(&root, "diff-dirty-badge").is_some()
        });
        send_key(&app, &window, "s", "s", 1, NSEventModifierFlags::Command);
        wait_until("diff-dirty-badge cleared after Cmd+S", Duration::from_secs(4), || {
            view_with_id(&root, "diff-dirty-badge").is_none()
        });
        let saved_disk = fs::read_to_string(repo_copy.join("tracked.txt")).unwrap();
        assert!(
            saved_disk.contains("ZQ"),
            "checked Cmd+S in native window must persist typed text, got: {saved_disk:?}"
        );

        // 再键入一个字符后外部改盘 -> 按 ⌘S 必须弹出冲突弹窗且绝不静默覆盖外部写入
        send_key(&app, &window, "K", "k", 40, NSEventModifierFlags::Shift);
        wait_until("dirty before external overwrite", Duration::from_secs(4), || {
            view_with_id(&root, "diff-dirty-badge").is_some()
        });
        let ext_payload = "external agent overwrite must survive\n";
        fs::write(repo_copy.join("tracked.txt"), ext_payload).unwrap();
        send_key(&app, &window, "s", "s", 1, NSEventModifierFlags::Command);
        wait_until("save-conflict-modal shown on Cmd+S", Duration::from_secs(4), || {
            view_with_id(&root, "save-conflict-modal").is_some()
        });
        assert_eq!(
            fs::read_to_string(repo_copy.join("tracked.txt")).unwrap(),
            ext_payload,
            "external file content must NOT be overwritten on conflict"
        );
        let accept_btn = view_with_id(&root, "conflict-accept-disk-btn").unwrap();
        send_click(&window, &accept_btn, 1);
        wait_until("save-conflict-modal dismissed", Duration::from_secs(4), || {
            view_with_id(&root, "save-conflict-modal").is_none()
        });
        println!("STEP10_NATIVE_EDIT_SAVE_AND_CONFLICT ok");

        // 11. 真实前台窗口验证历史提交重命名双侧只读 Diff（先提交基线多行内容，再 git mv 保持 >80% 相似度确保识别为 R 重命名）
        git_run(&repo_copy, &["config", "user.name", "Native Tester"]);
        git_run(&repo_copy, &["config", "user.email", "native@example.com"]);
        fs::write(
            repo_copy.join("tracked.txt"),
            "alpha line 1\nbeta line 2\ngamma line 3\ndelta line 4\nversion = v1\n",
        )
        .unwrap();
        git_run(&repo_copy, &["add", "tracked.txt"]);
        git_run(&repo_copy, &["commit", "-m", "chore: prepare rename base"]);

        git_run(&repo_copy, &["mv", "tracked.txt", "tracked_renamed.txt"]);
        fs::write(
            repo_copy.join("tracked_renamed.txt"),
            "alpha line 1\nbeta line 2\ngamma line 3\ndelta line 4\nversion = v2\n",
        )
        .unwrap();
        git_run(&repo_copy, &["add", "-A"]);
        git_run(&repo_copy, &["commit", "-m", "refactor: rename tracked in native test"]);
        let head_short = git_run(&repo_copy, &["rev-parse", "--short", "HEAD"]);

        // 触发手动刷新 ⌘⌥Y 与打开底部 Git 面板 ⌘9
        send_key(
            &app,
            &window,
            "y",
            "y",
            16,
            NSEventModifierFlags::Command | NSEventModifierFlags::Option,
        );
        send_key(&app, &window, "9", "9", 25, NSEventModifierFlags::Command);
        let commit_row_id = format!("git-commit-row-{head_short}");
        wait_until("renamed commit row visible in Git panel", Duration::from_secs(6), || {
            view_with_id(&root, &commit_row_id).is_some()
        });
        let commit_row = view_with_id(&root, &commit_row_id).unwrap();
        send_click(&window, &commit_row, 1);
        wait_until("renamed commit file visible", Duration::from_secs(4), || {
            view_with_id(&root, "git-commit-file-tracked_renamed.txt").is_some()
        });
        let commit_file_row = view_with_id(&root, "git-commit-file-tracked_renamed.txt").unwrap();
        send_click(&window, &commit_file_row, 2);
        wait_until("commit-diff-pane opened for rename", Duration::from_secs(4), || {
            view_with_id(&root, "commit-diff-pane").is_some()
        });
        let old_hdr = view_with_id(&root, "diff-old-pane-header")
            .map(|v| view_a11y_text(&v))
            .unwrap_or_default();
        let new_hdr = view_with_id(&root, "diff-new-pane-header")
            .map(|v| view_a11y_text(&v))
            .unwrap_or_default();
        assert!(
            old_hdr.contains("tracked.txt") && new_hdr.contains("tracked_renamed.txt"),
            "rename commit diff headers mismatch: old={old_hdr:?}, new={new_hdr:?}"
        );
        save_window_png(&window, &evidence_dir.join("new-1440x900-commit-rename.png"));

        // STEP 11.5: 真实窗口历史重命名左右双侧验证：⌘L 跳转、原生菜单/剪贴板（系统允许时）与只读拦截
        // 右侧（新路径 tracked_renamed.txt）
        let new_pane_bar = view_with_id(&root, "diff-new-pane-bar").expect("diff-new-pane-bar");
        send_click(&window, &new_pane_bar, 1);
        send_key(&app, &window, "l", "l", 37, NSEventModifierFlags::Command);
        wait_until("goto-line-modal on rename right side", Duration::from_secs(3), || {
            view_with_id(&root, "goto-line-modal").is_some()
        });
        let goto_in_r = view_with_id(&root, "goto-line-input").expect("goto-line-input");
        send_click(&window, &goto_in_r, 1);
        send_key(&app, &window, "2", "2", 19, NSEventModifierFlags::empty());
        send_key(&app, &window, "\r", "\r", 36, NSEventModifierFlags::empty());
        wait_until("goto-line-modal closed right", Duration::from_secs(3), || {
            view_with_id(&root, "goto-line-modal").is_none()
        });
        wait_until("status cursor shows 2:1 on right", Duration::from_secs(3), || {
            view_with_id(&root, "status-cursor-pos")
                .is_some_and(|v| view_a11y_text(&v).contains("2:1"))
        });
        if pb_usable {
            send_key(
                &app,
                &window,
                "c",
                "c",
                8,
                NSEventModifierFlags::Command
                    | NSEventModifierFlags::Option
                    | NSEventModifierFlags::Shift,
            );
            wait_until("toast shows new rename path reference", Duration::from_secs(3), || {
                view_with_id(&root, "toast-notification")
                    .is_some_and(|v| view_a11y_text(&v).contains("tracked_renamed.txt:2"))
            });
            assert_eq!(pasteboard_get().as_deref(), Some("tracked_renamed.txt:2"));
            send_key(&app, &window, "a", "a", 0, NSEventModifierFlags::Command);
            assert!(trigger_edit_copy_menu(&app), "Edit > Copy on rename right side");
            let right_text = pasteboard_get().unwrap_or_default();
            assert!(
                right_text.contains("version = v2"),
                "rename right Edit > Copy must copy new text, got: {right_text:?}"
            );
        }
        send_key(&app, &window, "X", "x", 7, NSEventModifierFlags::Shift);
        assert!(view_with_id(&root, "diff-dirty-badge").is_none(), "commit diff right side must remain read-only");

        // 左侧（旧路径 tracked.txt）
        let old_pane_bar = view_with_id(&root, "diff-old-pane-bar").expect("diff-old-pane-bar");
        send_click(&window, &old_pane_bar, 1);
        send_key(&app, &window, "l", "l", 37, NSEventModifierFlags::Command);
        wait_until("goto-line-modal on rename left side", Duration::from_secs(3), || {
            view_with_id(&root, "goto-line-modal").is_some()
        });
        let goto_in_l = view_with_id(&root, "goto-line-input").expect("goto-line-input");
        send_click(&window, &goto_in_l, 1);
        send_key(&app, &window, "5", "5", 23, NSEventModifierFlags::empty());
        send_key(&app, &window, "\r", "\r", 36, NSEventModifierFlags::empty());
        wait_until("goto-line-modal closed left", Duration::from_secs(3), || {
            view_with_id(&root, "goto-line-modal").is_none()
        });
        wait_until("status cursor shows 5:1 on left", Duration::from_secs(3), || {
            view_with_id(&root, "status-cursor-pos")
                .is_some_and(|v| view_a11y_text(&v).contains("5:1"))
        });
        if pb_usable {
            send_key(
                &app,
                &window,
                "c",
                "c",
                8,
                NSEventModifierFlags::Command
                    | NSEventModifierFlags::Option
                    | NSEventModifierFlags::Shift,
            );
            wait_until("toast shows old rename path reference", Duration::from_secs(3), || {
                view_with_id(&root, "toast-notification")
                    .is_some_and(|v| view_a11y_text(&v).contains("tracked.txt:5"))
            });
            assert_eq!(pasteboard_get().as_deref(), Some("tracked.txt:5"));
            send_key(&app, &window, "a", "a", 0, NSEventModifierFlags::Command);
            assert!(trigger_edit_copy_menu(&app), "Edit > Copy on rename left side");
            let left_text = pasteboard_get().unwrap_or_default();
            assert!(
                left_text.contains("version = v1"),
                "rename left Edit > Copy must copy old text, got: {left_text:?}"
            );
        }
        send_key(&app, &window, "Y", "y", 16, NSEventModifierFlags::Shift);
        assert!(view_with_id(&root, "diff-dirty-badge").is_none(), "commit diff left side must remain read-only");
        println!(
            "STEP11_NATIVE_COMMIT_RENAME_DIFF ok old={old_hdr:?} new={new_hdr:?} both_sides_goto_readonly=PASS pb_usable={pb_usable}"
        );

        let mut ids = Vec::new();
        dump_ids(&root, &mut ids);
        println!("FINAL_VIEW_IDS_COUNT={}", ids.len());

        window.close();
        pump(150);

        // 12. 真实前台窗口验证 100px 极窄左栏自动折叠至 More 按钮
        let cfg_100 = work_dir.join("narrow/config.json");
        fs::create_dir_all(cfg_100.parent().unwrap()).unwrap();
        let cfg_100_json = serde_json::json!({
            "workspace_path": repo_copy.to_string_lossy(),
            "theme": "light",
            "change_diff_preview": false,
            "commit_left_width": 100.0,
            "editor_font_size": 14.0
        });
        fs::write(&cfg_100, serde_json::to_string_pretty(&cfg_100_json).unwrap()).unwrap();
        unsafe {
            std::env::set_var("NEMOROUS_CONFIG", &cfg_100);
        }

        let win2 = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(1040.0, 660.0)),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            win2.setReleasedWhenClosed(false);
        }
        win2.setTitle(&NSString::from_str("Nemorous 100px"));
        let mut opt2 = platform::AppOptions::default();
        opt2.title = "Nemorous 100px".to_string();
        opt2.size = (1040, 660);
        let surf2 = platform::AppView::new(mtm, opt2);
        win2.setContentView(Some(&surf2.view()));
        surf2.load(platform::App::Rust(platform::Root::of::<nemorous::App>()));
        win2.makeKeyAndOrderFront(None);
        let root2 = surf2.view();
        wait_until("100px narrow sidebar shows changes-toolbar-more", Duration::from_secs(6), || {
            view_with_id(&root2, "changes-toolbar-more").is_some()
        });
        save_window_png(&win2, &evidence_dir.join("new-1040x660-100px-more.png"));
        println!("STEP12_NATIVE_100PX_MORE_MENU ok");
        win2.close();
        pump(100);

        let _ = fs::remove_dir_all(&work_dir);
        println!("NATIVE_MACOS_WINDOW_AB PASS");
    }
}
