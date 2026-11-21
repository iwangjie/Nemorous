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
        // Pump NSApplication event loop so local event monitors (nsh/keymap) process posted NSEvents!
        let deadline = Instant::now() + Duration::from_millis(220);
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

    pub fn run() {
        let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let baseline_repo = root_dir.join("build/migration-handoff/baseline/repo");
        assert!(
            baseline_repo.exists(),
            "baseline repo fixture missing: {}",
            baseline_repo.display()
        );

        let work_dir = root_dir.join("build/tmp-ab/native-macos-ab");
        let _ = fs::remove_dir_all(&work_dir);
        let repo_copy = work_dir.join("repo");
        copy_dir_all(&baseline_repo, &repo_copy);

        let cfg_path = work_dir.join("config-new.json");
        let cfg_json = serde_json::json!({
            "workspace_path": repo_copy.to_string_lossy(),
            "theme": "light",
            "change_diff_preview": false,
            "editor_font_size": 14.0
        });
        fs::write(&cfg_path, serde_json::to_string_pretty(&cfg_json).unwrap()).unwrap();

        unsafe {
            std::env::set_var("NEMOROUS_CONFIG", &cfg_path);
            std::env::set_var("NEMOROUS_WORKSPACE", &repo_copy);
        }

        let mtm = MainThreadMarker::new().expect("must run on main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.finishLaunching();

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
        let cfg_100 = work_dir.join("config-100px.json");
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
