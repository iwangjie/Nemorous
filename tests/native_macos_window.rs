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
    use platform::objc2::{MainThreadMarker, MainThreadOnly, Message, rc::Retained, sel};
    use platform::objc2_app_kit::{
        NSAccessibility, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType,
        NSBitmapImageFileType, NSBitmapImageRepPropertyKey, NSEvent, NSEventModifierFlags,
        NSEventType, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSView, NSWindow,
        NSWindowStyleMask,
    };
    use platform::objc2_foundation::{
        NSDate, NSDictionary, NSPoint, NSProcessInfo, NSRect, NSRunLoop, NSSize, NSString,
    };

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
        let pt = center_in_window(target);
        for c in 1..=clicks {
            let ts = NSProcessInfo::processInfo().systemUptime();
            let down = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                NSEventType::LeftMouseDown,
                pt,
                NSEventModifierFlags::empty(),
                ts,
                window.windowNumber(),
                None,
                0,
                c,
                1.0,
            )
            .expect("mouse down event");
            let up = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                NSEventType::LeftMouseUp,
                pt,
                NSEventModifierFlags::empty(),
                ts + 0.01,
                window.windowNumber(),
                None,
                0,
                c,
                0.0,
            )
            .expect("mouse up event");
            window.sendEvent(&down);
            pump(25);
            window.sendEvent(&up);
            pump(40);
        }
        pump(120);
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
        pump(180);
        let view = window.contentView().expect("window contentView");
        let bounds = view.bounds();
        let rep = view
            .bitmapImageRepForCachingDisplayInRect(bounds)
            .expect("bitmapImageRepForCachingDisplayInRect");
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props =
            NSDictionary::<NSBitmapImageRepPropertyKey, platform::objc2::runtime::AnyObject>::new();
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props)
        }
        .expect("PNG encode");
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(out_path, data.to_vec()).expect("write PNG");
        println!(
            "SCREENSHOT saved={} bytes={} bounds={}x{}",
            out_path.display(),
            data.len(),
            bounds.size.width,
            bounds.size.height
        );
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
        send_click(window, &view_with_id(root, "find-results-hit-0").unwrap(), 1);
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
        click("project-repo-repo", 1);
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
        click("code-split-right-btn", 1);
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

        click("code-git-history-btn", 1);
        wait("git-bottom-pane");
        wait("git-history-affected");
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
        send_key(app, window, "d", "d", 2, command);
        wait("diff-stage-hunk");
        click("diff-stage-hunk");
        wait_until("only the selected hunk entered the real index", Duration::from_secs(6), || {
            let staged = git_run(repo, &["show", ":batch3-ticket.txt"]);
            staged.contains("edited first") && !staged.contains("edited last")
        });
        wait("change-row-staged-batch3-ticket.txt");
        assert!(fs::read_to_string(repo.join("batch3-ticket.txt")).unwrap().contains("edited last"));
        save_window_png(window, &evidence.join("native-selected-hunk.png"));
        click("diff-scope-staged");
        click("diff-stage-hunk");
        wait_until("selected staged hunk was reverted from the index", Duration::from_secs(6), || {
            !git_run(repo, &["show", ":batch3-ticket.txt"]).contains("edited first")
        });
        println!("BATCH3_NATIVE_STAGE_AND_UNSTAGE ok");

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

    pub fn run() {
        let _ = env_logger::try_init();
        let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let baseline_repo = root_dir.join("build/migration-handoff/baseline/repo");
        assert!(
            baseline_repo.exists(),
            "baseline repo fixture missing: {}",
            baseline_repo.display()
        );

        let batch2 = std::env::var_os("NEMOROUS_NATIVE_BATCH2").is_some();
        let batch3 = std::env::var_os("NEMOROUS_NATIVE_BATCH3").is_some();
        let batch4 = std::env::var_os("NEMOROUS_NATIVE_BATCH4").is_some();
        let batch5 = std::env::var_os("NEMOROUS_NATIVE_BATCH5").is_some();
        let work_dir = if batch5 {
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
        copy_dir_all(&baseline_repo, &repo_copy);
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
        if batch5 {
            copy_dir_all(&repo_copy, &work_dir.join("initial-repo"));
        }
        let mut cfg_json = serde_json::json!({
            "workspace_path": repo_copy.to_string_lossy(),
            "theme": "light",
            "change_diff_preview": false,
            "editor_font_size": 14.0
        });
        if batch5 {
            cfg_json["run_configs"] = serde_json::json!([{
                "id": "native-run", "name": "Native Run fixture", "kind": "run", "command": "/bin/sh",
                "args": "-c 'printf NATIVE_RUN_OUTPUT; exit 7'"
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
        let filter_input = view_with_id(&root, "changes-filter-input").expect("changes-filter-input");
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
                && view_with_id(&root, "diff-header-path")
                    .is_some_and(|v| view_a11y_text(&v).contains("new.txt"))
        });
        let added_txt = view_with_id(&root, "diff-stat-added")
            .map(|v| view_a11y_text(&v))
            .unwrap_or_default();
        let deleted_txt = view_with_id(&root, "diff-stat-deleted")
            .map(|v| view_a11y_text(&v))
            .unwrap_or_default();
        assert_eq!(added_txt.trim(), "+1", "new.txt single line must show +1");
        assert_eq!(deleted_txt.trim(), "-0", "new.txt single line must show -0");
        println!("STEP7_DOUBLE_CLICK_NEW_TXT ok stats={added_txt}/{deleted_txt}");

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
