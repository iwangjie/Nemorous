use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use nsh_core::geometry::{point, px, size};
use nsh_core::platform::{Idiom, Lifecycle, Os};
use nsh_engine::{EngineConfig, Environment, Root};
use nsh_testing::Headless;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(unix)]
struct TestPath(Option<std::ffi::OsString>);

#[cfg(unix)]
impl TestPath {
    fn prepend(dir: &Path) -> Self {
        let original = std::env::var_os("PATH");
        let inherited = original.as_deref().into_iter().flat_map(std::env::split_paths);
        let path = std::env::join_paths(std::iter::once(dir.to_owned()).chain(inherited)).unwrap();
        unsafe { std::env::set_var("PATH", path) };
        Self(original)
    }
}

#[cfg(unix)]
impl Drop for TestPath {
    fn drop(&mut self) {
        unsafe {
            if let Some(original) = &self.0 {
                std::env::set_var("PATH", original);
            } else {
                std::env::remove_var("PATH");
            }
        }
    }
}

struct Fixture {
    root: PathBuf,
    repo_dir: PathBuf,
    config_file: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "nemorous-ui-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo_dir = root.join("sample-repo");
        let config_file = root.join("config.json");
        fs::create_dir_all(&repo_dir).unwrap();

        git_run(&repo_dir, &["init", "-b", "main"]);
        git_run(&repo_dir, &["config", "user.name", "Tester"]);
        git_run(&repo_dir, &["config", "user.email", "tester@example.com"]);

        fs::create_dir_all(repo_dir.join("src")).unwrap();
        fs::write(
            repo_dir.join("src/app.go"),
            "package main\n\nfunc hello() string {\n\treturn \"old-greeting\"\n}\n\nfunc footer() string {\n\treturn \"end\"\n}\n",
        )
        .unwrap();
        fs::write(
            repo_dir.join("src/rename_old.go"),
            "package main\n\nconst AppName = \"Nemorous\"\nconst Module = \"review\"\nconst Version = \"v1\"\n",
        )
        .unwrap();
        fs::write(repo_dir.join("README.md"), "# Sample\nline 2\n").unwrap();
        git_run(&repo_dir, &["add", "."]);
        git_run(&repo_dir, &["commit", "-m", "feat: initial fixture"]);

        // 创建第二条提交（含重命名）用于历史 Diff 验证
        git_run(&repo_dir, &["mv", "src/rename_old.go", "src/rename_new.go"]);
        fs::write(
            repo_dir.join("src/rename_new.go"),
            "package main\n\nconst AppName = \"Nemorous\"\nconst Module = \"review\"\nconst Version = \"v2\"\n",
        )
        .unwrap();
        git_run(&repo_dir, &["add", "-A"]);
        git_run(&repo_dir, &["commit", "-m", "refactor: rename file to v2"]);

        // 创建工作区变更：修改 src/app.go + 新增未跟踪 notes.txt
        fs::write(
            repo_dir.join("src/app.go"),
            "package main\n\nfunc hello() string {\n\treturn \"new-greeting\"\n}\n\nfunc footer() string {\n\treturn \"end\"\n}\n",
        )
        .unwrap();
        fs::write(repo_dir.join("notes.txt"), "untracked note line 1\n").unwrap();

        let cfg_json = serde_json::json!({
            "workspace_path": repo_dir.to_string_lossy(),
            "theme": "light",
            "change_diff_preview": false,
            "changes_flat": false
        });
        fs::write(&config_file, serde_json::to_string_pretty(&cfg_json).unwrap()).unwrap();

        Self {
            root,
            repo_dir,
            config_file,
        }
    }

    fn start_app(&self, width: f32, height: f32) -> Headless {
        self.start_app_on(width, height, Os::MacOs)
    }

    fn start_app_on(&self, width: f32, height: f32, os: Os) -> Headless {
        let mut env = Environment::default();
        env.os = os;
        env.idiom = Idiom::Desktop;
        env.apple_keyboard = os == Os::MacOs;
        self.start_app_in(width, height, env)
    }

    fn start_app_in(&self, width: f32, height: f32, env: Environment) -> Headless {
        unsafe {
            std::env::set_var("NEMOROUS_CONFIG", &self.config_file);
            std::env::set_var("NEMOROUS_WORKSPACE", &self.repo_dir);
        }
        let mut config = EngineConfig::new(Root::of::<nemorous::App>());
        config.environment = env;
        config.viewport = size(px(width), px(height));
        #[cfg(target_os = "macos")]
        let app =
            Headless::start_with_text_engine(config, nsh_platform_macos::CoreTextEngine::default)
                .expect("start headless with CoreTextEngine");
        #[cfg(not(target_os = "macos"))]
        let app = Headless::start(config).expect("start headless");
        app
    }

    fn start_headless_expect(&self, width: f32, height: f32, wait_sel: &str) -> Headless {
        let mut app = self.start_app(width, height);
        app.wait_for("id:changes-header-title-row").expect("wait for commit header");
        app.wait_for(wait_sel).expect("wait for expected element");
        app
    }

    fn start_headless(&self, width: f32, height: f32) -> Headless {
        self.start_headless_expect(width, height, "app.go")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git_run(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.com")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.com")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn open_working_change(app: &mut Headless, scope: &str, path: &str) {
    let row = format!("id:change-row-{scope}-{path}");
    app.wait_for(&row).unwrap();
    app.press(&row).unwrap();
    app.press_key("mod+d").unwrap();
}

fn wait_for_diff_file(app: &mut Headless, kind: &str, path: &str) {
    let name = nemorous::paths::basename(path);
    app.wait_for(&format!("id:{kind}-diff-pane")).unwrap();
    app.wait_for(&format!("id:editor-tab-{kind}-diff >> text:Diff: {name}")).unwrap();
}

#[test]
fn ui_commit_focus_single_click_vs_cmd_d_and_copy_rules() {
    let _guard = lock_env();
    let fx = Fixture::new("focus-copy");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 默认单击不打开 Diff，仅选中行并聚焦 Commit 树
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    assert!(!app.screen().matches("id:working-diff-pane").unwrap());

    // 2. 已跟踪文件 Cmd+C 复制 basename
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("app.go"));

    // 3. 未跟踪文件 Cmd+C 复制 displayPath（/tmp 下在主目录外为绝对路径）
    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+c").unwrap();
    let expected_untracked = nemorous::paths::display_path(
        &fx.repo_dir.join("notes.txt").to_string_lossy(),
    );
    assert_eq!(app.screen().clipboard(), Some(expected_untracked.as_str()));

    // 4. 选中 src/app.go 按 Cmd+D 打开并聚焦工作区 Diff
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.wait_for("Diff: app.go").unwrap();

    // 5. 按 Cmd+W 关闭 Diff，回到 Commit 树且不报错
    app.press_key("mod+w").unwrap();
    app.wait_for_gone("id:working-diff-pane", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn ui_working_diff_f4_source_and_cmd_bracket_return_roundtrip() {
    let _guard = lock_env();
    let fx = Fixture::new("f4-return");
    let mut app = fx.start_headless(1440.0, 900.0);

    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // 按 F4 跳转到源码页
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    assert!(!app.screen().matches("返回差异").unwrap());
    assert!(!app.screen().matches("id:code-language-tools").unwrap());
    assert!(!app.screen().matches("⌘[ 返回").unwrap());

    // 按 Cmd+[ 返回工作区 Diff
    app.press_key("mod+[").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // 按 Cmd+] 前进回源码页
    app.press_key("mod+]").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.press("id:editor-tab-working-diff").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
}

#[test]
fn ui_diff_find_focused_side_goto_line_and_whitespace_menu() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-tools");
    let mut app = fx.start_headless(1440.0, 900.0);

    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // Cmd+F 打开按聚焦侧查找条（右侧默认）
    app.press_key("mod+f").unwrap();
    app.wait_for("id:diff-find-bar").unwrap();
    app.wait_for("查找右侧").unwrap();
    app.type_text("id:diff-find-input", "new-greeting").unwrap();
    app.wait_for("1 个结果").or_else(|_| app.wait_for("1/1")).unwrap();

    // 关闭查找条后按 Cmd+L 打开转到行:列弹窗并跳到第 7 行
    app.press_key("escape").unwrap();
    app.press_key("mod+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.type_text("id:goto-line-input", "7:2").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.wait_for_gone("id:goto-line-modal", Duration::from_secs(2))
        .unwrap();
    app.wait_for("7:2").unwrap();

    // 4 级忽略空白菜单可达
    app.press("id:diff-options-btn").unwrap();
    app.choose_menu_item("空白比较 > 忽略空白和空行").unwrap();
    let cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    assert_eq!(cfg.diff_whitespace_mode, "all_and_empty");
}

#[test]
fn ui_agent_disk_change_auto_refreshes_commit_list() {
    let _guard = lock_env();
    let fx = Fixture::new("agent-watch");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 模拟外部 Agent 写盘新增一个文件
    fs::write(fx.repo_dir.join("agent_generated.rs"), "pub fn hi() {}\n").unwrap();
    // 监听去抖 280ms 后自动刷新进 Commit 树
    app.wait_for_within("agent_generated.rs", Duration::from_secs(4))
        .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn ui_review_refresh_keeps_input_live_and_merges_the_latest_open_document_state() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("review-refresh-worker");
    let control = fx.root.join("refresh-control");
    let bin = fx.root.join("bin");
    fs::create_dir(&control).unwrap();
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
if [ "$3 $4" = 'cat-file blob' ] && [ -f ../refresh-control/hold ] && [ "$5" = "$(/usr/bin/git rev-parse --verify :src/app.go 2>/dev/null)" ] && mkdir ../refresh-control/claimed 2>/dev/null; then
    /usr/bin/git "$@" > ../refresh-control/blob || exit $?
    : > ../refresh-control/started
    count=0
    while [ -f ../refresh-control/hold ] && [ "$count" -lt 250 ]; do
        sleep 0.02
        count=$((count + 1))
    done
    : > ../refresh-control/finished
    cat ../refresh-control/blob
else
    exec /usr/bin/git "$@"
fi
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();

    let hold = control.join("hold");
    let started = control.join("started");
    let finished = control.join("finished");
    let wait_for_git_marker = |app: &mut Headless, marker: &Path| {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !marker.is_file() {
            assert!(std::time::Instant::now() < deadline, "missing Git marker {}", marker.display());
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    let readme = fx.repo_dir.join("README.md");
    let file = fx.repo_dir.join("src/app.go");
    let update = |index: &str, text: &str, readme_text: &str| {
        fs::write(&file, format!("package main\n// {index}\n")).unwrap();
        git_run(&fx.repo_dir, &["add", "src/app.go"]);
        fs::write(&file, format!("package main\n// {text}\n")).unwrap();
        fs::write(&readme, format!("# {readme_text}\n")).unwrap();
    };
    fs::write(&hold, "").unwrap();
    update("first staged base", "first version on disk", "First external README");
    app.press_key("mod+alt+y").unwrap();
    wait_for_git_marker(&mut app, &started);
    assert!(!finished.is_file(), "the UI loop only returned after the held Git read finished");
    app.press("id:rail-project-btn").unwrap();
    app.wait_for("id:project-file-README.md").unwrap();
    app.type_text("id:working-diff-new-editor", "// unsaved review edit\n").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    assert!(!finished.is_file(), "the UI waited for Git before allowing the click and edit");
    fs::remove_file(&hold).unwrap();
    app.wait_for("id:working-diff-old-editor >> text:/first staged base/").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/unsaved review edit/").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "package main\n// first version on disk\n");

    wait_for_git_marker(&mut app, &finished);
    fs::remove_file(&started).unwrap();
    fs::remove_file(&finished).unwrap();
    fs::remove_dir(control.join("claimed")).unwrap();
    fs::write(&hold, "").unwrap();
    update("stale staged base", "stale disk version", "Stale external README");
    app.press_key("mod+alt+y").unwrap();
    wait_for_git_marker(&mut app, &started);
    update("latest staged base", "latest disk version", "Latest external README");
    app.press_key("mod+alt+y").unwrap();
    app.wait_for("id:working-diff-old-editor >> text:/latest staged base/").unwrap();
    app.press("id:editor-tab-file-README.md").unwrap();
    app.wait_for("id:code-editor-view >> text:# Latest external README").unwrap();
    assert!(!finished.is_file(), "the newer refresh waited behind the stale request");
    fs::remove_file(&hold).unwrap();
    wait_for_git_marker(&mut app, &finished);
    app.wait(Duration::from_millis(170)).unwrap();
    app.wait_for("id:code-editor-view >> text:# Latest external README").unwrap();
    app.press("text:Diff: app.go").unwrap();
    app.wait_for("id:working-diff-old-editor >> text:/latest staged base/").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/unsaved review edit/").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "package main\n// latest disk version\n");
}

#[test]
fn ui_review_navigation_protects_dirty_diff_and_keeps_history_on_external_write() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-history-save");
    let mut app = fx.start_headless(1440.0, 900.0);
    for path in ["src/app.go", "notes.txt"] {
        app.press(&format!("id:change-row-unstaged-{path}")).unwrap();
        app.press_key("mod+d").unwrap();
        wait_for_diff_file(&mut app, "working", path);
    }
    app.type_text("id:working-diff-new-editor", "my pending review\n").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    let file = fx.repo_dir.join("notes.txt");
    fs::write(&file, "Agent wrote to disk\n").unwrap();

    app.press_key("mod+[").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "Agent wrote to disk\n");
    app.press("id:conflict-copy-mine-btn").unwrap();
    assert!(app.screen().clipboard().unwrap().contains("my pending review"));
    app.press("id:conflict-keep-editing-btn").unwrap();
    wait_for_diff_file(&mut app, "working", "notes.txt");
    app.wait_for("id:diff-dirty-badge").unwrap();
    app.press_key("mod+[").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.press("id:conflict-accept-disk-btn").unwrap();
    app.press_key("mod+[").unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    app.press_key("mod+]").unwrap();
    wait_for_diff_file(&mut app, "working", "notes.txt");
    app.wait_for("Agent wrote to disk").unwrap();

    app.type_text("id:working-diff-new-editor", "save on history navigation\n").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    app.press_key("mod+[").unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    assert!(fs::read_to_string(&file).unwrap().contains("save on history navigation"));

    git_run(&fx.repo_dir, &["add", "notes.txt"]);
    app.wait_for("id:change-row-staged-notes.txt").unwrap();
    app.wait_for_gone("id:change-row-unstaged-notes.txt", Duration::from_secs(4)).unwrap();
    app.press_key("mod+]").unwrap();
    app.wait_for_gone("id:diff-read-status", Duration::from_secs(4)).unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    git_run(&fx.repo_dir, &["commit", "-m", "Remove notes diff from the change list", "--", "notes.txt"]);
    app.wait_for_gone("id:change-row-unstaged-notes.txt", Duration::from_secs(4)).unwrap();
    app.press_key("mod+]").unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
}

#[cfg(target_os = "macos")]
#[test]
fn ui_opening_working_diff_keeps_newer_selection_and_edits_made_during_read() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("working-diff-reader");
    let control = fx.root.join("working-read");
    let bin = fx.root.join("bin");
    fs::create_dir(&control).unwrap();
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
if [ "$3 $4" = 'cat-file blob' ] && [ -f ../working-read/hold ] && [ "$5" = "$(/usr/bin/git rev-parse --verify :src/app.go 2>/dev/null)" ] && mkdir ../working-read/claimed 2>/dev/null; then
    /usr/bin/git "$@" > ../working-read/blob || exit $?
    : > ../working-read/started
    count=0
    while [ -f ../working-read/hold ] && [ "$count" -lt 240 ]; do
        sleep 0.02
        count=$((count + 1))
    done
    cat ../working-read/blob
    : > ../working-read/finished
else
    exec /usr/bin/git "$@"
fi
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    let target = "id:change-row-unstaged-src/app.go";
    let notes = "id:change-row-unstaged-notes.txt";
    let right = "id:working-diff-new-editor";
    let hold = control.join("hold");
    let finished = control.join("finished");
    let wait_for_marker = |app: &mut Headless, name: &str| {
        let marker = control.join(name);
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !marker.is_file() {
            assert!(std::time::Instant::now() < deadline, "missing Git marker {name}");
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    fs::write(&hold, "").unwrap();
    app.press(target).unwrap();
    app.press_key("mod+d").unwrap();
    wait_for_marker(&mut app, "started");
    assert!(!finished.is_file(), "opening the Diff blocked the interface");
    app.press(notes).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(&format!("{right} >> text:untracked note line 1")).unwrap();
    assert!(!finished.is_file(), "the newer Diff was delayed behind the previous read");
    fs::remove_file(&hold).unwrap();
    wait_for_marker(&mut app, "finished");
    app.wait(Duration::from_millis(170)).unwrap();
    app.wait_for(&format!("{right} >> text:untracked note line 1")).unwrap();

    app.press("id:changes-view-options-btn").unwrap();
    app.choose_menu_item("单击时预览差异").unwrap();
    fs::remove_file(control.join("started")).unwrap();
    fs::remove_file(&finished).unwrap();
    fs::remove_dir(control.join("claimed")).unwrap();
    fs::write(&hold, "").unwrap();
    app.press(target).unwrap();
    wait_for_marker(&mut app, "started");
    app.type_text(right, "review typed while waiting\n").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    let tracked = fx.repo_dir.join("src/app.go");
    fs::write(&tracked, "package main\n// latest staged base\n").unwrap();
    git_run(&fx.repo_dir, &["add", "src/app.go"]);
    fs::write(&tracked, "package main\n// latest worktree version\n").unwrap();
    app.press_key("mod+alt+y").unwrap();
    assert!(!finished.is_file(), "editing and refresh waited for the Diff read");
    fs::remove_file(&hold).unwrap();
    app.wait_for("id:working-diff-old-editor >> text:/latest staged base/").unwrap();
    app.wait_for(&format!("{right} >> text:/latest worktree version/")).unwrap();
    let notes_path = fx.repo_dir.join("notes.txt");
    assert!(fs::read_to_string(&notes_path).unwrap().contains("review typed while waiting"));

    app.press(notes).unwrap();
    app.wait_for(&format!("{right} >> text:/review typed while waiting/")).unwrap();
    fs::remove_file(control.join("started")).unwrap();
    fs::remove_file(&finished).unwrap();
    fs::remove_dir(control.join("claimed")).unwrap();
    fs::write(&hold, "").unwrap();
    app.press(target).unwrap();
    wait_for_marker(&mut app, "started");
    app.type_text(right, "unsaved text during conflicting read\n").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    fs::write(&notes_path, "Agent saved during the read\n").unwrap();
    fs::remove_file(&hold).unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.press("id:conflict-copy-mine-btn").unwrap();
    assert!(app.screen().clipboard().unwrap().contains("unsaved text during conflicting read"));
    app.press("id:conflict-keep-editing-btn").unwrap();
    wait_for_diff_file(&mut app, "working", "notes.txt");
    app.wait_for("id:diff-dirty-badge").unwrap();
    assert_eq!(fs::read_to_string(&notes_path).unwrap(), "Agent saved during the read\n");
    app.press(notes).unwrap();
    app.wait_for_gone("id:diff-read-status", Duration::from_secs(4)).unwrap();
    app.wait(Duration::from_millis(170)).unwrap();
    app.wait_for(&format!("{right} >> text:/unsaved text during conflicting read/")).unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
}

#[test]
fn ui_review_navigation_reopens_previous_committed_diff() {
    let _guard = lock_env();
    let fx = Fixture::new("commit-diff-history");
    let initial = nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", "--short", "HEAD^"]).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+9").unwrap();
    let row = format!("id:git-commit-row-{}", initial.trim());
    app.wait_for(&row).unwrap();
    app.press(&row).unwrap();
    for path in ["README.md", "src/app.go"] {
        let row = format!("id:git-commit-file-{path}");
        app.wait_for(&row).unwrap();
        app.press(&row).unwrap();
        app.press_key("mod+d").unwrap();
        wait_for_diff_file(&mut app, "commit", path);
    }
    app.press_key("mod+[").unwrap();
    wait_for_diff_file(&mut app, "commit", "README.md");
    app.press_key("mod+]").unwrap();
    wait_for_diff_file(&mut app, "commit", "src/app.go");
}

#[cfg(target_os = "macos")]
#[test]
fn ui_slow_review_history_keeps_the_destination_until_navigation_succeeds() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("slow-review-history");
    let initial = nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", "--short", "HEAD^"]).unwrap();
    let control = fx.root.join("navigation-control");
    let bin = fx.root.join("bin");
    fs::create_dir(&control).unwrap();
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
case "$3 $4 $5" in
    'cat-file blob '*':README.md') gate=commit ;;
    'cat-file blob '*':src/app.go') gate=recent ;;
esac
if [ "$3 $4" = 'cat-file blob' ] && [ -z "$gate" ] && [ "$5" = "$(/usr/bin/git rev-parse --verify :src/app.go 2>/dev/null)" ]; then
    gate=working
fi
if [ -n "$gate" ] && [ "$(cat ../navigation-control/hold 2>/dev/null)" = "$gate" ] && mkdir ../navigation-control/claimed 2>/dev/null; then
    : > ../navigation-control/started
    count=0
    while [ -f ../navigation-control/hold ] && [ "$count" -lt 240 ]; do
        sleep 0.02
        count=$((count + 1))
    done
    : > ../navigation-control/finished
fi
if [ "$gate" = 'commit' ] && [ -f ../navigation-control/fail ]; then
    echo 'synthetic read failure' >&2
    exit 128
fi
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    let source = "id:code-editor-view >> text:# Sample";
    app.wait_for(source).unwrap();

    let hold = control.join("hold");
    let finished = control.join("finished");
    let wait_marker = |app: &mut Headless, name: &str| {
        let marker = control.join(name);
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !marker.is_file() {
            assert!(std::time::Instant::now() < deadline, "missing Git marker {name}");
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    let cancel_return = |app: &mut Headless, gate: &str| {
        fs::write(&hold, gate).unwrap();
        app.press_key("mod+[").unwrap();
        wait_marker(app, "started");
        app.press("id:editor-tab-file-README.md").unwrap();
        app.wait_for(source).unwrap();
        assert!(!finished.is_file(), "opening {gate} history blocked the source tab");
        fs::remove_file(&hold).unwrap();
        wait_marker(app, "finished");
        app.wait(Duration::from_millis(170)).unwrap();
        app.wait_for(source).unwrap();
        fs::remove_file(control.join("started")).unwrap();
        fs::remove_file(&finished).unwrap();
        fs::remove_dir(control.join("claimed")).unwrap();
    };

    for path in ["src/app.go", "notes.txt"] {
        app.press(&format!("id:change-row-unstaged-{path}")).unwrap();
        app.press_key("mod+d").unwrap();
        wait_for_diff_file(&mut app, "working", path);
    }
    cancel_return(&mut app, "working");
    app.press_key("mod+[").unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    app.press_key("mod+]").unwrap();
    app.wait_for(source).unwrap();

    app.press_key("mod+9").unwrap();
    let row = format!("id:git-commit-row-{}", initial.trim());
    app.wait_for(&row).unwrap();
    app.press(&row).unwrap();
    for path in ["README.md", "src/app.go"] {
        let row = format!("id:git-commit-file-{path}");
        app.wait_for(&row).unwrap();
        app.press(&row).unwrap();
        app.press_key("mod+d").unwrap();
        wait_for_diff_file(&mut app, "commit", path);
    }
    cancel_return(&mut app, "commit");
    fs::write(control.join("fail"), "").unwrap();
    app.press_key("mod+[").unwrap();
    app.wait_for("text:/synthetic read failure/").unwrap();
    app.wait_for(source).unwrap();
    fs::remove_file(control.join("fail")).unwrap();
    app.press_key("mod+[").unwrap();
    wait_for_diff_file(&mut app, "commit", "README.md");
    app.press_key("mod+]").unwrap();
    app.wait_for(source).unwrap();

    let recent = "text:/提交差异 · src/app.go:/";
    fs::write(&hold, "recent").unwrap();
    app.press_key("mod+shift+e").unwrap();
    app.wait_for(recent).unwrap();
    app.press(recent).unwrap();
    wait_marker(&mut app, "started");
    app.wait_for("id:diff-read-status").unwrap();
    app.press("id:editor-tab-working-diff").unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    assert!(!finished.is_file());
    fs::remove_file(&hold).unwrap();
    wait_marker(&mut app, "finished");
    app.wait(Duration::from_millis(170)).unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    app.press_key("mod+shift+e").unwrap();
    app.wait_for(recent).unwrap();
    app.press(recent).unwrap();
    wait_for_diff_file(&mut app, "commit", "src/app.go");
}

#[cfg(unix)]
#[test]
fn ui_review_refreshes_baseline_and_permissions_after_external_git_changes() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("diff-agent-commit");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    wait_for_diff_file(&mut app, "working", "notes.txt");

    git_run(&fx.repo_dir, &["add", "notes.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Agent committed notes", "--", "notes.txt"]);
    app.wait_for_gone("id:change-row-unstaged-notes.txt", Duration::from_secs(4)).unwrap();
    app.wait_for("id:working-diff-pane >> text:0 处差异").unwrap();

    let notes = fx.repo_dir.join("notes.txt");
    fs::set_permissions(&notes, fs::Permissions::from_mode(0o444)).unwrap();
    app.wait_for("id:diff-new-pane-bar >> text:只读").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.wait_for("磁盘文件为只读权限").unwrap();
    fs::set_permissions(&notes, fs::Permissions::from_mode(0o644)).unwrap();
    app.wait_for_gone("磁盘文件为只读权限", Duration::from_secs(4)).unwrap();

    git_run(&fx.repo_dir, &["mv", "notes.txt", "renamed-notes.txt"]);
    fs::write(fx.repo_dir.join("renamed-notes.txt"), "Agent changed after renaming\n").unwrap();
    let staged = "id:change-row-staged-renamed-notes.txt";
    let unstaged = "id:change-row-unstaged-renamed-notes.txt";
    app.wait_for(staged).unwrap();
    app.wait_for(unstaged).unwrap();
    app.press(staged).unwrap();
    app.press_key("mod+d").unwrap();
    wait_for_diff_file(&mut app, "working", "renamed-notes.txt");
    app.press("id:diff-details").unwrap();
    assert!(app.menu().unwrap().iter().any(|item| item.label.contains("notes.txt → renamed-notes.txt")));
    app.close_menu().unwrap();
    app.press(unstaged).unwrap();
    app.press_key("mod+d").unwrap();
    wait_for_diff_file(&mut app, "working", "renamed-notes.txt");
    app.wait_for("Agent changed after renaming").unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn ui_repository_revision_keeps_exact_version_when_reopening_history() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("revision-tab-history");
    let path = "src/rename_new.go";
    let file = fx.repo_dir.join(path);
    let worktree = fs::read_to_string(&file).unwrap().replace("\"v2\"", "\"worktree-v3\"");
    fs::write(&file, &worktree).unwrap();
    let control = fx.root.join("revision-read");
    let bin = fx.root.join("bin");
    fs::create_dir(&control).unwrap();
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
case "$3 $4 $5" in
    'cat-file blob '*':src/rename_new.go')
        if [ -f ../revision-read/hold ] && mkdir ../revision-read/claimed 2>/dev/null; then
            : > ../revision-read/started
            count=0
            while [ -f ../revision-read/hold ] && [ "$count" -lt 240 ]; do
                sleep 0.02
                count=$((count + 1))
            done
            : > ../revision-read/finished
        fi
        if [ -f ../revision-read/fail ]; then
            echo 'synthetic source read failure' >&2
            exit 128
        fi ;;
esac
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);

    let mut app = fx.start_headless(1440.0, 900.0);
    let source = "id:code-editor-view >> text:# Sample";
    let revision_contents = "id:code-editor-view >> text:/const Version = \"v2\"/";
    let row = "id:git-commit-file-src/rename_new.go";
    let open_source = |app: &mut Headless, path: &str| {
        app.press_key("mod+shift+o").unwrap();
        app.type_text("id:review-picker-input", path).unwrap();
        app.submit("id:review-picker-input").unwrap();
    };
    open_source(&mut app, "README.md");
    app.wait_for(source).unwrap();
    app.press_key("mod+9").unwrap();
    app.wait_for(row).unwrap();
    app.right_click(row).unwrap();
    app.choose_menu_item("打开仓库版本").unwrap();
    app.wait_for(revision_contents).unwrap();
    app.press_key("mod+[").unwrap();
    app.wait_for(source).unwrap();
    app.press_key("mod+]").unwrap();
    app.wait_for(revision_contents).unwrap();

    open_source(&mut app, path);
    let live = "id:code-editor-view >> text:/worktree-v3/";
    app.wait_for(live).unwrap();
    app.press_key("mod+[").unwrap();
    app.wait_for(revision_contents).unwrap();
    app.press_key("mod+w").unwrap();
    app.wait_for_gone("id:code-editor-view >> text:/const Version = \"v2\"/", Duration::from_secs(4)).unwrap();
    app.press_key("mod+]").unwrap();
    app.wait_for(live).unwrap();

    let hold = control.join("hold");
    let finished = control.join("finished");
    let wait_marker = |app: &mut Headless, name: &str| {
        let marker = control.join(name);
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !marker.is_file() {
            assert!(std::time::Instant::now() < deadline, "missing Git marker {name}");
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    fs::write(&hold, "").unwrap();
    app.right_click(row).unwrap();
    app.choose_menu_item("打开仓库版本").unwrap();
    wait_marker(&mut app, "started");
    app.press("id:editor-tab-file-README.md").unwrap();
    app.wait_for(source).unwrap();
    assert!(!finished.is_file());
    fs::remove_file(&hold).unwrap();
    wait_marker(&mut app, "finished");
    app.wait(Duration::from_millis(170)).unwrap();
    app.wait_for(source).unwrap();

    fs::write(control.join("fail"), "").unwrap();
    app.right_click(row).unwrap();
    app.choose_menu_item("打开仓库版本").unwrap();
    app.wait_for("text:/synthetic source read failure/").unwrap();
    app.wait_for(source).unwrap();
    fs::remove_file(control.join("fail")).unwrap();

    app.press_key("mod+shift+e").unwrap();
    let recent = "text:/仓库版本 · src/rename_new.go/";
    app.wait_for(recent).unwrap();
    app.press(recent).unwrap();
    app.wait_for(revision_contents).unwrap();
    app.wait_for("text:/为只读视图/").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), worktree);

    app.press("text:feat: initial fixture").unwrap();
    let old_row = "id:git-commit-file-src/rename_old.go";
    app.wait_for(old_row).unwrap();
    app.right_click(old_row).unwrap();
    app.choose_menu_item("打开仓库版本").unwrap();
    app.wait_for("id:code-editor-view >> text:/const Version = \"v1\"/").unwrap();
    assert!(!fx.repo_dir.join("src/rename_old.go").exists());
}

#[test]
fn ui_save_conflict_modal_and_second_disk_change_recheck() {
    let _guard = lock_env();
    let fx = Fixture::new("save-conflict");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 打开 src/app.go 源码页并进行轻编辑
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();

    // 通过查找替换把 new-greeting 改成 my-local-edit（确保编辑器变脏）
    app.press_key("mod+alt+f").unwrap();
    app.wait_for("id:code-find-bar").unwrap();
    app.type_text("id:code-find-input", "new-greeting").unwrap();
    app.type_text("id:code-replace-input", "my-local-edit").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("未保存").unwrap();

    // 外部 Agent 覆盖磁盘文件
    fs::write(
        fx.repo_dir.join("src/app.go"),
        "package main\n\nfunc hello() string {\n\treturn \"external-v1\"\n}\n",
    )
    .unwrap();

    // 按 Cmd+S 触发受检保存冲突弹窗
    app.press_key("mod+s").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.wait_for("磁盘文件已变化，编辑仍保留").unwrap();

    // 弹窗打开期间外部再次改盘 -> 点「使用磁盘版本」必须先二次核对并给出警告！
    fs::write(
        fx.repo_dir.join("src/app.go"),
        "package main\n\nfunc hello() string {\n\treturn \"external-v2\"\n}\n",
    )
    .unwrap();
    app.press("id:conflict-accept-disk-btn").unwrap();
    app.wait_for("磁盘内容再次变化，请核对最新版本").unwrap();

    // 再次点击「使用磁盘版本」确认采用 v2
    app.press("id:conflict-accept-disk-btn").unwrap();
    app.wait_for_gone("id:save-conflict-modal", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn ui_committed_diff_and_rename_context_copy() {
    let _guard = lock_env();
    let fx = Fixture::new("commit-diff");
    let mut app = fx.start_headless(1440.0, 900.0);

    // ⌘9 打开 Git 底部面板
    app.press_key("mod+9").unwrap();
    app.wait_for("id:git-bottom-pane").unwrap();
    app.wait_for("text:refactor: rename file to v2").unwrap();

    // 选中重命名文件并按 Cmd+D 打开历史提交 Diff
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.press("id:git-commit-file-src/rename_new.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:commit-diff-pane").unwrap();
    app.press("id:diff-details").unwrap();
    assert!(app.menu().unwrap().iter().any(|item| item.label.contains("src/rename_old.go → src/rename_new.go")));
    app.close_menu().unwrap();

    // 1. 默认聚焦右侧（新路径）：⌘L 跳第 2 行，⌘⌥⇧C 复制新路径:2
    app.press_key("mod+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.type_text("id:goto-line-input", "2:1").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.wait_for_gone("id:goto-line-modal", Duration::from_secs(2))
        .unwrap();
    app.press_key("mod+alt+shift+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src/rename_new.go:2"));

    // 右侧全选 + ⌘C 复制新版本文本，键入字符绝不修改只读内容
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    let right_clip = app.screen().clipboard().unwrap_or("").to_string();
    assert!(
        right_clip.contains("Version = \"v2\""),
        "expected new commit text in clipboard, got {right_clip:?}"
    );
    app.press_key("x").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard().unwrap_or(""), right_clip);

    // 2. 切到左侧（旧路径）：聚焦左侧编辑器，⌘L 跳第 1 行，⌘⌥⇧C 复制旧路径:1，选区片段复制 + 全选复制 + 右键复制 + 只读拦截
    app.press("id:diff-old-pane-bar").unwrap();
    app.press_key("mod+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.type_text("id:goto-line-input", "1:1").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.wait_for_gone("id:goto-line-modal", Duration::from_secs(2))
        .unwrap();
    app.press_key("mod+alt+shift+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src/rename_old.go:1"));

    // 左侧局部选区片段复制（第 1 行前 7 字符 "package"）
    for _ in 0..7 {
        app.press_key("shift+right").unwrap();
    }
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("package"));

    // 左侧全选 + ⌘C 复制旧版本文本，再通过右键上下文菜单「复制」验证，键入字符绝不修改只读内容
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    let left_clip = app.screen().clipboard().unwrap_or("").to_string();
    assert!(
        left_clip.contains("Version = \"v1\""),
        "expected old commit text in clipboard, got {left_clip:?}"
    );
    // 先把剪贴板改成路径（全选后光标 head 在第 6 行），再右键左侧代码行点「复制」，验证右键「复制」真实把旧版本选区写入剪贴板
    app.press_key("mod+alt+shift+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src/rename_old.go:6"));
    app.press_key("mod+a").unwrap();
    app.right_click("const Version = \"v1\"").unwrap();
    app.choose_menu_item("复制").unwrap();
    assert_eq!(app.screen().clipboard().unwrap_or(""), left_clip);
    app.press_key("z").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard().unwrap_or(""), left_clip);

    // 3. 切去其他标签再点回提交 Diff 标签时，保留左侧 active_side（不再被硬编码重置到右侧）
    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();

    app.press("id:editor-tab-commit-diff").unwrap();
    app.wait_for("id:commit-diff-pane").unwrap();
    app.press_key("mod+alt+shift+c").unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("src/rename_old.go:6"),
        "switching back to commit diff tab must preserve active_side=Old"
    );

    app.right_click("id:commit-diff-old-editor >> text:const Version = \"v1\"").unwrap();
    app.choose_menu_item("跳转到源码 (F4)").unwrap();
    app.wait_for("id:code-editor-view >> text:const Version = \"v2\"").unwrap();
    app.press_key("mod+alt+shift+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src/rename_new.go:5"));
}

#[test]
fn ui_narrow_pane_1040x660_and_100px_more_menu_stash() {
    let _guard = lock_env();
    let fx = Fixture::new("narrow");
    let mut app = fx.start_headless(1040.0, 660.0);

    // 1. 1040x660 默认左栏 320px：标题行与计数单行展示且按钮可见
    app.wait_for("id:changes-header-title-row").unwrap();
    app.wait_for("id:changes-view-options-btn").unwrap();
    app.wait_for("id:changes-stash-btn").unwrap();

    // 2. 通过配置以极窄左栏 100px 启动，验证工具栏自动收入 More 菜单且可直达 Stash -> 管理 Stash…
    let narrow_cfg = serde_json::json!({
        "workspace_path": fx.repo_dir.to_string_lossy(),
        "theme": "light",
        "commit_left_width": 100.0,
        "code_left_width": 100.0
    });
    fs::write(&fx.config_file, serde_json::to_string_pretty(&narrow_cfg).unwrap()).unwrap();
    drop(app);

    let mut narrow_app = fx.start_headless_expect(1040.0, 660.0, "id:changes-toolbar-more");
    narrow_app.press("id:changes-toolbar-more").unwrap();
    narrow_app.choose_menu_item("Stash > 管理 Stash… (0)").unwrap();
    narrow_app.wait_for("id:stash-modal").unwrap();
    narrow_app.press_key("escape").unwrap();
    narrow_app.press_key("mod+1").unwrap();
    narrow_app.wait_for("id:project-hide").unwrap();
    narrow_app.press("id:project-options-button").unwrap();
    narrow_app.choose_menu_item("全部折叠").unwrap();
    narrow_app.wait_for("id:project-workspace-root").unwrap();
}

#[test]
fn ui_workspace_stash_resumes_index_and_untracked_files() {
    let _guard = lock_env();
    let fx = Fixture::new("workspace-stash");
    git_run(&fx.repo_dir, &["add", "src/app.go"]);
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:changes-stash-btn").unwrap();
    app.choose_menu_item("贮藏全部仓库").unwrap();
    app.wait_for("id:git-advanced-report").unwrap();
    assert!(!fx.repo_dir.join("notes.txt").exists());
    assert!(fs::read_to_string(fx.repo_dir.join("src/app.go")).unwrap().contains("old-greeting"));
    app.press_key("escape").unwrap();

    app.press("id:changes-stash-btn").unwrap();
    app.choose_menu_item("恢复全部仓库的最近贮藏…").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("id:git-advanced-report").unwrap();
    assert!(fx.repo_dir.join("notes.txt").exists());
    let staged = Command::new("git")
        .current_dir(&fx.repo_dir)
        .args(["show", ":src/app.go"])
        .output()
        .unwrap();
    assert!(staged.status.success());
    assert!(String::from_utf8(staged.stdout).unwrap().contains("new-greeting"));
}

#[test]
fn ui_project_follow_and_keyboard_reveal_offscreen_files() {
    let _guard = lock_env();
    let fx = Fixture::new("project-reveal-offscreen");
    for i in 0..60 {
        fs::write(fx.repo_dir.join(format!("review-{i:02}.txt")), "review line\n").unwrap();
    }
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+1").unwrap();
    app.wait_for("id:project-file-README.md").unwrap();
    let rows = "id:project-tree-rows";
    assert!(app.scroll(rows, px(-24.0)).is_err());

    app.hover("id:project-tree-scroll").unwrap();
    app.press("id:project-options-button").unwrap();
    app.choose_menu_item("行为 > 始终选中打开的文件").unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "review-59.txt").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:editor-tab-file-review-59.txt").unwrap();
    app.wait(Duration::from_millis(100)).unwrap();
    app.scroll(rows, px(-24.0)).expect("following the file should have scrolled the tree");

    app.press_key("mod+1").unwrap();
    let previous_commands = app.screen().commands().len();
    app.press_key("down").unwrap();
    app.wait(Duration::from_millis(100)).unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src"));
    assert!(app.screen().commands()[previous_commands..].iter().any(|(_, command)| {
        matches!(command, nsh_core::Command::ScrollTo { y, .. } if y.0 < 100.0)
    }), "keyboard wrap should reveal the first selectable row");
}

#[test]
fn ui_project_tree_locate_and_file_picker_line_col() {
    let _guard = lock_env();
    let fx = Fixture::new("picker-locate");
    let mut app = fx.start_headless(1440.0, 900.0);

    // Cmd+Shift+O 转到文件支持 path:line:col
    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:4:2").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.wait_for("4:2").unwrap();

    // ⌥F1 在文件树中定位 -> 切到 Project 面板并展开 src 目录显示 app.go
    app.press_key("alt+f1").unwrap();
    app.wait_for("id:project-pane").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
}

#[test]
fn ui_commit_view_options_preview_and_flat_toggle() {
    let _guard = lock_env();
    let fx = Fixture::new("view-opts");
    for directory in ["config/development", "config/production"] {
        let path = fx.repo_dir.join(directory);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("application.yml"), "fixture: true\n").unwrap();
    }
    let mut app = fx.start_headless(1440.0, 900.0);

    // 打开“变更视图选项”并启用“单击时预览差异”
    app.press("id:changes-view-options-btn").unwrap();
    app.choose_menu_item("单击时预览差异").unwrap();

    // 开启后单击 src/app.go 立即打开工作区 Diff
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // 平铺列表把文件名与目录分别显示，同名文件靠目录区分。
    app.press("id:changes-view-options-btn").unwrap();
    app.choose_menu_item("按目录分组").unwrap();
    app.wait_for_gone("id:change-dir-unstaged-src", Duration::from_secs(2)).unwrap();
    app.wait_for("id:change-row-unstaged-src/app.go >> text:app.go").unwrap();
    app.wait_for("id:change-row-unstaged-src/app.go >> text:src").unwrap();
    for directory in ["config/development", "config/production"] {
        let row = format!("id:change-row-unstaged-{directory}/application.yml");
        app.wait_for(&format!("{row} >> text:application.yml")).unwrap();
        app.wait_for(&format!("{row} >> text:{directory}")).unwrap();
        assert!(!app.screen().matches(&format!("{row} >> text:{directory}/application.yml")).unwrap());
    }
}

#[test]
fn ui_keyboard_selection_scrolls_commit_and_git_file_lists() {
    let _guard = lock_env();
    let fx = Fixture::new("review-list-keyboard-scroll");
    for i in 0..58 {
        fs::write(fx.repo_dir.join(format!("a-review-{i:02}.txt")), "baseline\n").unwrap();
    }
    git_run(&fx.repo_dir, &["add", "--", "a-review-*.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Many files to review"]);
    for i in 0..18 {
        git_run(&fx.repo_dir, &["commit", "--allow-empty", "-m", &format!("Review checkpoint {i:02}")]);
    }
    for i in 0..58 {
        fs::write(fx.repo_dir.join(format!("a-review-{i:02}.txt")), "working edit\n").unwrap();
    }

    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.changes_flat = true;
    nemorous::config::save_to(&fx.config_file, &config).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    let first = "id:change-row-unstaged-a-review-00.txt";
    let last = "id:change-row-unstaged-src/app.go";
    app.wait_for(first).unwrap();
    app.scroll("id:changes-tree-rows", px(2000.0)).unwrap();
    app.press(last).unwrap();
    let before = app.screen().commands().len();
    app.press_key("down").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("a-review-00.txt"));
    assert!(app.screen().commands()[before..].iter().any(|(_, command)| {
        matches!(command, nsh_core::Command::ScrollTo { y, .. } if y.0 < 100.0)
    }), "Commit tree should scroll back to the selected first row");

    app.press_key("mod+9").unwrap();
    app.wait_for("text:Review checkpoint 17").unwrap();
    app.press("text:Review checkpoint 17").unwrap();
    let log = "id:git-commit-list-scroll";
    app.scroll(log, px(1500.0)).unwrap();
    let before = app.screen().commands().len();
    app.press_key("down").unwrap();
    assert!(app.screen().commands()[before..].iter().any(|(_, command)| {
        matches!(command, nsh_core::Command::ScrollTo { y, .. } if y.0 < 100.0)
    }), "Git log should scroll back to the selected commit");
    app.press("text:Many files to review").unwrap();
    let file = "id:git-commit-file-a-review-00.txt";
    app.wait_for(file).unwrap();
    app.press(file).unwrap();
    app.scroll("id:git-commit-file-list-scroll", px(2500.0)).unwrap();
    let before = app.screen().commands().len();
    app.press_key("down").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("a-review-01.txt"));
    assert!(app.screen().commands()[before..].iter().any(|(_, command)| {
        matches!(command, nsh_core::Command::ScrollTo { y, .. } if y.0 < 100.0)
    }), "Git files should scroll back to the selected second row");
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: a-review-01.txt").unwrap();
}

#[test]
fn ui_commit_list_scrolls_long_paths_in_both_directions() {
    let _guard = lock_env();
    let fx = Fixture::new("long-change-path");
    let path = "src/development/review/fixture/a-very-long-file-name-for-the-local-changes-panel.example.yaml";
    let absolute = fx.repo_dir.join(path);
    fs::create_dir_all(absolute.parent().unwrap()).unwrap();
    fs::write(absolute, "review: true\n").unwrap();
    for i in 0..40 {
        fs::write(fx.repo_dir.join(format!("src/file-{i:02}.txt")), "change\n").unwrap();
    }
    let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    cfg.commit_left_width = 350.0;
    cfg.changes_flat = true;
    nemorous::config::save_to(&fx.config_file, &cfg).unwrap();
    let mut app = fx.start_headless_expect(1140.0, 720.0, &format!("id:change-row-unstaged-{path}"));

    app.scroll("id:changes-tree-scroll", px(1_000.0)).unwrap();
    app.wait_for("text:src/development/review/fixture").unwrap();
    assert!(!app.screen().matches(r"id:changes-tree-scroll >> text:/^\?$/").unwrap());
    app.scroll("id:changes-tree-rows", px(150.0)).unwrap();
    assert!(!app.screen().matches("id:changes-filter-input").unwrap());
    app.press_key("mod+f").unwrap();
    app.wait_for("id:changes-filter-input").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:changes-filter-input", Duration::from_secs(2)).unwrap();
    app.type_text("id:git-commit-message", "Review changes").unwrap();
    app.scroll("id:changes-tree-scroll", px(-1_000.0)).unwrap();
    app.scroll("id:changes-tree-rows", px(-150.0)).unwrap();
}

#[test]
fn ui_project_tree_scrolls_long_names_without_moving_the_header() {
    let _guard = lock_env();
    let filename = format!("000-{}-filename-ending.txt", "project-name-".repeat(12));
    let viewport = "project-tree-scroll";
    let header = "project-header-title-row";
    let filter = "project-filter-input";
    let row = format!("project-file-{filename}");
    let bounds = |app: &Headless, id| {
        app.screen().layout_nodes().into_iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("missing layout for {id}")).bounds
    };
    for width in [350.0, 128.0] {
        let fx = Fixture::new(&format!("long-project-filename-{width}"));
        fs::write(fx.repo_dir.join(&filename), "long filename\n").unwrap();
        for index in 0..40 {
            fs::write(fx.repo_dir.join(format!("project-file-{index:02}.txt")), "fixture\n").unwrap();
        }
        let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
        config.code_left_width = width;
        nemorous::config::save_to(&fx.config_file, &config).unwrap();

        let mut app = fx.start_headless(1140.0, 720.0);
        app.press_key("mod+1").unwrap();
        app.wait_for(&format!("id:{row}")).unwrap_or_else(|err| panic!("{width}-point project tree: {err}"));
        assert!(!app.screen().matches("id:project-filter-input").unwrap());
        app.press_key("mod+f").unwrap();
        app.wait_for("id:project-filter-input").unwrap();
        let initial_row = bounds(&app, &row);
        let initial_header = bounds(&app, header);
        let initial_filter = bounds(&app, filter);
        assert_eq!(bounds(&app, viewport).size.width, px(width));

        app.wheel("id:project-tree-scroll", point(px(2_000.0), px(0.0))).unwrap();
        let after = bounds(&app, &row);
        assert!(after.origin.x < initial_row.origin.x, "the long filename did not scroll horizontally at {width} points");
        let clip = bounds(&app, viewport);
        assert!(after.origin.x + after.size.width <= clip.origin.x + clip.size.width + px(1.0));
        assert_eq!(after.origin.y, initial_row.origin.y);
        assert_eq!(bounds(&app, header), initial_header);
        assert_eq!(bounds(&app, filter), initial_filter);
        app.wheel("id:project-tree-scroll", point(px(-2_000.0), px(0.0))).unwrap();
        assert_eq!(bounds(&app, &row).origin.x, initial_row.origin.x);

        app.wheel("id:project-tree-scroll", point(px(0.0), px(420.0))).unwrap();
        assert!(bounds(&app, &row).origin.y < initial_row.origin.y);
        assert_eq!(bounds(&app, header), initial_header);
        assert_eq!(bounds(&app, filter), initial_filter);
    }
}

#[test]
fn ui_commit_context_menu_copy_path_variants_and_locate() {
    let _guard = lock_env();
    let fx = Fixture::new("ctx-menu");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 右键 src/app.go -> 复制路径／引用 > 相对仓库路径
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("复制路径／引用 > 相对仓库路径").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src/app.go"));

    // 右键 src/app.go -> 复制路径／引用 > 绝对路径
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("复制路径／引用 > 绝对路径").unwrap();
    let expected_abs = fx.repo_dir.join("src/app.go").to_string_lossy().to_string();
    assert_eq!(app.screen().clipboard(), Some(expected_abs.as_str()));

    // 右键 src/app.go -> 在文件树中定位
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("在文件树中定位").unwrap();
    app.wait_for("id:project-pane").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
}

#[test]
fn ui_editor_structure_and_todo_navigate_to_source_lines() {
    let _guard = lock_env();
    let fx = Fixture::new("editor-tools");
    fs::write(fx.repo_dir.join("README.md"), "# Sample\nTODO project setup\n").unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);

    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.press_key("mod+f12").unwrap();
    app.wait_for("id:structure-modal").unwrap();
    app.press("id:structure-row-7").unwrap();
    app.wait_for_gone("id:structure-modal", Duration::from_secs(2)).unwrap();
    app.wait_for("7:1").unwrap();

    app.press_key("mod+shift+t").unwrap();
    app.wait_for("id:todos-modal").unwrap();
    app.wait_for("README.md:2").unwrap();
    app.press("id:todo-row-0").unwrap();
    app.wait_for_gone("id:todos-modal", Duration::from_secs(2)).unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();
    app.wait_for("2:1").unwrap();
}

#[test]
fn ui_fonts_can_be_adjusted_and_reset_independently() {
    let _guard = lock_env();
    let fx = Fixture::new("independent-fonts");
    let mut config: serde_json::Value = serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    config["editor_font_size"] = 17.0.into();
    fs::write(&fx.config_file, config.to_string()).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:rail-settings-btn").unwrap();
    for id in ["font-increase", "tree-font-increase", "search-font-increase"] {
        app.press(&format!("id:{id}")).unwrap();
    }
    let saved = || nemorous::config::load_from(&fx.config_file).unwrap();
    let config = saved();
    assert_eq!((config.editor_font_size, config.diff_font_size), (18.0, 17.0));
    assert_eq!((config.tree_font_size, config.search_font_size), (12.5, 13.0));
    app.press("id:diff-font-reset").unwrap();
    assert_eq!(saved().diff_font_size, 14.0);
    assert_eq!(saved().editor_font_size, 18.0);
}

#[test]
fn ui_search_previews_selected_context_and_discards_old_queries() {
    let _guard = lock_env();
    let fx = Fixture::new("search-context-preview");
    let document: String = (1..=100).map(|line| match line {
        70 => "查找内容\n".to_owned(),
        71 => "文档上下文\n".to_owned(),
        90 => "远处上下文\n".to_owned(),
        _ => format!("文档行 {line:03}\n"),
    }).collect();
    fs::write(fx.repo_dir.join("README.md"), document).unwrap();
    fs::write(fx.repo_dir.join("notes.txt"), "第二处\n查找内容\n备注上下文\n").unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    let input = "id:content-search-input";
    let preview = "id:search-preview-editor";
    app.press_key("mod+shift+f").unwrap();
    app.type_text(input, "查找内容").unwrap();
    app.wait_for("id:content-search-hit-1").unwrap();
    app.wait_for(&format!("{preview} >> text:文档上下文")).unwrap();
    app.wait_for(&format!("{preview} >> text:70")).unwrap();
    let preview_height = |app: &Headless| {
        app.screen().layout_nodes().into_iter()
            .find(|node| node.test_id.as_deref() == Some("search-code-preview"))
            .unwrap().bounds.size.height
    };
    let original_height = preview_height(&app);
    app.drag("label:调整代码预览高度", point(px(0.0), px(-45.0))).unwrap();
    assert!(preview_height(&app) > original_height);
    app.wheel(preview, point(px(0.0), px(390.0))).unwrap();
    let distant = format!("{preview} >> text:远处上下文");
    app.wait_for(&distant).unwrap();
    app.press(&distant).unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert!(app.screen().clipboard().unwrap_or_default().contains("远处上下文"));
    app.press(input).unwrap();
    app.press_key("down").unwrap();
    app.wait_for(&format!("{preview} >> text:备注上下文")).unwrap();
    assert!(!app.screen().matches("id:editor-tab-file-notes.txt").unwrap());
    app.press("id:content-search-preview-toggle").unwrap();
    app.wait_for_gone(preview, Duration::from_secs(2)).unwrap();
    app.press("id:content-search-preview-toggle").unwrap();
    app.wait_for(&format!("{preview} >> text:备注上下文")).unwrap();
    app.clear(input).unwrap();
    app.wait_for_gone(preview, Duration::from_secs(2)).unwrap();
    app.type_text(input, "文档上下文").unwrap();
    app.wait_for(&format!("{preview} >> text:文档上下文")).unwrap();
    assert!(!app.screen().matches(&format!("{preview} >> text:备注上下文")).unwrap());
    app.submit(input).unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();
}

#[test]
fn ui_find_results_pin_preserves_snapshot_and_reopens() {
    let _guard = lock_env();
    let fx = Fixture::new("find-results-pin");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 正常工作区搜索，等到稳定命中后再钉入面板。
    app.press_key("mod+shift+f").unwrap();
    app.wait_for("id:content-search-modal").unwrap();
    app.type_text("id:content-search-input", "new-greeting").unwrap();
    app.wait_for("id:content-search-hit-0").unwrap();
    app.press("id:content-search-pin").unwrap();
    app.wait_for_gone("id:content-search-modal", Duration::from_secs(2))
        .unwrap();

    // 2. 底部面板显示查询与分组命中。
    app.wait_for("id:find-results-panel").unwrap();
    app.wait_for("查找 · new-greeting").unwrap();
    app.wait_for("id:find-results-hit-0").unwrap();

    app.set_viewport(size(px(800.0), px(640.0))).unwrap();
    let preview_width = |app: &Headless| {
        app.screen().layout_nodes().into_iter()
            .find(|node| node.test_id.as_deref() == Some("search-code-preview"))
            .unwrap().bounds.size.width
    };
    let original_width = preview_width(&app);
    app.drag("label:调整代码预览宽度", point(px(-45.0), px(0.0))).unwrap();
    assert!(preview_width(&app) > original_width);
    app.press("id:find-results-hit-0").unwrap();
    app.wait_for("id:search-preview-editor >> text:/new-greeting/").unwrap();
    assert!(!app.screen().matches("id:editor-tab-file-src/app.go").unwrap());
    app.press_key("enter").unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();

    // 4. 隐藏后经左侧「查找结果」按钮重新打开，快照仍在。
    app.press("id:find-results-hide").unwrap();
    app.wait_for_gone("id:find-results-panel", Duration::from_secs(2))
        .unwrap();
    app.press("id:rail-find-btn").unwrap();
    app.wait_for("id:find-results-panel").unwrap();
    app.wait_for("id:find-results-hit-0").unwrap();

    // 5. 后续模态活动（新查询、关闭）不会改写已钉入的快照。
    app.press_key("mod+shift+f").unwrap();
    app.wait_for("id:content-search-modal").unwrap();
    app.type_text("id:content-search-input", "Sample").unwrap();
    app.wait_for("id:content-search-hit-0").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:content-search-modal", Duration::from_secs(2))
        .unwrap();
    app.wait_for("查找 · new-greeting").unwrap();
    app.wait_for("id:find-results-hit-0").unwrap();
}

#[test]
fn ui_moved_worktree_source_keeps_active_editor_at_stale_entry_points() {
    let _guard = lock_env();
    let fx = Fixture::new("moved-source-navigation");
    let mut app = fx.start_headless(1440.0, 900.0);
    let readme = "id:code-editor-view >> text:# Sample";
    let old_tab = "id:editor-tab-file-src/app.go";
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for(readme).unwrap();

    app.press_key("mod+shift+f").unwrap();
    app.type_text("id:content-search-input", "new-greeting").unwrap();
    app.wait_for("id:content-search-hit-0").unwrap();
    app.press("id:content-search-pin").unwrap();
    app.wait_for("id:find-results-hit-0").unwrap();
    app.press_key("mod+1").unwrap();
    app.press("id:project-dir-toggle-src").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    app.right_click("id:project-file-src/app.go").unwrap();

    let previous = fx.repo_dir.join("src/app.go");
    let moved = fx.repo_dir.join("src/moved.go");
    fs::rename(&previous, &moved).unwrap();
    app.choose_menu_item("预览").unwrap();
    let missing = "text:/文件已移动或删除/";
    app.wait_for(missing).unwrap();
    app.wait_for(readme).unwrap();
    assert!(!app.screen().matches(old_tab).unwrap());
    app.press("label:关闭提示").unwrap();
    app.wait_for_gone("id:toast-notification", Duration::from_secs(2)).unwrap();
    app.press("id:find-results-hit-0").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for(missing).unwrap();
    app.wait_for(readme).unwrap();
    assert!(!app.screen().matches(old_tab).unwrap());
    app.press("label:关闭提示").unwrap();
    app.wait_for_gone("id:toast-notification", Duration::from_secs(2)).unwrap();

    app.wait_for("id:project-file-src/moved.go").unwrap();
    app.right_click("id:project-file-src/moved.go").unwrap();
    app.choose_menu_item("打开源码").unwrap();
    app.wait_for("id:code-editor-view >> text:/new-greeting/").unwrap();
    assert!(!previous.exists());

    app.press_key("mod+0").unwrap();
    let deleted = "id:change-row-unstaged-src/app.go";
    app.wait_for(deleted).unwrap();
    app.press(deleted).unwrap();
    app.press_key("mod+d").unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    app.press_key("f4").unwrap();
    app.wait_for(missing).unwrap();
    wait_for_diff_file(&mut app, "working", "src/app.go");
    assert!(!app.screen().matches(old_tab).unwrap());
}

#[test]
fn ui_content_search_modal_scope_open_and_close() {
    let _guard = lock_env();
    let fx = Fixture::new("content-search");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 先回到项目树（此时没有打开任何文件，Changes 面板仍是当前面板）。
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("在文件树中定位").unwrap();
    app.wait_for("id:project-dir-src").unwrap();

    // 2. 目录右键「在目录中查找」：范围锁定 src，命中只来自 src。
    app.right_click("id:project-dir-src").unwrap();
    app.choose_menu_item("在目录中查找").unwrap();
    app.wait_for("id:content-search-modal").unwrap();
    app.wait_for("id:content-search-scope").unwrap();
    app.type_text("id:content-search-input", "new-greeting").unwrap();
    app.wait_for("id:content-search-hit-0").unwrap();

    // README 中的 Sample 不在 src 范围内，新查询也会清掉上一轮的命中。
    app.clear("id:content-search-input").unwrap();
    app.type_text("id:content-search-input", "Sample").unwrap();
    app.wait_for("无匹配结果").unwrap();
    app.wait_for_gone("id:content-search-hit-0", Duration::from_secs(2))
        .unwrap();

    // 4. Escape 关闭模态并交还键盘所有权。
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:content-search-modal", Duration::from_secs(2))
        .unwrap();

    // 5. 再次按目录范围搜索，回车直接打开被限定的源码文件。
    app.right_click("id:project-dir-src").unwrap();
    app.choose_menu_item("在目录中查找").unwrap();
    app.wait_for("id:content-search-modal").unwrap();
    app.type_text("id:content-search-input", "new-greeting").unwrap();
    app.wait_for("id:content-search-hit-0").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for_gone("id:content-search-modal", Duration::from_secs(2))
        .unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();

    // 6. 工作区搜索仍能找到 README 的 Sample（说明第 3 步是范围排除而非全局缺失）。
    app.press_key("mod+shift+f").unwrap();
    app.wait_for("id:content-search-modal").unwrap();
    app.type_text("id:content-search-input", "Sample").unwrap();
    app.wait_for("id:content-search-hit-0").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for_gone("id:content-search-modal", Duration::from_secs(2))
        .unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();

    // 7. Escape 与关闭流程不残留状态。
    app.press_key("escape").unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();
}

#[test]
fn ui_local_history_restores_old_format_one_hunk_and_full_snapshot() {
    use sha1::{Digest, Sha1};

    let _guard = lock_env();
    let fx = Fixture::new("legacy-local-history");
    let file = fx.repo_dir.join("README.md");
    let original = "new-a\nmiddle\nunchanged\nnew-z\n";
    let snapshot = "old-a\nmiddle\nunchanged\nold-z\n";
    fs::write(&file, original).unwrap();

    let bucket = fx.root.join("localhistory")
        .join(format!("{:x}", Sha1::digest(b"sample-repo")))
        .join(format!("{:x}", Sha1::digest(b"README.md")));
    fs::create_dir_all(&bucket).unwrap();
    let id = "01700000000000000000";
    let legacy_content = bucket.join(format!("{id}.txt"));
    fs::write(&legacy_content, snapshot).unwrap();
    fs::write(bucket.join(format!("{id}.json")), serde_json::json!({
        "id": id,
        "repoId": "sample-repo",
        "path": "README.md",
        "at": "2026-11-20T16:00:00.123+08:00",
        "reason": "旧版快照",
        "size": snapshot.len()
    }).to_string()).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.press("id:code-history-btn").unwrap();
    app.wait_for("旧版快照").unwrap();
    app.wait_for("id:local-history-restore-hunk").unwrap();
    app.press("id:local-history-restore-hunk").unwrap();
    app.wait_for("已在编辑器恢复此变更块").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:local-history-modal", Duration::from_secs(2)).unwrap();
    app.press("old-a").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("old-a\nmiddle\nunchanged\nnew-z\n"));

    app.press("id:code-history-btn").unwrap();
    app.wait_for("id:local-history-restore-all").unwrap();
    app.press("id:local-history-restore-all").unwrap();
    app.wait_for("覆盖当前文件？").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), original);
    app.press("id:local-history-confirm-restore").unwrap();
    app.wait_for("已恢复本地历史").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), snapshot);
    assert_eq!(fs::read_to_string(&legacy_content).unwrap(), snapshot);
}

#[test]
fn ui_local_history_reveals_late_changes_before_restoring_the_selected_hunk() {
    use sha1::{Digest, Sha1};

    let _guard = lock_env();
    let fx = Fixture::new("local-history-late-hunks");
    let file = fx.repo_dir.join("README.md");
    let current = (1..=270).map(|line| format!("current {line:03}\n")).collect::<String>();
    fs::write(&file, &current).unwrap();
    let bucket = fx.root.join("localhistory")
        .join(format!("{:x}", Sha1::digest(b"sample-repo")))
        .join(format!("{:x}", Sha1::digest(b"README.md")));
    fs::create_dir_all(&bucket).unwrap();
    for (id, reason, snapshot) in [
        ("01700000000000000002", "两处改动的快照", current.replace("current 115", "snapshot first").replace("current 225", "snapshot last")),
        ("01700000000000000001", "一处改动的快照", current.replace("current 170", "snapshot alternate")),
    ] {
        fs::write(bucket.join(format!("{id}.txt")), &snapshot).unwrap();
        fs::write(bucket.join(format!("{id}.json")), serde_json::json!({
            "id": id, "repoId": "sample-repo", "path": "README.md",
            "at": format!("2026-11-20T16:00:0{}+08:00", &id[id.len() - 1..]),
            "reason": reason, "size": snapshot.len(),
        }).to_string()).unwrap();
    }

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.press("id:code-history-btn").unwrap();
    app.wait_for("id:local-history-snapshot >> text:snapshot first").unwrap();
    app.wait_for("id:local-history-current >> text:current 115").unwrap();
    assert!(!app.screen().enabled("id:local-history-hunk-prev").unwrap());
    app.scroll("id:local-history-current", px(2_100.0)).unwrap();
    app.wait_for("id:local-history-modal >> text:1 / 2").unwrap();
    app.press("id:local-history-current >> text:current 225").unwrap();
    app.wait_for("id:local-history-modal >> text:2 / 2").unwrap();
    assert!(!app.screen().enabled("id:local-history-hunk-next").unwrap());
    app.press_key("shift+f7").unwrap();
    app.wait_for("id:local-history-current >> text:current 115").unwrap();
    app.press_key("f7").unwrap();
    app.wait_for("id:local-history-snapshot >> text:snapshot last").unwrap();
    app.wait_for("id:local-history-current >> text:current 225").unwrap();
    assert!(!app.screen().enabled("id:local-history-hunk-next").unwrap());
    app.press("id:local-history-current >> text:current 224").unwrap();
    assert!(!app.screen().enabled("id:local-history-restore-hunk").unwrap());
    app.press_key("f7").unwrap();
    app.wait_for("id:local-history-modal >> text:2 / 2").unwrap();
    app.press("id:local-history-snapshot >> text:snapshot last").unwrap();
    app.press("id:local-history-restore-hunk").unwrap();
    app.wait_for("已在编辑器恢复此变更块").unwrap();
    app.wait_for("id:local-history-snapshot >> text:snapshot first").unwrap();
    app.press("一处改动的快照").unwrap();
    app.wait_for("id:local-history-snapshot >> text:snapshot alternate").unwrap();
    app.wait_for("id:local-history-current >> text:current 170").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:local-history-modal", Duration::from_secs(2)).unwrap();
    app.press("id:code-editor-view >> text:current 001").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    let restored = current.replace("current 225", "snapshot last");
    assert_eq!(app.screen().clipboard(), Some(restored.as_str()));
    assert_eq!(fs::read_to_string(&file).unwrap(), current);
}

#[test]
fn ui_working_diff_local_history_restore_is_undoable() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-local-history-undo");
    let path = fx.repo_dir.join("README.md");
    let original = "before-a\nmiddle\nunchanged\nbefore-z\n";
    let snapshot = "history-a\nmiddle\nunchanged\nhistory-z\n";
    fs::write(&path, original).unwrap();
    nemorous::local_history::LocalHistoryStore::new(fx.root.join("local-history"))
        .record("sample-repo", "README.md", "历史撤销测试", snapshot.as_bytes())
        .unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-README.md").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:before-a").unwrap();
    let open_history = |app: &mut Headless| {
        app.press_key("mod+shift+a").unwrap();
        app.type_text("id:navigation-picker-input", "本地历史").unwrap();
        app.submit("id:navigation-picker-input").unwrap();
        app.wait_for("历史撤销测试").unwrap();
    };

    open_history(&mut app);
    app.wait_for("id:local-history-current >> text:before-a").unwrap();
    app.press("id:local-history-current >> text:before-a").unwrap();
    app.press("id:local-history-restore-hunk").unwrap();
    app.wait_for("已在编辑器恢复此变更块").unwrap();
    app.press_key("mod+z").unwrap();
    app.press_key("mod+s").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:local-history-modal", Duration::from_secs(2)).unwrap();
    assert!(app.screen().shows("history-a"));
    assert!(app.screen().shows("before-z"));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
    assert!(app.screen().shows("before-a"));
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    assert!(app.screen().shows("history-a"));
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();

    open_history(&mut app);
    app.press("id:local-history-restore-all").unwrap();
    app.press("id:local-history-confirm-restore").unwrap();
    app.wait_for("已恢复本地历史").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:local-history-modal", Duration::from_secs(2)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), snapshot);
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    assert!(app.screen().shows("before-a"));
    assert!(app.screen().shows("before-z"));
    assert_eq!(fs::read_to_string(&path).unwrap(), snapshot);
    app.press_key("mod+shift+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
    assert!(app.screen().shows("history-a"));
    assert!(app.screen().shows("history-z"));
}

#[test]
fn ui_rebind_shortcuts_rejects_conflicts_and_survives_restart() {
    let _guard = lock_env();
    let fx = Fixture::new("keymap-settings");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    config["keyboard_shortcuts"] = serde_json::json!({
        "goToLine": [{"modifiers": 9, "key": 61}],
        "otherAction": [{"modifiers": 8, "key": 49}]
    });
    fs::write(&fx.config_file, serde_json::to_vec(&config).unwrap()).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press_key("mod+l").unwrap();
    assert!(!app.screen().matches("id:goto-line-modal").unwrap());
    app.press_key("mod+shift+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.press_key("escape").unwrap();

    app.press_key("mod+,").unwrap();
    app.press("id:settings-keymap").unwrap();
    app.wait_for("id:keymap-settings").unwrap();
    app.type_text("id:keymap-filter", "goToLine").unwrap();
    app.press("id:keymap-action-goToLine").unwrap();
    app.clear("id:keymap-draft").unwrap();
    app.type_text("id:keymap-draft", "mod+s").unwrap();
    app.press("id:keymap-save").unwrap();
    app.wait_for("已被「保存」使用").unwrap();
    app.clear("id:keymap-draft").unwrap();
    app.type_text("id:keymap-draft", "mod+alt+g").unwrap();
    app.press("id:keymap-save").unwrap();
    app.wait_for("已保存快捷键").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:keymap-settings", Duration::from_secs(2)).unwrap();
    app.press_key("mod+alt+g").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    drop(app);

    let mut app = fx.start_app(1440.0, 900.0);
    app.wait_for("id:nemorous-root").unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press_key("mod+alt+g").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.press_key("escape").unwrap();
    app.press_key("mod+,").unwrap();
    app.press("id:settings-keymap").unwrap();
    app.type_text("id:keymap-filter", "goToLine").unwrap();
    app.press("id:keymap-action-goToLine").unwrap();
    app.press("id:keymap-reset").unwrap();
    app.wait_for("已恢复默认快捷键").unwrap();
    app.press_key("escape").unwrap();
    app.press_key("mod+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert!(config["keyboard_shortcuts"]["goToLine"].is_null());
    assert_eq!(config["keyboard_shortcuts"]["otherAction"][0]["key"], 49);
}

#[test]
fn ui_mac_control_g_adds_next_occurrence_as_an_editable_selection() {
    let _guard = lock_env();
    let fx = Fixture::new("next-occurrence");
    fs::write(fx.repo_dir.join("src/app.go"), "package main\nfunc x() {}\nfunc y() {}\n").unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:2:2").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press("func x() {}").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "2:2").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("2:2").unwrap();
    app.press_key("ctrl+g").unwrap();
    app.press_key("ctrl+g").unwrap();
    app.press_key("z").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("package main\nz x() {}\nz y() {}\n"));
}

#[test]
fn ui_compare_pre_rename_revision_with_unsaved_source() {
    let _guard = lock_env();
    let fx = Fixture::new("revision-picker-rename");
    let working = fx.repo_dir.join("src/rename_new.go");
    let original = fs::read(&working).unwrap();
    let hash = Command::new("git").current_dir(&fx.repo_dir)
        .args(["rev-parse", "--short", "HEAD^"]).output().unwrap();
    let old_hash = String::from_utf8(hash.stdout).unwrap();
    let date = Command::new("git").current_dir(&fx.repo_dir)
        .args(["show", "-s", "--format=%aI", "HEAD^"]).output().unwrap();
    assert!(date.status.success());
    let authored_at = String::from_utf8(date.stdout).unwrap();
    let authored_at = chrono::DateTime::parse_from_rfc3339(authored_at.trim()).unwrap()
        .with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M");

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/rename_new.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press_key("mod+r").unwrap();
    app.type_text("id:code-find-input", "v2").unwrap();
    app.type_text("id:code-replace-input", "edited-v2").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("未保存").unwrap();
    app.press_key("escape").unwrap();

    app.right_click(r#"const Version = "edited-v2""#).unwrap();
    app.choose_menu_item("Git > 与修订版本比较…").unwrap();
    app.wait_for(&format!("id:file-revision-{} >> text:{} · Tester · {authored_at} · src/rename_old.go", old_hash.trim(), old_hash.trim())).unwrap();
    app.press_key("down").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("id:comparison-pane").unwrap();
    app.wait_for(&format!("{} · src/rename_old.go", old_hash.trim())).unwrap();
    app.wait_for(r#"const Version = "v1""#).unwrap();
    app.wait_for(r#"const Version = "edited-v2""#).unwrap();
    assert_eq!(fs::read(&working).unwrap(), original);
}

#[test]
fn ui_search_everywhere_actions_recent_files_and_switcher() {
    let _guard = lock_env();
    let fx = Fixture::new("navigation-picker");
    let mut app = fx.start_headless(1440.0, 900.0);

    app.press("id:top-search-btn").unwrap();
    app.wait_for("随处搜索").unwrap();
    app.press("文件").unwrap();
    app.type_text("id:navigation-picker-input", "rename_new.go").unwrap();
    app.submit("id:navigation-picker-input").unwrap();
    app.wait_for("id:editor-tab-file-src/rename_new.go").unwrap();

    app.press_key("mod+shift+a").unwrap();
    app.wait_for("查找操作").unwrap();
    app.type_text("id:navigation-picker-input", "在文件中查找").unwrap();
    app.submit("id:navigation-picker-input").unwrap();
    app.wait_for("id:content-search-input").unwrap();
    app.press_key("escape").unwrap();

    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md:2").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("2:1").unwrap();
    app.press_key("ctrl+tab").unwrap();
    app.wait_for("切换器").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for_gone("id:navigation-picker-modal", Duration::from_secs(2)).unwrap();
    app.wait_for("1:1").unwrap();

    app.press_key("mod+e").unwrap();
    app.wait_for("最近文件").unwrap();
    app.type_text("id:navigation-picker-input", "README.md").unwrap();
    app.submit("id:navigation-picker-input").unwrap();
    app.wait_for("2:1").unwrap();
}

#[test]
fn ui_compare_selected_project_files_and_clipboard_reads_unsaved_text() {
    let _guard = lock_env();
    let fx = Fixture::new("file-comparison");
    let path = fx.repo_dir.join("src/app.go");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        fx.repo_dir.join("README.md"),
        original.replace("new-greeting", "dirty-comparison").replace('\t', "  "),
    ).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press_key("mod+r").unwrap();
    app.type_text("id:code-find-input", "new-greeting").unwrap();
    app.type_text("id:code-replace-input", "dirty-comparison").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("未保存").unwrap();
    app.press_key("escape").unwrap();
    app.press_key("alt+f1").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    app.press("id:project-file-src/app.go").unwrap();
    app.hold_keys("cmd", true).unwrap();
    app.press("id:project-file-README.md").unwrap();
    app.hold_keys("cmd", false).unwrap();
    app.hold_keys("cmd", true).unwrap();
    app.press("id:project-file-README.md").unwrap();
    app.hold_keys("cmd", false).unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("app.go"));
    app.hold_keys("cmd", true).unwrap();
    app.press("id:project-file-README.md").unwrap();
    app.hold_keys("cmd", false).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:comparison-pane").unwrap();
    assert!(app.screen().matches("id:comparison-left-editor").unwrap());
    app.wait_for("id:comparison-pane >> text:1/2").unwrap();
    app.press("id:comparison-whitespace").unwrap();
    app.choose_menu_item("忽略首尾空白").unwrap();
    app.wait_for("变更 0").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    app.press_key("mod+w").unwrap();
    app.wait_for_gone("id:comparison-pane", Duration::from_secs(2)).unwrap();
    app.set_clipboard(Some("clipboard snapshot\n".to_string()));
    app.right_click(r#"return "dirty-comparison""#).unwrap();
    app.choose_menu_item("与剪贴板比较").unwrap();
    app.wait_for("id:comparison-pane").unwrap();
    app.wait_for("clipboard snapshot").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn ui_manual_comparison_reveals_and_navigates_late_differences() {
    let _guard = lock_env();
    let fx = Fixture::new("file-comparison-navigation");
    let current = (1..=270).map(|line| format!("current {line:03}\n")).collect::<String>();
    fs::write(fx.repo_dir.join("README.md"), &current).unwrap();
    let compared = current.replace("current 112", " current 112").replace("current 224", "clipboard last");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.set_clipboard(Some(compared));
    app.right_click("current 001").unwrap();
    app.choose_menu_item("与剪贴板比较").unwrap();
    app.wait_for("id:comparison-right-editor >> text:current 112").unwrap();
    app.wait_for("id:comparison-pane >> text:1/2").unwrap();
    assert!(!app.screen().enabled("id:comparison-prev-change").unwrap());
    app.press_key("mod+f").unwrap();
    app.wait_for("id:comparison-pane >> id:diff-find-input").unwrap();
    app.press_key("c").unwrap();
    assert_eq!(app.screen().input_text("id:diff-find-input").unwrap(), Some("c"));
    app.clear("id:diff-find-input").unwrap();
    app.type_text("id:diff-find-input", "current 224").unwrap();
    app.wait_for("查找右侧").unwrap();
    app.press_key("mod+g").unwrap();
    app.wait_for("id:comparison-right-editor >> text:current 224").unwrap();
    app.wait_for("id:diff-find-bar >> text:1/1").unwrap();
    app.press("id:comparison-left-editor >> text:/current 112/").unwrap();
    app.press_key("mod+f").unwrap();
    app.wait_for("查找左侧").unwrap();
    app.wait_for("id:diff-find-bar >> text:0 个结果").unwrap();
    app.clear("id:diff-find-input").unwrap();
    app.type_text("id:diff-find-input", "CLIPBOARD LAST").unwrap();
    app.wait_for("id:diff-find-bar >> text:1 个结果").unwrap();
    app.press("id:diff-find-case").unwrap();
    app.wait_for("id:diff-find-bar >> text:0 个结果").unwrap();
    app.press("id:diff-find-case").unwrap();
    app.press_key("mod+g").unwrap();
    app.wait_for("id:comparison-left-editor >> text:clipboard last").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:diff-find-bar", Duration::from_secs(2)).unwrap();
    app.press_key("shift+f7").unwrap();
    app.wait_for("id:comparison-pane >> text:1/2").unwrap();
    assert_eq!(fs::read_to_string(fx.repo_dir.join("README.md")).unwrap(), current);
    app.press("id:comparison-next-change").unwrap();
    app.wait_for("id:comparison-left-editor >> text:clipboard last").unwrap();
    app.wait_for("id:comparison-right-editor >> text:current 224").unwrap();
    app.wait_for("id:comparison-pane >> text:2/2").unwrap();
    assert!(!app.screen().enabled("id:comparison-next-change").unwrap());
    app.press_key("shift+f7").unwrap();
    app.wait_for("id:comparison-right-editor >> text:current 112").unwrap();
    app.scroll("id:comparison-right-editor", px(2_100.0)).unwrap();
    app.press("id:comparison-right-editor >> text:current 224").unwrap();
    app.wait_for("id:comparison-pane >> text:2/2").unwrap();
    app.press("id:comparison-prev-change").unwrap();
    app.wait_for("id:comparison-right-editor >> text:current 112").unwrap();
    app.press("id:comparison-whitespace").unwrap();
    app.choose_menu_item("忽略首尾空白").unwrap();
    app.wait_for("id:comparison-left-editor >> text:clipboard last").unwrap();
    app.wait_for("id:comparison-pane >> text:1/1").unwrap();
    app.press("id:comparison-find-btn").unwrap();
    app.wait_for("查找右侧").unwrap();
    app.wait_for("id:diff-find-bar >> text:0 个结果").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:diff-find-bar", Duration::from_secs(2)).unwrap();
}

#[test]
fn ui_manual_comparison_stops_at_a_tail_hunk_on_the_shorter_side() {
    let _guard = lock_env();
    let fx = Fixture::new("comparison-tail-hunk");
    let short = (1..=185).map(|line| format!("context {line:03}\n")).collect::<String>() + "tail 🐈";
    let long = short.replace("context 071", "EARLIER_CHANGE") + "\nCLIPBOARD_TAIL\n";
    fs::write(fx.repo_dir.join("README.md"), &short).unwrap();
    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.diff_whitespace_mode = "trim".into();
    nemorous::config::save_to(&fx.config_file, &config).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.set_clipboard(Some(long));
    app.wait_for("id:code-editor-view >> text:context 001").unwrap();
    app.right_click("id:code-editor-view >> text:context 001").unwrap();
    app.choose_menu_item("与剪贴板比较").unwrap();
    app.wait_for("id:comparison-right-editor >> text:context 071").unwrap();
    app.press("id:comparison-right-editor >> text:context 071").unwrap();
    app.press_key("f7").unwrap();
    app.wait_for("id:comparison-left-editor >> text:CLIPBOARD_TAIL").unwrap();
    app.wait_for("id:comparison-pane >> text:2/2").unwrap();
    assert!(!app.screen().enabled("id:comparison-next-change").unwrap());
    app.press_key("shift+f7").unwrap();
    app.wait_for("id:comparison-left-editor >> text:EARLIER_CHANGE").unwrap();
}

#[test]
fn ui_format_file_and_format_on_save_use_checked_writes() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("format-on-save");
    let formatter = fx.root.join("formatter fixture.sh");
    fs::write(
        &formatter,
        "#!/bin/sh\ncat > \"$1\" <<'FORMATTED'\n# Formatted\nsecond line\nFORMATTED\n",
    )
    .unwrap();
    fs::set_permissions(&formatter, fs::Permissions::from_mode(0o700)).unwrap();
    let path = fx.repo_dir.join("README.md");
    fs::write(&path, "# Sample\r\nline 2\r\n").unwrap();
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    config["format_command"] = serde_json::json!(format!("'{}' {{}}", formatter.display()));
    fs::write(&fx.config_file, serde_json::to_vec(&config).unwrap()).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press_key("mod+alt+l").unwrap();
    app.wait_for("已格式化 README.md").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"# Formatted\r\nsecond line\r\n");

    app.right_click("# Formatted").unwrap();
    app.choose_menu_item("保存时自动格式化").unwrap();
    app.press_key("mod+r").unwrap();
    app.type_text("id:code-find-input", "Formatted").unwrap();
    app.type_text("id:code-replace-input", "Edited").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("未保存").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"# Formatted\r\nsecond line\r\n");
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("未保存", Duration::from_secs(4)).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"# Formatted\r\nsecond line\r\n");
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert_eq!(settings["format_on_save"], true);
}

#[cfg(unix)]
#[test]
fn ui_working_diff_auto_format_keeps_draft_undo_in_both_layouts() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let formatted = "package main\n\n// FORMAT_RESULT\n";
    let marker = "USER_FORMAT_DRAFT";
    for single in [false, true] {
        let fx = Fixture::new("diff-format-undo");
        let formatter = fx.root.join("formatter.sh");
        fs::write(&formatter, format!("#!/bin/sh\ncat > \"$1\" <<'FORMATTED'\n{formatted}FORMATTED\n"))
            .unwrap();
        fs::set_permissions(&formatter, fs::Permissions::from_mode(0o700)).unwrap();
        let mut config: serde_json::Value = serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
        config["format_command"] = serde_json::json!(format!("'{}' {{}}", formatter.display()));
        config["format_on_save"] = serde_json::json!(true);
        fs::write(&fx.config_file, serde_json::to_vec(&config).unwrap()).unwrap();

        let path = fx.repo_dir.join("src/app.go");
        let mut app = fx.start_headless(1440.0, 900.0);
        let right = "id:working-diff-new-editor";
        app.press("id:change-row-unstaged-src/app.go").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for(right).unwrap();
        app.type_text(right, marker).unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        if single {
            app.press("id:diff-layout-toggle").unwrap();
            app.wait_for("id:working-diff-unified-editor").unwrap();
        }

        app.press_key("mod+s").unwrap();
        app.wait_for("已格式化 src/app.go").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), formatted);
        if single {
            app.press("id:diff-layout-toggle").unwrap();
            app.wait_for(right).unwrap();
        }
        app.press("id:diff-new-pane-bar").unwrap();
        app.press_key("mod+z").unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        assert!(app.screen().texts().iter().any(|line| line.contains(marker)));
        app.press_key("mod+z").unwrap();
        assert!(!app.screen().texts().iter().any(|line| line.contains(marker)));
        assert!(app.screen().texts().iter().any(|line| line.contains("new-greeting")));
        assert_eq!(fs::read_to_string(&path).unwrap(), formatted);
        app.press_key("mod+shift+z").unwrap();
        assert!(app.screen().texts().iter().any(|line| line.contains(marker)));
        app.press_key("mod+shift+z").unwrap();
        app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
        assert!(app.screen().shows("// FORMAT_RESULT"));
        assert_eq!(fs::read_to_string(&path).unwrap(), formatted);
    }
}

#[test]
fn ui_editor_split_shares_text_undo_and_distinct_tab_groups() {
    let _guard = lock_env();
    let fx = Fixture::new("code-split");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:3").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("3:1").unwrap();
    app.right_click("id:editor-tab-file-src/app.go").unwrap();
    app.choose_menu_item("向右拆分").unwrap();
    app.wait_for("id:split-editor-left").unwrap();
    app.wait_for("id:split-editor-right").unwrap();
    let left = "id:split-tab-left-file::sample-repo::src/app.go";
    let right = "id:split-tab-right-file::sample-repo::src/app.go";

    app.press(right).unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "7").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("7:1").unwrap();
    app.press(left).unwrap();
    app.wait_for("3:1").unwrap();
    app.press_key("z").unwrap();
    app.press(right).unwrap();
    app.wait_for("7:1").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert!(app.screen().clipboard().unwrap_or_default().contains("zfunc hello"));

    app.press_key("mod+z").unwrap();
    app.press(left).unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert!(!app.screen().clipboard().unwrap_or_default().contains("zfunc hello"));
    app.press_key("mod+shift+z").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert!(app.screen().clipboard().unwrap_or_default().contains("zfunc hello"));

    app.press(right).unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md:2").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:split-tab-right-file::sample-repo::README.md").unwrap();
    app.wait_for("id:split-tab-left-file::sample-repo::src/app.go").unwrap();
    assert!(!app.screen().matches("id:split-tab-left-file::sample-repo::README.md").unwrap());
    app.press(left).unwrap();
    app.wait_for("text:line 2").unwrap();
    app.right_click("text:line 2").unwrap();
    app.choose_menu_item("Git > 查看文件历史").unwrap();
    app.wait_for("文件历史：README.md").unwrap();
    app.press_key("mod+9").unwrap();
    app.wait_for_gone("id:git-file-history", Duration::from_secs(2)).unwrap();
    app.press("id:split-code-merge").unwrap();
    app.wait_for_gone("id:split-editor-right", Duration::from_secs(2)).unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();
    app.wait_for("2:1").unwrap();
}

#[test]
fn ui_workspace_search_prefills_cross_line_selection() {
    let _guard = lock_env();
    let fx = Fixture::new("multiline-search");
    let mut app = fx.start_headless(1040.0, 660.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press("# Sample").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("# Sample\nline 2\n"));
    app.press_key("mod+shift+f").unwrap();
    app.wait_for("id:content-search-multiline").unwrap();
    app.wait_for("1 条匹配").unwrap();
    app.wait_for("id:content-search-group-README.md").unwrap();

    app.press_key("mod+enter").unwrap();
    app.wait_for_gone("id:content-search-modal", Duration::from_secs(2)).unwrap();
    app.wait_for("1:1").unwrap();

    app.press_key("mod+shift+f").unwrap();
    app.press("id:content-search-multiline").unwrap();
    app.type_text("id:content-search-input", "package main\n\nfunc hello").unwrap();
    app.wait_for("id:content-search-group-src/app.go").unwrap();
}

#[test]
fn ui_line_bookmarks_persist_jump_and_remove_by_repository() {
    let _guard = lock_env();
    let fx = Fixture::new("line-bookmarks");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    config["bookmarks"] = serde_json::json!([
        {"repo_id":"other", "rel_path":"src/app.go", "line":7, "note":"keep"}
    ]);
    fs::write(&fx.config_file, serde_json::to_vec(&config).unwrap()).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:7").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("7:1").unwrap();
    app.press_key("mod+shift+b").unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert_eq!(config["bookmarks"][1]["repo_id"], "sample-repo");
    assert_eq!(config["bookmarks"][1]["line"], 7);
    drop(app);

    let mut app = fx.start_app(1440.0, 900.0);
    app.wait_for("id:nemorous-root").unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:3").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("3:1").unwrap();
    app.right_click(r#"return "new-greeting""#).unwrap();
    app.choose_menu_item("书签…").unwrap();
    app.wait_for("id:bookmarks-modal").unwrap();
    app.type_text("id:bookmarks-filter", "foo").unwrap();
    app.wait_for("暂无书签").unwrap();
    app.clear("id:bookmarks-filter").unwrap();
    app.type_text("id:bookmarks-filter", "src/app.go").unwrap();
    app.press("id:bookmark-jump-sample-repo-src/app.go-7").unwrap();
    app.wait_for("7:1").unwrap();

    app.right_click(r#"return "new-greeting""#).unwrap();
    app.choose_menu_item("书签…").unwrap();
    app.press("id:bookmark-remove-sample-repo-src/app.go-7").unwrap();
    app.wait_for("暂无书签").unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert_eq!(config["bookmarks"].as_array().unwrap().len(), 1);
    assert_eq!(config["bookmarks"][0]["note"], "keep");
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:bookmarks-modal", Duration::from_secs(2)).unwrap();
}

#[test]
fn ui_legacy_workspace_session_restores_and_persists_after_reopen() {
    let _guard = lock_env();
    let fx = Fixture::new("legacy-session");
    let workspace = fx.repo_dir.to_str().unwrap();
    let path = nemorous::session::session_path_beside(&fx.config_file, workspace);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let saved = serde_json::json!({
        "workspace": workspace,
        "openFiles": [
            {"repoId": "sample-repo", "path": "missing.go", "line": 3},
            {"repoId": "sample-repo", "path": "README.md", "line": 2},
            {"repoId": "sample-repo", "path": "src/app.go", "line": 7}
        ],
        "activeFile": 1,
        "activeTab": "code",
        "showGitPanel": true,
        "changesSplit": 360,
        "codeSplit": 310,
        "pluginPanel": "old-plugin"
    });
    fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();

    let mut app = fx.start_app(1440.0, 900.0);
    app.wait_for("id:project-pane").unwrap();
    app.wait_for("id:git-bottom-pane").unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();
    app.wait_for("2:1").unwrap();
    assert!(!app.screen().matches("id:editor-tab-file-missing.go").unwrap());

    app.press("id:editor-tab-file-src/app.go").unwrap();
    app.wait_for("7:1").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "4").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("4:1").unwrap();

    let mut saved = nemorous::session::load(&path);
    for _ in 0..100 {
        if saved.open_files.len() == 2 && saved.open_files[1].line == 4 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        let _ = app.screen();
        saved = nemorous::session::load(&path);
    }
    assert_eq!(saved.open_files.len(), 2);
    assert_eq!(saved.open_files[1].line, 4);
    assert_eq!(saved.active_file, Some(1));
    assert_eq!(saved.extra["pluginPanel"], "old-plugin");
    assert_eq!(saved.changes_split, 360.0);
    assert_eq!(saved.code_split, 310.0);
    assert!(saved.show_git_panel);
    drop(app);

    let mut app = fx.start_app(1440.0, 900.0);
    app.wait_for("id:editor-tab-file-README.md").unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();
    app.wait_for("id:git-bottom-pane").unwrap();
    app.wait_for("4:1").unwrap();
}

#[cfg(unix)]
#[test]
fn ui_workspace_startup_restores_files_while_git_status_is_pending() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("workspace-startup");
    let path = nemorous::session::session_path_beside(&fx.config_file, fx.repo_dir.to_str().unwrap());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&serde_json::json!({
        "activeTab": "code",
        "openFiles": [{"repoId": "sample-repo", "path": "README.md", "line": 2}]
    })).unwrap()).unwrap();

    let bin = fx.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(&git, r#"#!/bin/sh
if [ "$3" = status ]; then
    : > "../status-started-$$"
    count=0
    while [ ! -f ../status-release ] && [ "$count" -lt 250 ]; do
        sleep 0.02
        count=$((count + 1))
    done
    : > "../status-finished-$$"
fi
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(git, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut env = Environment::default();
    env.os = Os::MacOs;
    env.idiom = Idiom::Desktop;
    env.apple_keyboard = true;
    env.lifecycle = Lifecycle::Inactive;
    let mut app = fx.start_app_in(1440.0, 900.0, env);
    let markers = |prefix: &str| {
        fs::read_dir(&fx.root).unwrap().flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix)).count()
    };
    let limit = std::time::Instant::now() + Duration::from_secs(3);
    while markers("status-started-") == 0 {
        assert!(std::time::Instant::now() < limit, "status did not start");
        app.wait(Duration::from_millis(20)).unwrap();
    }
    app.set_environment("window activated", |env| {
        env.lifecycle = Lifecycle::Active;
        env.window_state.get_or_insert_with(Default::default).focused = true;
    }).unwrap();
    app.set_environment("window scale reported", |env| {
        env.window_state.as_mut().unwrap().scale_factor += 0.25;
    }).unwrap();

    app.wait_for("id:project-file-README.md").unwrap();
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    assert_eq!(markers("status-started-"), 1, "startup focus duplicated the scan");
    assert_eq!(markers("status-finished-"), 0, "the source should appear ahead of Git status");

    app.press_key("mod+0").unwrap();
    app.wait_for("正在读取 Git 状态…").unwrap();
    assert!(!app.shows("工作区干净，没有本地变更"));
    fs::write(fx.root.join("status-release"), "").unwrap();
    app.wait_for("id:change-row-unstaged-src/app.go").unwrap();
}

#[cfg(unix)]
#[test]
fn ui_workspace_file_read_failure_keeps_the_tree_and_retries() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("workspace-file-retry");
    let bin = fx.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(&git, r#"#!/bin/sh
if [ "$3" = ls-files ] && [ -f ../fail-file-list ]; then
    echo 'controlled file listing failure' >&2
    exit 1
fi
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(git, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+1").unwrap();
    app.wait_for("id:project-file-README.md").unwrap();

    let failure = fx.root.join("fail-file-list");
    fs::write(&failure, "").unwrap();
    fs::write(fx.repo_dir.join("new-file.txt"), "new").unwrap();
    app.press_key("mod+alt+y").unwrap();
    app.wait_for("id:workspace-read-error").unwrap();
    assert!(app.screen().matches("id:project-file-README.md").unwrap());
    assert!(!app.screen().matches("id:project-file-new-file.txt").unwrap());
    assert!(!app.shows("未发现仓库"));

    fs::remove_file(failure).unwrap();
    app.press("id:workspace-read-retry").unwrap();
    app.wait_for("id:project-file-new-file.txt").unwrap();
    app.wait_for_gone("id:workspace-read-error", Duration::from_secs(2)).unwrap();
}

#[test]
fn ui_replace_in_files_checks_selection_confirmation_and_external_changes() {
    let _guard = lock_env();
    let fx = Fixture::new("replace-in-files");
    let source = fx.repo_dir.join("src/app.go");
    let other = fx.repo_dir.join("src/rename_new.go");
    let readme = fx.repo_dir.join("README.md");
    fs::write(&source, "// 😀 needle needle\r\npackage main\r\n").unwrap();
    fs::write(&other, "// needle\npackage main\n").unwrap();
    fs::write(&readme, "# Sample needle\n").unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+r").unwrap();
    app.wait_for("在文件中替换").unwrap();
    app.type_text("id:content-search-input", "needle").unwrap();
    app.press("id:content-search-mask-toggle").unwrap();
    app.type_text("id:content-search-mask-input", "*.go").unwrap();
    app.type_text("id:content-search-replacement-input", "new").unwrap();
    app.wait_for("3 条匹配").unwrap();
    app.press("id:content-search-hit-1").unwrap();
    app.press("id:replace-selected").unwrap();
    app.wait_for("已替换 1 个文件，共 1 处").unwrap();
    assert_eq!(
        fs::read(&source).unwrap(),
        "// 😀 needle new\r\npackage main\r\n".as_bytes()
    );

    app.wait_for("2 条匹配").unwrap();
    app.press("id:replace-all").unwrap();
    app.wait_for("确认替换 2 个文件，共 2 处？").unwrap();
    assert_eq!(
        fs::read_to_string(&other).unwrap(),
        "// needle\npackage main\n"
    );
    app.press_key("escape").unwrap();
    app.wait_for("已取消替换").unwrap();
    app.wait_for("在文件中替换").unwrap();

    app.press("id:replace-all").unwrap();
    app.wait_for("确认替换 2 个文件，共 2 处？").unwrap();
    fs::write(&other, "// external needle\npackage main\n").unwrap();
    app.press("id:replace-confirm").unwrap();
    app.wait_for("已替换 1 个文件，共 1 处；1 个文件失败（src/rename_new.go: 文件内容自预检后已变化，已跳过）")
        .unwrap();
    assert_eq!(
        fs::read(&source).unwrap(),
        "// 😀 new new\r\npackage main\r\n".as_bytes()
    );
    assert_eq!(
        fs::read_to_string(&other).unwrap(),
        "// external needle\npackage main\n"
    );
    assert_eq!(fs::read_to_string(&readme).unwrap(), "# Sample needle\n");
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:content-search-modal", Duration::from_secs(2))
        .unwrap();
}

#[test]
fn ui_replace_in_files_word_option_controls_disk_replacement() {
    let _guard = lock_env();
    let fx = Fixture::new("replace-whole-word");
    let path = fx.repo_dir.join("README.md");
    fs::write(&path, "token tokenized token_id\r\n").unwrap();

    let mut app = fx.start_headless(1040.0, 660.0);
    app.press_key("mod+shift+r").unwrap();
    app.type_text("id:content-search-input", "token").unwrap();
    app.wait_for("3 条匹配").unwrap();
    app.press("id:content-search-word").unwrap();
    app.wait_for("1 条匹配").unwrap();
    app.type_text("id:content-search-replacement-input", "entry").unwrap();
    app.press("id:replace-all").unwrap();
    app.wait_for("确认替换 1 个文件，共 1 处？").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "token tokenized token_id\r\n");
    app.press("id:replace-confirm").unwrap();
    app.wait_for("已替换 1 个文件，共 1 处").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "entry tokenized token_id\r\n");
}

#[test]
fn ui_project_collapse_survives_refresh_and_filter_then_locate_expands() {
    let _guard = lock_env();
    let fx = Fixture::new("project-collapse");
    let mut app = fx.start_headless(1040.0, 660.0);
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("在文件树中定位").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();

    app.press("id:project-collapse-all").unwrap();
    app.wait_for("id:project-workspace-root").unwrap();
    app.wait_for_gone("id:project-repo-sample-repo", Duration::from_secs(2)).unwrap();
    app.press_key("mod+alt+y").unwrap();
    app.wait_for("id:project-workspace-root").unwrap();
    app.press_key("mod+f").unwrap();
    app.type_text("id:project-filter-input", "app.go").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    app.clear("id:project-filter-input").unwrap();
    app.wait_for_gone("id:project-repo-sample-repo", Duration::from_secs(2)).unwrap();

    app.press("id:project-workspace-root").unwrap();
    app.wait_for("id:project-repo-sample-repo").unwrap();
    assert!(!app.screen().matches("id:project-file-src/app.go").unwrap());
    app.press("id:project-repo-sample-repo").unwrap();
    app.wait_for("id:project-dir-src").unwrap();
    app.press("id:project-dir-toggle-src").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
}

#[test]
fn ui_left_tool_titles_follow_hover_keyboard_and_open_menu() {
    let _guard = lock_env();
    let fx = Fixture::new("tool-window-titles");
    let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    cfg.commit_left_width = 350.0;
    cfg.code_left_width = 350.0;
    nemorous::config::save_to(&fx.config_file, &cfg).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    let bounds = |app: &Headless, id: &str| {
        app.screen().layout_nodes().iter()
            .find(|node| node.test_id.as_deref() == Some(id) && !node.hidden)
            .unwrap_or_else(|| panic!("missing layout for {id}"))
            .bounds
    };
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    for (key, toolbar, scroll, options, hide) in [
        ("mod+0", "changes-title-actions", "changes-tree-scroll", "changes-title-options", "changes-hide"),
        ("mod+1", "project-title-actions", "project-tree-scroll", "project-options-button", "project-hide"),
    ] {
        if key == "mod+1" {
            app.press_key(key).unwrap();
        }
        app.press("id:editor-tab-working-diff").unwrap();
        app.hover("id:working-diff-new-editor").unwrap();
        let actions = format!("id:{toolbar}");
        app.wait_for_gone(&actions, Duration::from_secs(2)).unwrap();
        let tree = bounds(&app, scroll);
        assert_eq!(tree.size.width.0, 350.0);
        app.hover(&format!("id:{scroll}")).unwrap();
        app.wait_for(&actions).unwrap();
        assert_eq!(bounds(&app, scroll), tree);
        app.press(&format!("id:{options}")).unwrap();
        assert!(app.menu().is_some());
        app.hover("id:working-diff-new-editor").unwrap();
        app.wait_for(&actions).unwrap();
        app.close_menu().unwrap();
        app.wait_for_gone(&actions, Duration::from_secs(2)).unwrap();
        assert_eq!(bounds(&app, scroll), tree);
        if key == "mod+0" {
            for id in ["changes-refresh-btn", "changes-rollback-btn", "changes-expand-all", "changes-collapse-all"] {
                app.wait_for(&format!("id:{id}")).unwrap();
            }
        }
        app.press_key(key).unwrap();
        app.wait_for(&actions).unwrap();
        app.press(&format!("id:{hide}")).unwrap();
        app.wait_for_gone(&format!("id:{scroll}"), Duration::from_secs(2)).unwrap();
        app.wait_for("id:working-diff-new-editor").unwrap();
        app.press_key(key).unwrap();
        app.wait_for(&format!("id:{scroll}")).unwrap();
        app.wait_for(&actions).unwrap();
    }
    app.press("id:rail-commit-btn").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: app.go").unwrap();
}

#[test]
fn ui_project_header_locates_and_expands_only_the_selected_branch() {
    let _guard = lock_env();
    let fx = Fixture::new("project-header-actions");
    for (dir, path) in [("src/deep", "src/deep/leaf.go"), ("docs/nested", "docs/nested/guide.md")] {
        fs::create_dir_all(fx.repo_dir.join(dir)).unwrap();
        fs::write(fx.repo_dir.join(path), "fixture\n").unwrap();
    }
    for number in 0..40 {
        fs::write(fx.repo_dir.join(format!("src/a-{number:02}.go")), "// fixture\n").unwrap();
    }
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.press_key("mod+1").unwrap();
    app.press("id:project-locate-current").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    let nodes = app.screen().layout_nodes();
    let bounds = |id| nodes.iter().find(|node| node.test_id.as_deref() == Some(id)).unwrap().bounds;
    let file = bounds("project-file-src/app.go");
    let scroll = bounds("project-tree-scroll");
    assert!(file.origin.y >= scroll.origin.y);
    assert!(file.origin.y + file.size.height <= scroll.origin.y + scroll.size.height);
    app.press("id:project-collapse-all").unwrap();
    app.press("id:project-workspace-root").unwrap();
    app.press("id:project-repo-sample-repo").unwrap();
    app.press("id:project-dir-src").unwrap();
    app.press("id:project-expand-selected").unwrap();
    app.wait_for("id:project-file-src/deep/leaf.go").unwrap();
    assert!(!app.screen().matches("id:project-file-docs/nested/guide.md").unwrap());
    app.press("id:project-collapse-all").unwrap();
    app.wait_for_gone("id:project-file-src/app.go", Duration::from_secs(2)).unwrap();
    app.press_key("mod+f").unwrap();
    app.type_text("id:project-filter-input", "guide.md").unwrap();
    app.press("id:project-locate-current").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    app.press("id:project-hide").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press("id:rail-project-btn").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
}

#[test]
fn ui_commit_toolbar_expands_and_rolls_back_selected_file_with_confirmation() {
    let _guard = lock_env();
    let fx = Fixture::new("commit-toolbar");
    let path = fx.repo_dir.join("src/app.go");
    let before = fs::read(&path).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press("id:changes-collapse-all").unwrap();
    app.wait_for_gone("id:change-row-unstaged-src/app.go", Duration::from_secs(2)).unwrap();
    app.press("id:changes-expand-all").unwrap();
    app.wait_for("id:change-row-unstaged-src/app.go").unwrap();
    app.press("id:changes-refresh-btn").unwrap();
    app.press("id:changes-rollback-btn").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("已回滚文件").unwrap();
    assert!(fs::read_to_string(path).unwrap().contains("old-greeting"));
    assert!(fx.repo_dir.join("notes.txt").is_file());
}

#[test]
fn ui_project_follow_editor_selects_open_file_without_focus_shift() {
    let _guard = lock_env();
    let fx = Fixture::new("project-follow-editor");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 变更行打开 src/app.go，再切到 Project 并打开 README。
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();

    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("在文件树中定位").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    app.right_click("id:project-file-README.md").unwrap();
    app.choose_menu_item("打开源码").unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();

    // 回到 app.go 并折叠 src，为跟随展开做前置。
    app.press("id:editor-tab-file-src/app.go").unwrap();
    app.press("id:project-dir-toggle-src").unwrap();
    app.wait_for_gone("id:project-file-src/app.go", Duration::from_secs(2))
        .unwrap();

    // 开启：立即按活动文件展开祖先并选中，不切换当前工具窗口。
    app.hover("id:project-tree-scroll").unwrap();
    app.press("id:project-options-button").unwrap();
    app.choose_menu_item("行为 > 始终选中打开的文件").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    assert!(app.screen().matches("id:editor-tab-file-src/app.go").unwrap());
    let saved_on: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&fx.config_file).unwrap()).unwrap();
    assert_eq!(saved_on["project_follow_editor"], serde_json::json!(true));

    // 切换标签：跟随 README；折叠 src 后切回 app.go 会重新展开。
    app.press("id:editor-tab-file-README.md").unwrap();
    app.press("id:project-dir-toggle-src").unwrap();
    app.press("id:editor-tab-file-src/app.go").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();

    // 关闭：再次切换标签不再展开/选中。
    app.hover("id:project-tree-scroll").unwrap();
    app.press("id:project-options-button").unwrap();
    app.choose_menu_item("行为 > 始终选中打开的文件").unwrap();
    app.press("id:project-dir-toggle-src").unwrap();
    app.wait_for_gone("id:project-file-src/app.go", Duration::from_secs(2))
        .unwrap();
    app.press("id:editor-tab-file-README.md").unwrap();
    app.press("id:editor-tab-file-src/app.go").unwrap();
    app.wait_for_gone("id:project-file-src/app.go", Duration::from_secs(2))
        .unwrap();

    let saved_off: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&fx.config_file).unwrap()).unwrap();
    assert_eq!(saved_off["project_follow_editor"], serde_json::json!(false));
}

#[test]
fn ui_project_preview_tab_reuse_pin_and_buffer_retention() {
    let _guard = lock_env();
    let fx = Fixture::new("project-preview");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 在文件树中定位 src/app.go，再通过 Project 右键「预览」打开预览标签
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("在文件树中定位").unwrap();
    app.wait_for("id:project-file-src/app.go").unwrap();
    app.right_click("id:project-file-src/app.go").unwrap();
    app.choose_menu_item("预览").unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();

    // 干净的预览标签被下一次预览复用替换
    app.right_click("id:project-file-README.md").unwrap();
    app.choose_menu_item("预览").unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();
    app.wait_for_gone("id:editor-tab-file-src/app.go", Duration::from_secs(2))
        .unwrap();

    // 编辑使预览标签钉住
    app.press_key("mod+alt+f").unwrap();
    app.type_text("id:code-find-input", "Sample").unwrap();
    app.type_text("id:code-replace-input", "PinnedSample").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    app.press_key("escape").unwrap();

    // 再预览另一个文件：已钉住的 README 标签保留，新预览正常打开
    app.right_click("id:project-file-src/app.go").unwrap();
    app.choose_menu_item("预览").unwrap();
    app.wait_for("id:editor-tab-file-src/app.go").unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();

    // 从文件树正常「打开源码」钉住 app.go 标签，不影响 README
    app.right_click("id:project-file-src/app.go").unwrap();
    app.choose_menu_item("打开源码").unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();

    app.right_click("id:project-file-notes.txt").unwrap();
    app.choose_menu_item("预览").unwrap();
    app.wait_for("id:editor-tab-file-notes.txt").unwrap();
    assert!(app.screen().matches("id:editor-tab-file-src/app.go").unwrap());

    // 回到 README 标签，替换后的未保存缓冲仍在
    app.press("id:editor-tab-file-README.md").unwrap();
    app.wait_for("# PinnedSample").unwrap();
}

#[test]
fn ui_working_diff_scope_switch_single_column_and_collapse() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-modes");
    let path = fx.repo_dir.join("src/app.go");
    let working = fs::read_to_string(&path).unwrap();
    let staged = working.replace("new-greeting", "staged-greeting");
    fs::write(&path, &staged).unwrap();
    git_run(&fx.repo_dir, &["add", "src/app.go"]);
    fs::write(&path, &working).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);

    open_working_change(&mut app, "staged", "src/app.go");
    app.wait_for("id:diff-old-pane-bar >> text:HEAD · src/app.go").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/staged-greeting/").unwrap();
    app.wait_for("id:diff-new-pane-bar >> text:只读").unwrap();

    open_working_change(&mut app, "unstaged", "src/app.go");
    app.wait_for("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
    app.wait_for("id:working-diff-old-editor >> text:/staged-greeting/").unwrap();
    for control in ["diff-scope-staged", "diff-scope-unstaged", "diff-save-btn"] {
        assert!(!app.screen().matches(&format!("id:{control}")).unwrap());
    }
    app.type_text("id:working-diff-new-editor", "// draft from Diff\n").unwrap();
    app.wait_for("id:editor-tab-working-diff >> id:diff-dirty-badge").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("draft from Diff"));
    assert_eq!(nemorous::git::run_git_string(&fx.repo_dir, &["show", ":src/app.go"]).unwrap().trim(), staged.trim());

    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.press("id:diff-collapse-btn").unwrap();
    app.press("id:diff-collapse-btn").unwrap();
}

#[test]
fn ui_dirty_tabs_close_all_together_protection() {
    let _guard = lock_env();
    let fx = Fixture::new("dirty-close");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 打开两个文件并分别做查找替换变脏
    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.press_key("mod+alt+f").unwrap();
    app.type_text("id:code-find-input", "new-greeting").unwrap();
    app.type_text("id:code-replace-input", "edited-app").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:editor-tab-file-README.md").unwrap();
    app.press_key("mod+alt+f").unwrap();
    app.clear("id:code-find-input").unwrap();
    app.type_text("id:code-find-input", "Sample").unwrap();
    app.clear("id:code-replace-input").unwrap();
    app.type_text("id:code-replace-input", "EditedSample").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    // 右键标签选“关闭全部” -> 合并弹出保护框“有 2 个未保存文件”
    app.right_click("id:editor-tab-file-README.md").unwrap();
    app.choose_menu_item("关闭全部").unwrap();
    app.wait_for("id:dirty-close-modal").unwrap();
    app.wait_for("有 2 个未保存文件").unwrap();

    // 点击“保存全部” -> 两文件均已受检写盘且弹窗关闭
    app.press("id:dirty-close-save").unwrap();
    app.wait_for_gone("id:dirty-close-modal", Duration::from_secs(2))
        .unwrap();
    assert!(fs::read_to_string(fx.repo_dir.join("src/app.go")).unwrap().contains("edited-app"));
    assert!(fs::read_to_string(fx.repo_dir.join("README.md")).unwrap().contains("EditedSample"));
}

#[test]
fn ui_diff_f7_boundary_confirmation_and_keyboard_mixed_staging() {
    let _guard = lock_env();
    let fx = Fixture::new("f7-stage");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 选中 src/app.go，按 S 暂存 -> 出现“已暂存的更改”分区，再按 U 取消暂存
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("s").unwrap();
    app.wait_for("已暂存的更改").unwrap();
    app.press_key("u").unwrap();
    app.wait_for_gone("已暂存的更改", Duration::from_secs(2))
        .unwrap();

    // 打开 src/app.go Diff（只有 1 处 hunk）：
    // 第一次按 F7 保留当前文件，悬停在差异箭头上才显示下一步。
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.press_key("f7").unwrap();
    assert!(!app.screen().matches("id:toolbar-tooltip").unwrap());
    assert!(!app.screen().matches("Diff: notes.txt").unwrap());
    app.hover("id:diff-next-hunk").unwrap();
    app.wait_for("id:toolbar-tooltip").unwrap();
    app.wait_for("切到下一个文件 (F7)").unwrap();

    // 第二次按 F7 切换到下一个变更文件 notes.txt
    app.press_key("f7").unwrap();
    app.wait_for("Diff: notes.txt").unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn ui_whole_file_stage_keeps_keyboard_and_menu_responsive() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("stage-worker");
    let control = fx.root.join("stage-control");
    let bin = fx.root.join("bin");
    fs::create_dir(&control).unwrap();
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
case " $* " in
    *' add -- src/app.go '*|*' reset HEAD -- src/app.go '*)
        if [ -f ../stage-control/hold ]; then
            printf '%s\n' started >> ../stage-control/started
            count=0
            while [ -f ../stage-control/hold ] && [ "$count" -lt 250 ]; do
                sleep 0.02
                count=$((count + 1))
            done
            /usr/bin/git "$@"
            result=$?
            : > ../stage-control/finished
            exit "$result"
        fi ;;
esac
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    let hold = control.join("hold");
    let started = control.join("started");
    let finished = control.join("finished");
    let unstaged = "id:change-row-unstaged-src/app.go";
    let staged = "id:change-row-staged-src/app.go";
    let cached = || nemorous::git::run_git_string(&fx.repo_dir, &["diff", "--cached", "--name-only"]).unwrap();

    fs::write(&hold, "").unwrap();
    app.press(unstaged).unwrap();
    app.press_key("s").unwrap();
    app.wait_until("the stage command started", Duration::from_secs(4), |_| started.is_file()).unwrap();
    assert!(!finished.is_file(), "the staging shortcut blocked the UI until Git returned");
    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("s").unwrap();
    app.wait_for("其他 Git 操作正在运行").unwrap();
    assert!(cached().is_empty());
    fs::remove_file(&hold).unwrap();
    app.wait_for(staged).unwrap();
    assert_eq!(cached(), "src/app.go\n");
    assert_eq!(fs::read_to_string(&started).unwrap(), "started\n");

    fs::remove_file(&started).unwrap();
    fs::remove_file(&finished).unwrap();
    fs::write(&hold, "").unwrap();
    app.right_click(staged).unwrap();
    app.choose_menu_item("取消暂存").unwrap();
    app.wait_until("the unstage command started", Duration::from_secs(4), |_| started.is_file()).unwrap();
    app.press("id:rail-project-btn").unwrap();
    app.wait_for("id:project-file-README.md").unwrap();
    assert!(!finished.is_file(), "the context menu blocked the UI until Git returned");
    assert_eq!(cached(), "src/app.go\n");
    fs::remove_file(&hold).unwrap();
    app.press("id:rail-commit-btn").unwrap();
    app.wait_for("id:changes-header-title-row").unwrap();
    app.wait_for_gone(staged, Duration::from_secs(4)).unwrap();
    assert!(cached().is_empty());

    let lock = fx.repo_dir.join(".git/index.lock");
    fs::write(&lock, "").unwrap();
    app.press(unstaged).unwrap();
    app.press_key("s").unwrap();
    app.wait_for("text:/暂存操作失败/").unwrap();
    assert!(cached().is_empty());
    fs::remove_file(&lock).unwrap();
    app.press_key("s").unwrap();
    app.wait_for(staged).unwrap();
    assert_eq!(cached(), "src/app.go\n");
}

#[test]
fn ui_whole_file_staging_treats_names_literally_and_unstages_both_rename_paths() {
    let _guard = lock_env();
    let fx = Fixture::new("literal-stage-rename");
    fs::write(fx.repo_dir.join("report[1].txt"), "selected file\n").unwrap();
    fs::write(fx.repo_dir.join("report1.txt"), "keep out of index\n").unwrap();
    git_run(&fx.repo_dir, &["mv", "src/rename_new.go", "src/renamed file.go"]);
    let mut app = fx.start_headless(1440.0, 900.0);
    let cached = || nemorous::git::run_git_string(&fx.repo_dir, &["diff", "--cached", "--name-status"]).unwrap();

    let rename = "id:change-row-staged-src/renamed file.go";
    app.wait_for(rename).unwrap();
    app.right_click(rename).unwrap();
    app.choose_menu_item("取消暂存").unwrap();
    app.wait_for_gone(rename, Duration::from_secs(4)).unwrap();
    assert!(cached().is_empty(), "rename left part of its change staged: {}", cached());
    assert!(!fx.repo_dir.join("src/rename_new.go").exists());
    assert!(fs::read_to_string(fx.repo_dir.join("src/renamed file.go")).unwrap().contains("Version"));

    app.press("id:change-row-unstaged-report[1].txt").unwrap();
    app.press_key("s").unwrap();
    app.wait_for("id:change-row-staged-report[1].txt").unwrap();
    assert_eq!(cached(), "A\treport[1].txt\n");
    app.wait_for("id:change-row-unstaged-report1.txt").unwrap();
}

#[test]
fn ui_deleted_file_save_rejected_and_shows_deleted_conflict_modal() {
    let _guard = lock_env();
    let fx = Fixture::new("deleted-save");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 打开 src/app.go 源码并做轻编辑
    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();

    app.press_key("mod+alt+f").unwrap();
    app.type_text("id:code-find-input", "new-greeting").unwrap();
    app.type_text("id:code-replace-input", "edited-before-delete").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    // 外部删除磁盘文件后按 Cmd+S -> 绝不静默重建，必须弹出“磁盘文件已删除”和“放弃编辑并关闭文件”
    let target = fx.repo_dir.join("src/app.go");
    fs::remove_file(&target).unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.wait_for("磁盘文件已删除").unwrap();
    app.wait_for("放弃编辑并关闭文件").unwrap();
    assert!(!target.exists(), "deleted file must never be silently recreated");

    // 点击“复制我的编辑”验证编辑内容完整写入剪贴板
    app.press("id:conflict-copy-mine-btn").unwrap();
    assert!(
        app.screen()
            .clipboard()
            .unwrap_or("")
            .contains("edited-before-delete")
    );
}

#[test]
fn ui_open_source_keeps_snapshot_and_draft_when_agent_moves_files() {
    let _guard = lock_env();
    let fx = Fixture::new("open-source-after-move");
    let mut app = fx.start_headless(1440.0, 900.0);
    let open = |app: &mut Headless, path: &str| {
        app.press_key("mod+shift+o").unwrap();
        app.type_text("id:review-picker-input", path).unwrap();
        app.submit("id:review-picker-input").unwrap();
    };
    let editor = "id:code-editor-view";
    let external = "id:code-disk-state";
    let readme = fx.repo_dir.join("README.md");
    open(&mut app, "README.md");
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    fs::write(&readme, "# Clean external update\n").unwrap();
    app.wait_for("id:code-editor-view >> text:# Clean external update").unwrap();
    fs::rename(&readme, fx.repo_dir.join("moved.md")).unwrap();
    app.wait_for("id:code-disk-state >> text:磁盘文件已移动或删除").unwrap();
    app.wait_for("id:code-editor-view >> text:# Clean external update").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("# Clean external update\n"));
    app.press_key("z").unwrap();
    app.wait_for("id:code-editor-view >> text:# Clean external update").unwrap();
    assert!(!app.screen().matches("id:code-dirty-badge").unwrap());
    app.press_key("mod+s").unwrap();
    assert!(!readme.exists());
    fs::write(&readme, "# Restored path\n").unwrap();
    app.wait_for("id:code-editor-view >> text:# Restored path").unwrap();
    app.wait_for_gone(external, Duration::from_secs(4)).unwrap();

    let draft_path = fx.repo_dir.join("src/app.go");
    let moved = fx.repo_dir.join("src/app-moved.go");
    open(&mut app, "src/app.go");
    app.wait_for("id:code-editor-view >> text:/new-greeting/").unwrap();
    app.type_text(editor, "// retained review draft\n").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    fs::write(&draft_path, "package main\n// Agent updated on disk\n").unwrap();
    app.wait_for("id:code-disk-state >> text:磁盘文件已修改").unwrap();
    app.wait_for("id:code-editor-view >> text:/retained review draft/").unwrap();
    fs::rename(&draft_path, &moved).unwrap();
    app.wait_for("id:code-disk-state >> text:磁盘文件已移动或删除").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    app.wait_for("id:code-editor-view >> text:/retained review draft/").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.press("id:conflict-copy-mine-btn").unwrap();
    assert!(app.screen().clipboard().unwrap().contains("retained review draft"));
    assert!(!draft_path.exists());
    assert!(fs::read_to_string(moved).unwrap().contains("Agent updated on disk"));
}

#[test]
fn ui_open_worktree_diff_displays_unavailable_disk_without_losing_review() {
    let _guard = lock_env();
    let fx = Fixture::new("open-diff-disk-state");
    let mut app = fx.start_headless(1440.0, 900.0);
    let path = fx.repo_dir.join("src/app.go");
    let moved = fx.repo_dir.join("src/app-moved.go");
    let disk_state = "id:diff-disk-state";
    let editor = "id:working-diff-new-editor";
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/new-greeting/").unwrap();

    fs::rename(&path, &moved).unwrap();
    fs::create_dir(&path).unwrap();
    app.press_key("mod+alt+y").unwrap();
    app.wait_for("id:diff-disk-state >> text:磁盘文件暂不可读").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
    app.press_key("z").unwrap();
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());

    fs::remove_dir(&path).unwrap();
    app.press_key("mod+alt+y").unwrap();
    app.wait_for("id:diff-disk-state >> text:磁盘文件已移动或删除").unwrap();
    app.press("id:diff-details").unwrap();
    assert!(app.menu().unwrap().iter().any(|item| item.label.starts_with("+0 /")));
    app.close_menu().unwrap();
    assert!(!app.screen().matches("id:working-diff-new-editor >> text:/new-greeting/").unwrap());

    fs::rename(&moved, &path).unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
    app.wait_for_gone(disk_state, Duration::from_secs(4)).unwrap();
    app.type_text(editor, "// retained Diff draft\n").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    fs::write(&path, "package main\n// Agent wrote to disk\n").unwrap();
    app.wait_for("id:diff-disk-state >> text:磁盘文件已修改").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/retained Diff draft/").unwrap();
    fs::rename(&path, &moved).unwrap();
    app.wait_for("id:diff-disk-state >> text:磁盘文件已移动或删除").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/retained Diff draft/").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for("id:save-conflict-modal >> text:磁盘文件已删除").unwrap();
    app.press("id:conflict-copy-mine-btn").unwrap();
    assert!(app.screen().clipboard().unwrap().contains("retained Diff draft"));
    assert!(!path.exists());
    assert!(fs::read_to_string(moved).unwrap().contains("Agent wrote to disk"));
}

#[test]
fn ui_diff_preserves_draft_while_the_staged_baseline_is_not_text() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-draft-nontext-baseline");
    let mut app = fx.start_headless(1440.0, 900.0);
    let relative = "src/app.go";
    let path = fx.repo_dir.join(relative);
    let disk_before = fs::read_to_string(&path).unwrap();
    let original = nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", ":src/app.go"]).unwrap();
    let stage = |oid: &str| git_run(&fx.repo_dir, &["update-index", "--cacheinfo", "100644", oid, relative]);
    let make_blob = |name: &str, bytes: &[u8]| {
        let file = fx.root.join(name);
        fs::write(&file, bytes).unwrap();
        nemorous::git::run_git_string(&fx.repo_dir, &["hash-object", "-w", file.to_str().unwrap()]).unwrap()
    };
    let binary = make_blob("binary", b"\0binary index\n");
    let large = make_blob("large", &vec![b'x'; nemorous::git::MAX_REVIEW_BYTES + 1]);
    let editor = "id:working-diff-new-editor";
    let old = "id:working-diff-old-editor >> text:/old-greeting/";
    let draft = "id:working-diff-new-editor >> text:/retained text draft/";
    let badge = "id:diff-disk-state";
    let row = "id:change-row-unstaged-src/app.go";
    app.press(row).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(old).unwrap();
    app.type_text(editor, "// retained text draft\n").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();

    stage(binary.trim());
    app.wait_for("id:diff-disk-state >> text:Git 比较基准为二进制或非 UTF-8 文件").unwrap();
    app.wait_for(draft).unwrap();
    app.wait_for(old).unwrap();
    app.press("id:diff-details").unwrap();
    assert!(!app.menu().unwrap().iter().any(|item| item.label.starts_with('+')));
    app.close_menu().unwrap();
    app.right_click(&format!("{draft} >> nth=0")).unwrap();
    let menu = app.menu().unwrap();
    assert!(!menu.iter().any(|item| matches!(item.label.as_str(), "暂存此块" | "回滚当前块")));
    app.close_menu().unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    assert!(app.screen().clipboard().unwrap().contains("retained text draft"));

    stage(large.trim());
    app.wait_for("id:diff-disk-state >> text:Git 比较基准超过 2 MiB 审阅上限").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), disk_before);
    app.wait_for(draft).unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:/retained text draft/").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for(draft).unwrap();
    stage(original.trim());
    app.wait_for_gone(badge, Duration::from_secs(4)).unwrap();
    app.wait_for(draft).unwrap();
    app.press("id:diff-details").unwrap();
    assert!(app.menu().unwrap().iter().any(|item| item.label.starts_with('+')));
    app.close_menu().unwrap();

    stage(binary.trim());
    app.wait_for("id:diff-disk-state >> text:Git 比较基准为二进制或非 UTF-8 文件").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for("已保存 src/app.go").unwrap();
    app.wait_for("id:diff-blocked-notice").unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("retained text draft"));
    let staged = nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", ":src/app.go"]).unwrap();
    assert_eq!(staged, binary);
    app.press("id:diff-details").unwrap();
    assert!(!app.menu().unwrap().iter().any(|item| item.label.starts_with('+')));
    app.close_menu().unwrap();
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());

    stage(original.trim());
    app.wait_for(draft).unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    app.type_text(editor, "x").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    stage(large.trim());
    app.wait_for("id:diff-disk-state >> text:Git 比较基准超过 2 MiB 审阅上限").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for("id:diff-blocked-notice >> text:文件超过 2 MiB 审阅上限").unwrap();
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), saved);
}

#[test]
fn ui_save_conflict_keeps_draft_if_missing_path_becomes_a_directory() {
    let _guard = lock_env();
    let fx = Fixture::new("source-conflict-directory");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    let contents = "id:code-editor-view >> text:/new-greeting/";
    app.wait_for(contents).unwrap();
    app.type_text("id:code-editor-view", "// protected draft\n").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    let path = fx.repo_dir.join("src/app.go");
    fs::remove_file(&path).unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.wait_for("放弃编辑并关闭文件").unwrap();
    fs::create_dir(&path).unwrap();
    app.press("id:conflict-accept-disk-btn").unwrap();
    app.wait_for("text:/无法核对磁盘版本/").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.press("id:conflict-copy-mine-btn").unwrap();
    assert!(app.screen().clipboard().unwrap().contains("protected draft"));
    app.press("id:conflict-keep-editing-btn").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    app.wait_for("id:code-editor-view >> text:/protected draft/").unwrap();
    assert!(path.is_dir());

    fs::remove_dir(&path).unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for("id:save-conflict-modal").unwrap();
    app.press("id:conflict-accept-disk-btn").unwrap();
    app.wait_for_gone("id:save-conflict-modal", Duration::from_secs(4)).unwrap();
    app.wait_for_gone("id:editor-tab-file-src/app.go", Duration::from_secs(4)).unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
}

#[test]
fn ui_working_diff_right_pane_edit_and_autosave_on_switch() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-right-edit");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 打开 src/app.go 工作区 Diff（右侧可轻编辑）
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();

    // 在右侧工作区编辑器内直接键入轻改内容 -> 出现未保存徽章
    app.type_text("id:working-diff-new-editor", "// diff-inline-edit\n")
        .unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();

    // 切换到另一文件 notes.txt 的 Diff -> 切文件前自动受检保存原右侧修改到磁盘
    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: notes.txt").unwrap();
    let saved = fs::read_to_string(fx.repo_dir.join("src/app.go")).unwrap();
    assert!(
        saved.contains("// diff-inline-edit"),
        "switching working diff file should auto-save right pane edits, got: {saved}"
    );
}

#[test]
fn ui_working_unified_diff_edits_only_local_text_and_shares_undo() {
    let _guard = lock_env();
    let fx = Fixture::new("editable-unified-review");
    let path = fx.repo_dir.join("README.md");
    let staged = "# Staged\r\nline 2\r\n";
    let original = "# Local\r\nline 2\r\n";
    git_run(&fx.repo_dir, &["config", "core.autocrlf", "false"]);
    fs::write(&path, staged).unwrap();
    git_run(&fx.repo_dir, &["add", "README.md"]);
    fs::write(&path, original).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-README.md").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    let single = "id:working-diff-unified-editor";
    app.wait_for(single).unwrap();

    app.press("text:+ # Local").unwrap();
    app.press_key("mod+up").unwrap();
    app.wait_for("text:- # Staged").unwrap();
    app.press("text:- # Staged").unwrap();
    app.type_text(single, "FORBIDDEN").unwrap();
    assert!(app.screen().shows("- # Staged"));
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());

    app.press("text:+ # Local").unwrap();
    app.press_key("mod+a").unwrap();
    let mut paste = nsh_core::PasteEvent::default();
    paste.text = "FORBIDDEN".into();
    app.paste(single, &paste).unwrap();
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
    app.press("text:+ # Local").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "1:8").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.type_text(single, " 🧪").unwrap();
    app.wait_for("text:+ # Local 🧪").unwrap();
    paste.text = "\npasted one\npasted two".into();
    app.paste(single, &paste).unwrap();
    app.wait_for("text:+ pasted two").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "3:5").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.wait_for("3:5").unwrap();
    for (label, mode) in [("按字符", "char"), ("关闭", "none"), ("按词", "word")] {
        app.press("id:diff-options-btn").unwrap();
        app.choose_menu_item(&format!("行内高亮 > {label}")).unwrap();
        assert_eq!(nemorous::config::load_from(&fx.config_file).unwrap().diff_inline_mode, mode);
        assert!(app.screen().shows("+ pasted two"));
        assert!(app.screen().shows("3:5"));
        assert!(app.screen().matches("id:diff-dirty-badge").unwrap());
    }
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("text:+ pasted two", Duration::from_secs(3)).unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:# Local 🧪").unwrap();
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:pasted two").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("text:+ pasted two").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "# Local 🧪\r\npasted one\r\npasted two\r\nline 2\r\n");
    let index = Command::new("git").current_dir(&fx.repo_dir)
        .args(["show", ":README.md"]).output().unwrap();
    assert!(index.status.success());
    assert_eq!(index.stdout, staged.as_bytes());

    open_working_change(&mut app, "staged", "README.md");
    app.wait_for_gone("text:+ pasted two", Duration::from_secs(3)).unwrap();
    app.wait_for("text:+ # Staged").unwrap();
    app.press("text:+ # Staged").unwrap();
    app.type_text(single, "FORBIDDEN").unwrap();
    assert!(app.screen().shows("+ # Staged"));
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());

    app.press_key("mod+9").unwrap();
    app.wait_for("text:feat: initial fixture").unwrap();
    app.press("text:feat: initial fixture").unwrap();
    app.wait_for("id:git-commit-file-README.md").unwrap();
    app.press("id:git-commit-file-README.md").unwrap();
    app.press_key("mod+d").unwrap();
    let historical = "id:commit-diff-unified-editor";
    app.wait_for(historical).unwrap();
    app.type_text(historical, "FORBIDDEN").unwrap();
    assert!(!app.screen().texts().iter().any(|line| line.contains("FORBIDDEN")));
    assert_eq!(fs::read_to_string(&path).unwrap(), "# Local 🧪\r\npasted one\r\npasted two\r\nline 2\r\n");
}

#[test]
fn ui_working_unified_diff_edits_empty_files_and_the_final_line() {
    let _guard = lock_env();
    let fx = Fixture::new("editable-unified-eof");
    let path = fx.repo_dir.join("empty.txt");
    fs::write(&path, b"").unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-empty.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    let single = "id:working-diff-unified-editor";
    app.wait_for(single).unwrap();
    app.type_text(single, "first").unwrap();
    app.wait_for("text:+ first").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("2:1").unwrap();
    app.type_text(single, "second").unwrap();
    app.wait_for("text:+ second").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond");
}

#[test]
fn ui_working_diff_keeps_native_undo_and_redo_across_layouts() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-undo-layouts");
    let mut app = fx.start_headless(1440.0, 900.0);
    let right = "id:working-diff-new-editor";
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(right).unwrap();
    app.type_text(right, "Z").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();

    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:/Z/")
        .unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/Z/")
        .unwrap();
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3))
        .unwrap();

    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for(right).unwrap();
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/Z/")
        .unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3))
        .unwrap();

    app.type_text(right, "Z").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-view >> text:/Z/").unwrap();
    app.press_key("mod+[").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/Z/").unwrap();
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3))
        .unwrap();

    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:untracked note line 1")
        .unwrap();
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+z").unwrap();
    app.press_key("mod+shift+z").unwrap();
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
    app.wait_for("id:working-diff-new-editor >> text:untracked note line 1")
        .unwrap();
}

#[test]
fn ui_working_diff_keeps_each_files_undo_after_switching() {
    let _guard = lock_env();
    for via_history in [false, true] {
        let fx = Fixture::new("diff-undo-file-switch");
        let path = fx.repo_dir.join("src/app.go");
        let original = fs::read_to_string(&path).unwrap();
        let mut app = fx.start_headless(1440.0, 900.0);
        let right = "id:working-diff-new-editor";
        let marker = "NEMOROUS_FILE_UNDO_MARKER";
        app.press("id:change-row-unstaged-src/app.go").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for(right).unwrap();
        app.type_text(right, marker).unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();

        app.press("id:change-row-unstaged-notes.txt").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for("id:working-diff-new-editor >> text:untracked note line 1")
            .unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains(marker));
        app.press("id:diff-new-pane-bar").unwrap();
        app.press_key("mod+z").unwrap();
        app.press_key("mod+shift+z").unwrap();
        assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());

        if via_history {
            app.press_key("mod+[").unwrap();
        } else {
            app.press("id:change-row-unstaged-src/app.go").unwrap();
            app.press_key("mod+d").unwrap();
        }
        wait_for_diff_file(&mut app, "working", "src/app.go");
        app.press("id:diff-new-pane-bar").unwrap();
        app.press_key("mod+z").unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        assert!(!app.screen().texts().iter().any(|line| line.contains(marker)));
        assert_eq!(fs::read_to_string(&path).unwrap(), saved);
        app.press_key("mod+shift+z").unwrap();
        app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
        assert!(app.screen().texts().iter().any(|line| line.contains(marker)));
        app.press_key("mod+z").unwrap();
        app.press_key("mod+s").unwrap();
        app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }
}

#[test]
fn ui_working_diff_clears_a_parked_undo_after_external_replacement() {
    let _guard = lock_env();
    for same_text in [false, true] {
        let fx = Fixture::new("diff-undo-inactive-external-text");
        let path = fx.repo_dir.join("src/app.go");
        let mut app = fx.start_headless(1440.0, 900.0);
        let right = "id:working-diff-new-editor";
        let marker = "NEMOROUS_FILE_UNDO_MARKER";
        app.press("id:change-row-unstaged-src/app.go").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for(right).unwrap();
        app.type_text(right, marker).unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        app.press("id:change-row-unstaged-notes.txt").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for("id:working-diff-new-editor >> text:untracked note line 1")
            .unwrap();

        let replacement_marker = "External replacement while reviewing another file";
        let (expected, external) = if same_text {
            (marker, fs::read_to_string(&path).unwrap())
        } else {
            (replacement_marker, format!("package main\n\n// {replacement_marker}\n"))
        };
        let replacement = fx.repo_dir.join(".git/replacement.txt");
        fs::write(&replacement, &external).unwrap();
        fs::rename(replacement, &path).unwrap();
        app.press_key("mod+[").unwrap();
        wait_for_diff_file(&mut app, "working", "src/app.go");
        app.press("id:diff-new-pane-bar").unwrap();
        for shortcut in ["mod+z", "mod+shift+z"] {
            app.press_key(shortcut).unwrap();
            assert!(!app.screen().matches("id:diff-dirty-badge").unwrap(), "same_text={same_text}");
            assert!(app.screen().texts().iter().any(|line| line.contains(expected)));
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), external);
    }
}

#[test]
fn ui_working_diff_drops_stale_undo_when_external_text_replaces_the_buffer() {
    let _guard = lock_env();
    let marker = "NEMOROUS_UNDO_MARKER";
    for single in [false, true] {
        let fx = Fixture::new("diff-undo-external-text");
        let mut app = fx.start_headless(1440.0, 900.0);
        let right = "id:working-diff-new-editor";
        app.press("id:change-row-unstaged-src/app.go").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for(right).unwrap();
        app.type_text(right, marker).unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        assert!(app.screen().texts().iter().any(|line| line.contains(marker)));
        app.press_key("mod+z").unwrap();
        app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3))
            .unwrap();
        assert!(!app.screen().texts().iter().any(|line| line.contains(marker)));
        if single {
            app.press("id:diff-layout-toggle").unwrap();
            app.wait_for("id:working-diff-unified-editor").unwrap();
        }

        let path = fx.repo_dir.join("src/app.go");
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("new-greeting", "external-greeting");
        fs::write(&path, &text).unwrap();
        app.wait_until("the external text is rendered", Duration::from_secs(4), |screen| {
            screen.texts().iter().any(|line| line.contains("external-greeting"))
        })
            .unwrap();
        if single {
            app.press("id:diff-layout-toggle").unwrap();
            app.wait_for("id:working-diff-new-editor >> text:/external-greeting/")
                .unwrap();
        }
        app.press("id:diff-new-pane-bar").unwrap();
        for shortcut in ["mod+z", "mod+shift+z"] {
            app.press_key(shortcut).unwrap();
            let screen = app.screen();
            let lines = screen.texts();
            assert!(
                lines.iter().any(|line| line.contains("external-greeting")),
                "external text changed after {shortcut}, single={single}"
            );
            assert!(
                !screen.matches("id:diff-dirty-badge").unwrap(),
                "draft changed after {shortcut}, single={single}"
            );
            assert!(
                !lines.iter().any(|line| line.contains(marker)),
                "old draft returned after {shortcut}, single={single}"
            );
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
    }
}

#[test]
fn ui_modal_escape_restores_focus_and_cmd0_cmd1_focus_routing() {
    let _guard = lock_env();
    let fx = Fixture::new("modal-focus-routing");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 选中 Commit 行后打开 ⌘⇧O 弹窗并按 Escape 关闭 -> 焦点必须恢复到 CommitTree 而非滞留 Modal
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:review-picker-modal", Duration::from_secs(2))
        .unwrap();

    // 弹窗关闭后立即按 ⌘D 必须能正常打开工作区 Diff
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // 2. 此时焦点在 Diff 编辑器且左侧 Commit 面板已显示，按 ⌘0 必须切焦点到 Commit 树而不隐藏左侧面板！
    app.press_key("mod+0").unwrap();
    app.wait_for("id:commit-pane").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("app.go"));

    // 2.5 验证 `<input>` 输入框内按 ⌘C 绝不被树复制劫持（选中 notes.txt 后在筛选框内键入并按 ⌘C，剪贴板不得被覆写为 notes.txt 路径）
    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+f").unwrap();
    app.type_text("id:changes-filter-input", "app").unwrap();
    app.press_key("mod+c").unwrap();
    assert_eq!(
        app.screen().clipboard(),
        Some("app.go"),
        "mod+c inside filter <input> must NOT be hijacked by tree copy_for_current_focus"
    );
    app.clear("id:changes-filter-input").unwrap();
    app.press("id:change-row-unstaged-src/app.go").unwrap();

    // 3. 焦点已在 CommitTree，再按 ⌘1 切到 Project 树并保持编辑器打开
    app.press_key("mod+1").unwrap();
    app.wait_for("id:project-pane").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
}

#[test]
fn ui_code_replace_cmd_r_find_next_prev_cmd_g_and_undo_save() {
    let _guard = lock_env();
    let fx = Fixture::new("cmd-r-g-undo");
    let mut app = fx.start_headless(1440.0, 900.0);

    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();

    // 按 ⌘R 打开查找替换条
    app.press_key("mod+r").unwrap();
    app.wait_for("id:code-find-bar").unwrap();
    app.wait_for("id:code-replace-input").unwrap();

    // 验证 ⌘G / ⇧⌘G 真实在多个匹配间前后切换（src/app.go 有 2 处 "return"：第 4 行与第 8 行）
    app.type_text("id:code-find-input", "return").unwrap();
    app.wait_for("2 个结果").unwrap();
    app.press_key("mod+g").unwrap();
    app.wait_for("1/2").unwrap();
    app.wait_for("4:8").unwrap();
    app.press_key("mod+g").unwrap();
    app.wait_for("2/2").unwrap();
    app.wait_for("8:8").unwrap();
    app.press_key("mod+shift+g").unwrap();
    app.wait_for("1/2").unwrap();
    app.wait_for("4:8").unwrap();

    app.clear("id:code-find-input").unwrap();
    app.type_text("id:code-find-input", "new-greeting").unwrap();
    app.type_text("id:code-replace-input", "replaced-by-cmd-r")
        .unwrap();
    app.press("id:code-replace-all").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    // 关闭查找替换条回归编辑器，按 ⌘Z 撤销并验证内容恢复为 new-greeting，再按 ⇧⌘Z 重做并 ⌘S 保存
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:code-find-bar", Duration::from_secs(2))
        .unwrap();
    app.press_key("mod+z").unwrap();
    app.press_key("mod+a").unwrap();
    app.press_key("mod+c").unwrap();
    let after_undo = app.screen().clipboard().unwrap_or("").to_string();
    assert!(
        after_undo.contains("new-greeting") && !after_undo.contains("replaced-by-cmd-r"),
        "Cmd+Z must undo replacement, got: {after_undo:?}"
    );
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    // ⌘S 保存并核对磁盘写入
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(2))
        .unwrap();
    assert!(
        fs::read_to_string(fx.repo_dir.join("src/app.go"))
            .unwrap()
            .contains("replaced-by-cmd-r")
    );
}

#[test]
fn ui_find_shortcut_focuses_input_and_escape_only_closes_the_visible_bar() {
    let _guard = lock_env();
    let fx = Fixture::new("find-focus");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.press_key("mod+f").unwrap();
    app.wait_for("id:diff-find-input").unwrap();
    for key in ["n", "e", "w"] {
        app.press_key(key).unwrap();
    }
    assert_eq!(app.screen().input_text("id:diff-find-input").unwrap(), Some("new"));
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());

    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press_key("mod+f").unwrap();
    app.wait_for("id:code-find-input").unwrap();
    for key in ["s", "a", "m"] {
        app.press_key(key).unwrap();
    }
    assert_eq!(app.screen().input_text("id:code-find-input").unwrap(), Some("sam"));
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:code-find-bar", Duration::from_secs(2)).unwrap();
    app.press("id:editor-tab-working-diff").unwrap();
    app.wait_for("id:diff-find-bar").unwrap();
    assert_eq!(app.screen().input_text("id:diff-find-input").unwrap(), Some("new"));
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+f").unwrap();
    app.press_key("o").unwrap();
    assert_eq!(app.screen().input_text("id:diff-find-input").unwrap(), Some("newo"));
}

#[test]
fn ui_find_uses_case_whole_word_and_regular_expression_in_source_and_diff() {
    let _guard = lock_env();
    let fx = Fixture::new("find-modes");
    let path = fx.repo_dir.join("README.md");
    let text = "Needle NEEDLE needles Needle_1";
    fs::write(&path, text).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-README.md").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    for kind in ["diff", "code"] {
        if kind == "code" {
            app.press_key("mod+shift+o").unwrap();
            app.type_text("id:review-picker-input", "README.md").unwrap();
            app.submit("id:review-picker-input").unwrap();
            app.wait_for("id:code-editor-view").unwrap();
            app.press("text:/Needle NEEDLE needles Needle_1/").unwrap();
            app.press_key("mod+a").unwrap();
            app.press_key("mod+c").unwrap();
            assert_eq!(app.screen().clipboard(), Some(text));
        }
        app.press_key("mod+f").unwrap();
        let input = format!("id:{kind}-find-input");
        let bar = format!("id:{kind}-find-bar");
        app.wait_for(&input).unwrap();
        if kind == "code" {
            assert_eq!(app.screen().input_text(&input).unwrap(), Some(text));
        }
        app.clear(&input).unwrap();
        app.type_text(&input, "Needle").unwrap();
        app.wait_for(&format!("{bar} >> text:4 个结果")).unwrap();
        app.press(&format!("id:{kind}-find-case")).unwrap();
        app.wait_for(&format!("{bar} >> text:2 个结果")).unwrap();
        app.press(&format!("id:{kind}-find-word")).unwrap();
        app.wait_for(&format!("{bar} >> text:1 个结果")).unwrap();
        app.press(&format!("id:{kind}-find-regex")).unwrap();
        app.clear(&input).unwrap();
        app.type_text(&input, "(").unwrap();
        app.wait_for(&format!("{bar} >> text:正则表达式无效")).unwrap();
        app.clear(&input).unwrap();
        app.type_text(&input, "Needle_[0-9]").unwrap();
        app.wait_for(&format!("{bar} >> text:1 个结果")).unwrap();
        app.press_key("mod+g").unwrap();
        app.wait_for(&format!("{bar} >> text:1/1")).unwrap();
        app.press_key("escape").unwrap();
    }
    app.right_click("id:editor-tab-file-README.md").unwrap();
    app.choose_menu_item("向右拆分").unwrap();
    app.wait_for("id:split-editor-right").unwrap();
    app.press("id:split-tab-right-file::sample-repo::README.md").unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    let right = "id:split-tab-right-file::sample-repo::src/app.go";
    app.wait_for(right).unwrap();
    app.press_key("mod+f").unwrap();
    app.wait_for("id:code-find-bar >> text:0 个结果").unwrap();
    app.press("id:split-tab-left-file::sample-repo::README.md").unwrap();
    app.wait_for("id:code-find-bar >> text:1 个结果").unwrap();
    app.press(right).unwrap();
    app.wait_for("id:code-find-bar >> text:0 个结果").unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), text);
}

#[test]
fn ui_diff_left_side_find_isolation_and_single_column_find() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-left-single-find");
    let mut app = fx.start_headless(1440.0, 900.0);

    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // 1. 聚焦左侧编辑器后按 ⌘F -> 必须显示“查找左侧”，查右侧独有词返回 0，查左侧独有词命中 1/1
    app.press("id:diff-old-pane-bar").unwrap();
    app.press_key("mod+f").unwrap();
    app.wait_for("id:diff-find-bar").unwrap();
    app.wait_for("查找左侧").unwrap();
    app.type_text("id:diff-find-input", "new-greeting").unwrap();
    app.wait_for("0 个结果").unwrap();

    app.clear("id:diff-find-input").unwrap();
    app.type_text("id:diff-find-input", "old-greeting").unwrap();
    app.wait_for("1 个结果").or_else(|_| app.wait_for("1/1")).unwrap();

    // 2. 切到单栏 Diff -> 查找条显示“查找单栏”并可查找旧/新两侧文本
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    app.wait_for("查找单栏").unwrap();
    app.clear("id:diff-find-input").unwrap();
    app.type_text("id:diff-find-input", "new-greeting").unwrap();
    app.wait_for("1 个结果").or_else(|_| app.wait_for("1/1")).unwrap();
}

#[test]
fn ui_working_diff_dirty_f4_preserves_edits_and_commit_dir_copy() {
    let _guard = lock_env();
    let fx = Fixture::new("dirty-f4-dir-copy");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 单击 Commit 目录行 `src` 后按 ⌘C -> 复制目录 display_path
    app.press("id:change-dir-unstaged-src").unwrap();
    app.press_key("mod+c").unwrap();
    let expected_dir = nemorous::paths::display_path(&fx.repo_dir.join("src").to_string_lossy());
    assert_eq!(app.screen().clipboard(), Some(expected_dir.as_str()));

    // 2. 打开 src/app.go 工作区 Diff 并在右侧轻改（不保存），直接按 F4 跳源码 -> 源码必须保留未保存编辑！
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.type_text("id:working-diff-new-editor", "// dirty-f4-carry\n")
        .unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();

    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    // 在源码页做二次编辑后再 ⌘S 保存 -> 磁盘包含两段修改，且 ⌘[ 返回工作区 Diff 时同步清脏
    app.press_key("mod+r").unwrap();
    app.wait_for("id:code-find-bar").unwrap();
    app.type_text("id:code-find-input", "new-greeting").unwrap();
    app.type_text("id:code-replace-input", "source-second-edit")
        .unwrap();
    app.press("id:code-replace-all").unwrap();
    app.press_key("escape").unwrap();

    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(2))
        .unwrap();
    let final_disk = fs::read_to_string(fx.repo_dir.join("src/app.go")).unwrap();
    assert!(
        final_disk.contains("// dirty-f4-carry") && final_disk.contains("source-second-edit"),
        "expected both diff and source edits persisted, got: {final_disk:?}"
    );
    app.press_key("mod+[").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    assert!(
        !app.screen().matches("id:diff-dirty-badge").unwrap(),
        "returning to working diff after saving in source must clear diff dirty state"
    );
}

#[test]
fn ui_alt_arrows_preserve_editor_input_word_nav_and_modal_blocks_shortcuts() {
    let _guard = lock_env();
    let fx = Fixture::new("alt-arrows-modal");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 打开 src/app.go 源码页并跳到 1:1 ("package main")，按 alt+right / alt+left 必须保持在源码页且按词移动！
    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:1:1").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.wait_for("1:1").unwrap();

    app.press_key("alt+right").unwrap();
    assert!(
        app.screen().matches("id:code-editor-pane").unwrap(),
        "alt+right in CodeEditor must stay in CodeEditor"
    );
    assert!(
        !app.screen().matches("id:working-diff-pane").unwrap(),
        "alt+right in CodeEditor must NOT switch into WorkingDiff"
    );
    app.wait_for("1:8").unwrap();
    app.press_key("alt+left").unwrap();
    app.wait_for("1:1").unwrap();

    // 2. 在筛选 <input> 内按 alt+left / alt+right 绝不劫持切入 WorkingDiff
    app.press_key("mod+0").unwrap();
    app.press_key("mod+f").unwrap();
    app.type_text("id:changes-filter-input", "app.go").unwrap();
    app.press_key("alt+left").unwrap();
    app.press_key("alt+right").unwrap();
    assert!(
        app.screen().matches("id:code-editor-pane").unwrap(),
        "alt+arrows in <input> must NOT open WorkingDiff"
    );
    assert!(!app.screen().matches("id:working-diff-pane").unwrap());
    app.clear("id:changes-filter-input").unwrap();

    // 3. 模态弹窗（goto-line-modal）打开时屏蔽后台全局快捷键（mod+w / mod+0 / f4 / alt+right）
    app.press("id:editor-tab-file-src/app.go").unwrap();
    app.press_key("mod+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.press_key("mod+w").unwrap();
    app.press_key("mod+0").unwrap();
    app.press_key("f4").unwrap();
    app.press_key("alt+right").unwrap();
    assert!(
        app.screen().matches("id:goto-line-modal").unwrap(),
        "modal must remain open against background shortcuts"
    );
    assert!(
        app.screen().matches("id:code-editor-pane").unwrap(),
        "mod+w behind modal must NOT close active code tab"
    );
    assert!(
        !app.screen().matches("id:working-diff-pane").unwrap(),
        "alt+right behind modal must NOT open working diff"
    );

    app.press_key("escape").unwrap();
    app.wait_for_gone("id:goto-line-modal", Duration::from_secs(2))
        .unwrap();
    assert!(app.screen().matches("id:code-editor-pane").unwrap());
}

#[test]
fn ui_undoing_a_diff_draft_keeps_the_source_tab_clean() {
    let _guard = lock_env();
    let fx = Fixture::new("undo-diff-source-draft");
    let path = fx.repo_dir.join("src/app.go");
    let disk = fs::read(&path).unwrap();
    let marker = "DIFF_UNDONE_DRAFT";
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.type_text("id:working-diff-new-editor", marker).unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    app.press_key("mod+[").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
    app.press_key("mod+]").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    assert!(!app.screen().matches("id:code-dirty-badge").unwrap());
    assert!(!app.screen().texts().iter().any(|line| line.contains(marker)));
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    assert!(app.screen().texts().iter().any(|line| line.contains(marker)));
    assert_eq!(fs::read(&path).unwrap(), disk);
}

#[test]
fn ui_diff_and_source_share_undo_in_both_directions() {
    let _guard = lock_env();
    let fx = Fixture::new("shared-diff-source-undo");
    let path = fx.repo_dir.join("src/app.go");
    let disk = fs::read(&path).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    let source = "id:code-editor-view";
    app.wait_for("id:code-editor-view >> text:package main").unwrap();
    app.press("id:code-editor-view >> text:package main").unwrap();
    let mut paste = nsh_core::PasteEvent::default();
    paste.text = "SOURCE_DRAFT".into();
    app.paste(source, &paste).unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    app.wait_for("id:code-editor-view >> text:/SOURCE_DRAFT/").unwrap();

    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/SOURCE_DRAFT/").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    app.press("text:/SOURCE_DRAFT/").unwrap();
    paste.text = "DIFF_DRAFT\n".into();
    app.paste("id:working-diff-unified-editor", &paste).unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:/DIFF_DRAFT/").unwrap();

    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-view >> text:/DIFF_DRAFT/").unwrap();
    app.press_key("mod+z").unwrap();
    assert!(app.screen().texts().iter().any(|line| line.contains("SOURCE_DRAFT")));
    assert!(!app.screen().texts().iter().any(|line| line.contains("DIFF_DRAFT")));
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(3)).unwrap();
    app.press_key("mod+[").unwrap();
    let local_change = "id:working-diff-unified-editor >> text:/new-greeting/";
    app.wait_for(local_change).unwrap();
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:/SOURCE_DRAFT/").unwrap();
    app.press_key("mod+shift+z").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:/DIFF_DRAFT/").unwrap();
    assert_eq!(fs::read(&path).unwrap(), disk);
}

#[test]
fn ui_working_diff_honors_configured_undo_and_redo_after_returning_from_source() {
    use nsh_core::desktop::{DesktopCommand, MenuItem, MenuRole, Shortcut};

    let check_edit_menu = |app: &Headless, document_has_focus: bool| {
        let menu = app.screen().commands().iter().rev().find_map(|(_, command)| {
            let nsh_core::Command::Desktop(command) = command else { return None };
            match &**command {
                DesktopCommand::SetMenu(Some(items)) => Some(items),
                _ => None,
            }
        }).expect("application menu");
        let edit_items = menu.iter().find_map(|item| match item {
            MenuItem::Submenu { label, items, .. } if label == "编辑" => Some(items),
            _ => None,
        }).expect("Edit menu");
        for (index, (label, key, role)) in [
            ("撤销", "mod+alt+z", MenuRole::Undo),
            ("重做", "mod+alt+shift+z", MenuRole::Redo),
        ].into_iter().enumerate() {
            let MenuItem::Action(action) = &edit_items[index] else {
                panic!("expected {label} menu action");
            };
            if document_has_focus {
                assert_eq!(action.label, label);
                assert_eq!(action.shortcut, Some(Shortcut::parse(key, true).unwrap()));
                assert!(action.enabled && action.role.is_none() && action.id != 0);
            } else {
                assert_eq!(action.role, Some(role));
                assert_eq!(action.id, 0);
            }
        }
    };

    let _guard = lock_env();
    let mut fx = Fixture::new("diff-configured-undo");
    let worktree = fx.root.join("linked");
    git_run(&fx.repo_dir, &["worktree", "add", "-b", "linked-review", worktree.to_str().unwrap()]);
    fs::copy(fx.repo_dir.join("src/app.go"), worktree.join("src/app.go")).unwrap();
    fx.repo_dir = worktree;
    let path = fx.repo_dir.join("src/app.go");
    let disk = fs::read(&path).unwrap();
    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.workspace_path = fx.repo_dir.to_string_lossy().into_owned();
    config.keymap_overrides.insert("undo".into(), "mod+alt+z".into());
    config.keymap_overrides.insert("redo".into(), "mod+alt+shift+z".into());
    nemorous::config::save_to(&fx.config_file, &config).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    for editor in ["working-diff-new-editor", "working-diff-unified-editor"] {
        if editor == "working-diff-unified-editor" {
            app.press("id:diff-layout-toggle").unwrap();
        }
        let changed_line = format!("id:{editor} >> text:/new-greeting/");
        app.wait_for(&changed_line).unwrap();
        app.press(&changed_line).unwrap();
        check_edit_menu(&app, true);
        app.type_text(&format!("id:{editor}"), "Z").unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        app.press_key("f4").unwrap();
        app.wait_for("id:code-dirty-badge").unwrap();
        check_edit_menu(&app, true);
        app.press_key("mod+alt+z").unwrap();
        app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(3)).unwrap();
        app.press_key("mod+[").unwrap();
        app.wait_for(&changed_line).unwrap();
        check_edit_menu(&app, true);
        if editor == "working-diff-new-editor" {
            app.press("id:diff-old-pane-bar").unwrap();
            check_edit_menu(&app, false);
            app.press_key("mod+alt+shift+z").unwrap();
            assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
            app.press("id:diff-new-pane-bar").unwrap();
            check_edit_menu(&app, true);
        }
        app.press_key("mod+alt+shift+z").unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        app.press_key("mod+alt+z").unwrap();
        app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), disk);
    }
}

#[test]
fn ui_shared_undo_is_cleared_on_same_byte_file_replacement() {
    let _guard = lock_env();
    let fx = Fixture::new("shared-undo-external-replacement");
    let path = fx.repo_dir.join("src/app.go");
    let mut app = fx.start_headless(1440.0, 900.0);
    let right = "id:working-diff-new-editor";
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(right).unwrap();
    app.press("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
    let mut paste = nsh_core::PasteEvent::default();
    paste.text = "EXTERNAL_IDENTITY".into();
    app.paste(right, &paste).unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-view >> text:/EXTERNAL_IDENTITY/").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(3)).unwrap();

    let saved = fs::read(&path).unwrap();
    let replacement = fx.repo_dir.join(".git/replacement.txt");
    fs::write(&replacement, &saved).unwrap();
    fs::rename(replacement, &path).unwrap();
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:/EXTERNAL_IDENTITY/").unwrap();
    app.press("id:diff-new-pane-bar").unwrap();
    app.press_key("mod+z").unwrap();
    assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-view >> text:/EXTERNAL_IDENTITY/").unwrap();
    app.press_key("mod+z").unwrap();
    assert!(!app.screen().matches("id:code-dirty-badge").unwrap());
    assert_eq!(fs::read(&path).unwrap(), saved);
}

#[test]
fn ui_discarding_either_view_clears_the_shared_working_draft() {
    let _guard = lock_env();
    for close_diff in [false, true] {
        let fx = Fixture::new("shared-draft-discard");
        let path = fx.repo_dir.join("src/app.go");
        let disk = fs::read(&path).unwrap();
        let mut app = fx.start_headless(1440.0, 900.0);
        let right = "id:working-diff-new-editor";
        app.press("id:change-row-unstaged-src/app.go").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
        app.press("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
        let mut paste = nsh_core::PasteEvent::default();
        paste.text = "DISCARD_SHARED_DRAFT".into();
        app.paste(right, &paste).unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        app.press_key("f4").unwrap();
        app.wait_for("id:code-dirty-badge").unwrap();
        if close_diff {
            app.press("id:editor-tab-working-diff").unwrap();
            app.wait_for("id:diff-dirty-badge").unwrap();
        }
        app.press_key("mod+w").unwrap();
        app.press("id:dirty-close-discard").unwrap();
        let (visible, badge) = if close_diff {
            ("id:code-editor-view >> text:/new-greeting/", "id:code-dirty-badge")
        } else {
            ("id:working-diff-new-editor >> text:/new-greeting/", "id:diff-dirty-badge")
        };
        app.wait_for(visible).unwrap();
        app.press(visible).unwrap();
        for shortcut in ["mod+z", "mod+shift+z"] {
            app.press_key(shortcut).unwrap();
            assert!(!app.screen().matches(badge).unwrap(), "close_diff={close_diff}");
            assert!(!app.screen().texts().iter().any(|line| line.contains("DISCARD_SHARED_DRAFT")));
        }
        assert_eq!(fs::read(&path).unwrap(), disk);
    }
}

#[test]
fn ui_accepting_disk_from_either_view_replaces_the_shared_working_draft() {
    let _guard = lock_env();
    for accept_from_diff in [false, true] {
        let fx = Fixture::new("shared-draft-accept-disk");
        let path = fx.repo_dir.join("src/app.go");
        let mut app = fx.start_headless(1440.0, 900.0);
        let right = "id:working-diff-new-editor";
        app.press("id:change-row-unstaged-src/app.go").unwrap();
        app.press_key("mod+d").unwrap();
        app.wait_for("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
        app.press("id:working-diff-new-editor >> text:/new-greeting/").unwrap();
        let mut paste = nsh_core::PasteEvent::default();
        paste.text = "DISCARDED_CONFLICT_DRAFT".into();
        app.paste(right, &paste).unwrap();
        app.wait_for("id:diff-dirty-badge").unwrap();
        app.press_key("f4").unwrap();
        app.wait_for("id:code-dirty-badge").unwrap();
        if accept_from_diff {
            app.press("id:editor-tab-working-diff").unwrap();
            app.press("id:diff-new-pane-bar").unwrap();
        }
        let external = "package main\n\n// ACCEPTED_DISK_VERSION\n";
        fs::write(&path, external).unwrap();
        app.press_key("mod+s").unwrap();
        app.wait_for("id:save-conflict-modal").unwrap();
        app.press("id:conflict-accept-disk-btn").unwrap();
        app.wait_for_gone("id:save-conflict-modal", Duration::from_secs(3)).unwrap();
        if accept_from_diff {
            app.press("id:editor-tab-file-src/app.go").unwrap();
            app.wait_for("id:code-editor-view >> text:/ACCEPTED_DISK_VERSION/").unwrap();
            assert!(!app.screen().matches("id:code-dirty-badge").unwrap());
        } else {
            app.press("id:editor-tab-working-diff").unwrap();
            app.wait_for("id:working-diff-new-editor >> text:/ACCEPTED_DISK_VERSION/").unwrap();
            assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
        }
        app.press_key("mod+z").unwrap();
        assert!(!app.screen().texts().iter().any(|line| line.contains("DISCARDED_CONFLICT_DRAFT")));
        assert_eq!(fs::read_to_string(&path).unwrap(), external);
    }
}

#[test]
fn ui_f4_keeps_the_working_diff_final_empty_line() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-source-final-empty-line");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    app.press("text:  package main").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "10:1").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.wait_for("10:1").unwrap();
    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.wait_for("10:1").unwrap();
}

#[test]
fn ui_working_diff_and_source_symmetric_dirty_sync_no_false_conflict() {
    let _guard = lock_env();
    let fx = Fixture::new("sym-dirty-sync");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. WorkingDiff 脏改 -> F4 跳源码 -> ⌘[ 回 Diff -> 在 Diff 按 ⌘S 保存 -> ⌘] 回源码：
    //    同源 CodeTab 必须同步清脏，继续在源码编辑再保存绝不弹伪保存冲突弹窗！
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.type_text("id:working-diff-new-editor", "// sym-edit-1\n")
        .unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();

    app.press_key("f4").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    app.press_key("mod+[").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(2))
        .unwrap();

    app.press_key("mod+]").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    assert!(
        !app.screen().matches("id:code-dirty-badge").unwrap(),
        "CodeTab must clear dirty badge after same-file WorkingDiff Cmd+S save"
    );

    // 在源码再改一处并保存：不得触发伪保存冲突弹窗
    app.press_key("mod+r").unwrap();
    app.wait_for("id:code-find-bar").unwrap();
    app.type_text("id:code-find-input", "new-greeting").unwrap();
    app.type_text("id:code-replace-input", "sym-edit-2").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.press_key("escape").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(2))
        .unwrap();
    assert!(
        !app.screen().matches("id:save-conflict-modal").unwrap(),
        "saving CodeTab after WorkingDiff save must NOT raise false save-conflict-modal"
    );
    let disk_after = fs::read_to_string(fx.repo_dir.join("src/app.go")).unwrap();
    assert!(disk_after.contains("// sym-edit-1") && disk_after.contains("sym-edit-2"));

    // 2. 源码脏改 -> Commit 树双击打开 WorkingDiff：WorkingDiff 必须带入源码脏编辑
    app.press_key("mod+r").unwrap();
    app.clear("id:code-find-input").unwrap();
    app.type_text("id:code-find-input", "sym-edit-2").unwrap();
    app.clear("id:code-replace-input").unwrap();
    app.type_text("id:code-replace-input", "from-source-dirty").unwrap();
    app.press("id:code-replace-all").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();

    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(2))
        .unwrap();
    assert!(
        fs::read_to_string(fx.repo_dir.join("src/app.go"))
            .unwrap()
            .contains("from-source-dirty")
    );

    // 3. WorkingDiff 脏改 notes.txt -> ⌘⇧O 打开 notes.txt 源码：源码必须带入 Diff 脏编辑
    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: notes.txt").unwrap();
    app.type_text("id:working-diff-new-editor", "from-diff-dirty\n")
        .unwrap();
    app.wait_for("id:diff-dirty-badge").unwrap();

    app.press_key("mod+shift+o").unwrap();
    app.wait_for("id:review-picker-modal").unwrap();
    app.type_text("id:review-picker-input", "notes.txt").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
}

#[test]
fn ui_f7_shift_f7_lands_on_last_hunk_and_crosses_zero_hunk_file() {
    let _guard = lock_env();
    let fx = Fixture::new("f7-last-zero");
    // 在 src/app.go 中制造 2 处相隔较远的改动（2 个 hunk），并在 ws_only.txt 仅改空白（忽略空白后为 0 处差异）
    fs::write(
        fx.repo_dir.join("src/app.go"),
        "package main_edited\n\nfunc hello() string {\n\treturn \"old-greeting\"\n}\n\nfunc footer() string {\n\treturn \"end_edited\"\n}\n",
    )
    .unwrap();
    fs::write(fx.repo_dir.join("ws_only.txt"), "let a = 1;\n").unwrap();
    git_run(&fx.repo_dir, &["add", "ws_only.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "add ws_only"]);
    fs::write(fx.repo_dir.join("ws_only.txt"), "let   a = 1;\n").unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 树顺序：src/app.go (2 hunks) -> notes.txt (1 hunk) -> ws_only.txt (1 hunk -> 忽略空白后 0 hunk)
    //    从 notes.txt 连按两次 ⇧F7 回切到 src/app.go，必须落到最后一个差异块 2/2！
    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: notes.txt").unwrap();

    app.press_key("shift+f7").unwrap();
    assert!(!app.screen().matches("id:toolbar-tooltip").unwrap());
    app.press_key("shift+f7").unwrap();
    app.wait_for("Diff: app.go").unwrap();
    app.wait_for("2/2").unwrap();

    // 2. 切到 ws_only.txt 并开启「忽略空白」使其变为 0 处差异，连按两次 ⇧F7 必须能跨回 notes.txt！
    app.press("id:change-row-unstaged-ws_only.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: ws_only.txt").unwrap();
    app.press("id:diff-options-btn").unwrap();
    app.choose_menu_item("空白比较 > 忽略空白").unwrap();
    app.wait_for("0 处差异").unwrap();

    app.press_key("shift+f7").unwrap();
    assert!(!app.screen().matches("id:toolbar-tooltip").unwrap());
    app.press_key("shift+f7").unwrap();
    app.wait_for("Diff: notes.txt").unwrap();
}

#[test]
fn ui_commit_diff_navigation_reveals_changes_below_the_viewport() {
    let _guard = lock_env();
    let fx = Fixture::new("commit-diff-navigation");
    let baseline = (1..=220).map(|line| format!("baseline {line:03}\n")).collect::<String>();
    for name in ["a-first.txt", "b-two.txt", "c-single.txt"] {
        fs::write(fx.repo_dir.join(name), &baseline).unwrap();
    }
    git_run(&fx.repo_dir, &["add", "a-first.txt", "b-two.txt", "c-single.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add long review fixtures"]);
    for (name, changes) in [
        ("a-first.txt", &[(95, "FIRST_FILE_CHANGE")][..]),
        ("b-two.txt", &[(110, "SECOND_FILE_FIRST_CHANGE"), (180, "SECOND_FILE_NEXT_CHANGE")][..]),
        ("c-single.txt", &[(138, "ONLY_CHANGE_IN_FILE")][..]),
    ] {
        let mut edited = baseline.clone();
        for (line, text) in changes {
            edited = edited.replace(&format!("baseline {line:03}\n"), &format!("{text}\n"));
        }
        fs::write(fx.repo_dir.join(name), edited).unwrap();
    }
    git_run(&fx.repo_dir, &["add", "a-first.txt", "b-two.txt", "c-single.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Review changes far below the file header"]);
    for (name, written, working) in [
        ("a-first.txt", "FIRST_FILE_CHANGE", "WORKTREE_FIRST_CHANGE"),
        ("b-two.txt", "SECOND_FILE_FIRST_CHANGE", "WORKTREE_SECOND_CHANGE"),
    ] {
        let path = fx.repo_dir.join(name);
        let text = fs::read_to_string(&path).unwrap().replace(written, working);
        fs::write(path, text).unwrap();
    }
    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.diff_single_column = true;
    nemorous::config::save_to(&fx.config_file, &config).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+9").unwrap();
    app.wait_for("text:Review changes far below the file header").unwrap();
    app.press("text:Review changes far below the file header").unwrap();
    app.wait_for("id:git-commit-file-a-first.txt").unwrap();
    app.press("id:git-commit-file-a-first.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("+ FIRST_FILE_CHANGE").unwrap();

    app.press("text:Add long review fixtures").unwrap();
    app.wait_for("id:git-commit-file-b-two.txt").unwrap();
    app.press("id:diff-next-file").unwrap();
    app.wait_for("+ SECOND_FILE_FIRST_CHANGE").unwrap();
    app.press("id:diff-next-hunk").unwrap();
    app.wait_for("+ SECOND_FILE_NEXT_CHANGE").unwrap();

    app.press("id:diff-next-file").unwrap();
    app.wait_for("+ ONLY_CHANGE_IN_FILE").unwrap();
    app.scroll("id:commit-diff-unified-editor", px(-10_000.0)).unwrap();
    app.wait_for("baseline 001").unwrap();
    app.press("id:diff-next-hunk").unwrap();
    app.wait_for("+ ONLY_CHANGE_IN_FILE").unwrap();

    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:commit-diff-new-editor >> text:ONLY_CHANGE_IN_FILE").unwrap();
    app.wait_for("id:change-row-unstaged-a-first.txt").unwrap();
    app.press("id:change-row-unstaged-a-first.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();
    app.wait_for("WORKTREE_FIRST_CHANGE").unwrap();
    app.press("id:diff-next-file").unwrap();
    app.wait_for("WORKTREE_SECOND_CHANGE").unwrap();
}

#[test]
fn ui_diff_navigation_uses_the_caret_in_working_and_commit_diffs() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-caret-navigation");
    let path = fx.repo_dir.join("review.txt");
    let base = (1..=205).map(|line| format!("context {line:03}\n")).collect::<String>();
    fs::write(&path, &base).unwrap();
    git_run(&fx.repo_dir, &["add", "review.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add navigation fixture"]);
    let change = |prefix: &str| {
        base.replace("context 020", &format!("{prefix} FIRST_CHANGE"))
            .replace("context 096", &format!("{prefix} SECOND_CHANGE"))
            .replace("context 097", &format!("{prefix} SECOND_CONTINUATION"))
            .replace("context 177", &format!("{prefix} THIRD_CHANGE"))
    };
    fs::write(&path, change("HISTORY")).unwrap();
    git_run(&fx.repo_dir, &["add", "review.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Navigation fixture changes"]);
    let worktree = change("WORKTREE");
    fs::write(&path, &worktree).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-review.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:WORKTREE FIRST_CHANGE").unwrap();

    let goto = |app: &mut Headless, line: &str| {
        app.press_key("mod+l").unwrap();
        app.type_text("id:goto-line-input", line).unwrap();
        app.press("id:goto-line-submit").unwrap();
    };
    goto(&mut app, "100:1");
    app.press("id:working-diff-new-editor >> text:WORKTREE SECOND_CONTINUATION").unwrap();
    app.wait_for("text:2/3").unwrap();
    app.press("id:diff-next-hunk").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:WORKTREE THIRD_CHANGE").unwrap();
    app.press_key("shift+f7").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:WORKTREE SECOND_CHANGE").unwrap();

    goto(&mut app, "68:1");
    app.press_key("f7").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:WORKTREE SECOND_CHANGE").unwrap();
    goto(&mut app, "68:1");
    app.press("id:diff-prev-hunk").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:WORKTREE FIRST_CHANGE").unwrap();

    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    app.press_key("mod+9").unwrap();
    app.wait_for("text:Navigation fixture changes").unwrap();
    app.press("text:Navigation fixture changes").unwrap();
    app.wait_for("id:git-commit-file-review.txt").unwrap();
    app.press("id:git-commit-file-review.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:commit-diff-unified-editor >> text:+ HISTORY FIRST_CHANGE").unwrap();
    goto(&mut app, "100:1");
    app.press("id:commit-diff-unified-editor >> text:+ HISTORY SECOND_CONTINUATION").unwrap();
    app.wait_for("text:2/3").unwrap();
    app.press_key("f7").unwrap();
    app.wait_for("id:commit-diff-unified-editor >> text:+ HISTORY THIRD_CHANGE").unwrap();
    app.press_key("shift+f7").unwrap();
    app.wait_for("id:commit-diff-unified-editor >> text:+ HISTORY SECOND_CHANGE").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), worktree);
}

#[test]
fn ui_split_diff_navigates_tail_changes_from_the_end_of_the_shorter_side() {
    let _guard = lock_env();
    let fx = Fixture::new("split-diff-tail-hunk");
    let path = fx.repo_dir.join("tail.txt");
    let short = (1..=244).map(|line| format!("context {line:03}\n")).collect::<String>() + "tail 🐈";
    let long = short.replace("context 112", "EARLIER_CHANGE") + "\nAPPENDED_TAIL\nanother tail\n";
    fs::write(&path, &long).unwrap();
    git_run(&fx.repo_dir, &["add", "tail.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Baseline with a long tail"]);
    fs::write(&path, &short).unwrap();
    git_run(&fx.repo_dir, &["add", "tail.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Remove the long tail"]);
    fs::write(&path, &long).unwrap();
    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.diff_whitespace_mode = "trim".into();
    config.diff_single_column = false;
    config.diff_sync_scroll = false;
    config.git_log_diff_preview = false;
    nemorous::config::save_to(&fx.config_file, &config).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.wait_for("id:change-row-unstaged-tail.txt").unwrap();
    app.press("id:change-row-unstaged-tail.txt").unwrap();
    app.press_key("mod+d").unwrap();
    for (short_side, long_side, unified_side, tail_prefix) in [
        ("working-diff-old-editor", "working-diff-new-editor", "working-diff-unified-editor", "+"),
        ("commit-diff-new-editor", "commit-diff-old-editor", "commit-diff-unified-editor", "-"),
    ] {
        if short_side.starts_with("commit") {
            app.press_key("mod+9").unwrap();
            app.wait_for("text:Remove the long tail").unwrap();
            app.press("text:Remove the long tail").unwrap();
            app.wait_for("id:git-commit-file-tail.txt").unwrap();
            app.press("id:git-commit-file-tail.txt").unwrap();
            app.press_key("mod+d").unwrap();
        }
        let context = format!("id:{short_side} >> text:context 112");
        let tail = format!("id:{long_side} >> text:APPENDED_TAIL");
        app.wait_for(&context).unwrap();
        app.press(&context).unwrap();
        app.press_key("f7").unwrap();
        app.wait_for(&tail).unwrap();
        app.wait_for("text:2/2").unwrap();
        app.wait_for("text:245:8").unwrap();

        // The focused pane stays at EOF while the other pane scrolls past the change.
        app.scroll(&format!("id:{long_side}"), px(-10_000.0)).unwrap();
        assert!(!app.screen().matches(&tail).unwrap());
        app.press_key("f7").unwrap();
        app.wait_for(&tail).unwrap();
        app.wait_for("text:2/2").unwrap();
        app.press("id:diff-layout-toggle").unwrap();
        app.wait_for(&format!("id:{unified_side} >> text:{tail_prefix} APPENDED_TAIL")).unwrap();
        app.wait_for("text:2/2").unwrap();
        app.press("id:diff-layout-toggle").unwrap();
        app.wait_for(&tail).unwrap();
        app.wait_for("text:2/2").unwrap();
        app.press_key("shift+f7").unwrap();
        app.wait_for(&context).unwrap();
        app.wait_for("text:1/2").unwrap();
        if short_side.starts_with("working") {
            app.press(&context).unwrap();
            app.press_key("f7").unwrap();
            app.wait_for("text:2/2").unwrap();
            app.press_key("mod+alt+z").unwrap();
            app.wait_for("id:diff-dirty-badge").unwrap();
            app.press_key("mod+a").unwrap();
            app.press_key("mod+c").unwrap();
            assert_eq!(app.screen().clipboard(), Some(short.replace("context 112", "EARLIER_CHANGE").as_str()));
            app.press_key("mod+z").unwrap();
            app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(2)).unwrap();
        }
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), long);
}

#[test]
fn ui_diff_rollback_targets_the_clicked_hunk_in_both_layouts() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-rollback-clicked-hunk");
    let path = fx.repo_dir.join("review.txt");
    let base = "first base\ncontext one\ncontext two\ncontext three\ncontext four\nsecond base\nlast\n";
    let working = base.replace("first base", "first working").replace("second base", "second working");
    fs::write(&path, base).unwrap();
    git_run(&fx.repo_dir, &["add", "review.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add rollback fixture"]);
    fs::write(&path, &working).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-review.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:first working").unwrap();

    for single in [false, true] {
        let (editor, first, second, restored) = if single {
            app.press("id:diff-layout-toggle").unwrap();
            ("working-diff-unified-editor", "+ first working", "+ second working", "  second base")
        } else {
            ("working-diff-new-editor", "first working", "second working", "second base")
        };
        let selected = |line| format!("id:{editor} >> text:{line}");
        app.wait_for(&selected(first)).unwrap();
        app.press(&selected(if single { "  context one" } else { "context one" })).unwrap();
        app.press_key("mod+alt+z").unwrap();
        app.wait_for("光标处没有可回滚的改动").unwrap();
        assert!(!app.screen().matches("id:diff-dirty-badge").unwrap());
        app.right_click(&selected(second)).unwrap();
        app.choose_menu_item("回滚当前块").unwrap();
        app.wait_for(&selected(restored)).unwrap();
        assert!(app.screen().matches(&selected(first)).unwrap());
        app.press_key("mod+z").unwrap();
        app.wait_for(&selected(second)).unwrap();
        app.wait_for_gone("id:diff-dirty-badge", Duration::from_secs(3)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), working);
    }
}

#[test]
fn ui_diff_layout_preserves_the_current_hunk_and_manually_scrolled_context() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-layout-viewport");
    let filename = "late-changes.txt";
    let path = fx.repo_dir.join(filename);
    let baseline = (1..=240).map(|line| format!("baseline {line:03}\n")).collect::<String>();
    fs::write(&path, &baseline).unwrap();
    git_run(&fx.repo_dir, &["add", filename]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add long review fixture"]);
    fs::write(
        &path,
        baseline.replace("baseline 140", "FIRST_CHANGE_HERE").replace("baseline 220", "SECOND_CHANGE_HERE"),
    ).unwrap();
    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.diff_single_column = false;
    config.diff_collapse_unchanged = false;
    nemorous::config::save_to(&fx.config_file, &config).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.wait_for("id:change-row-unstaged-late-changes.txt").unwrap();
    app.press("id:change-row-unstaged-late-changes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:FIRST_CHANGE_HERE").unwrap();
    app.press("id:diff-next-hunk").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:SECOND_CHANGE_HERE").unwrap();

    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:+ SECOND_CHANGE_HERE").unwrap();
    app.wait_for("2/2").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:SECOND_CHANGE_HERE").unwrap();

    // Scrolling the other pane changes the viewport without moving keyboard focus.
    app.scroll("id:working-diff-old-editor", px(-10_000.0)).unwrap();
    app.wait_for("id:working-diff-old-editor >> text:baseline 001").unwrap();
    app.scroll("id:working-diff-old-editor", px(540.0)).unwrap();
    app.wait_for("id:working-diff-old-editor >> text:baseline 042").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:SECOND_CHANGE_HERE").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:/baseline 042/").unwrap();
    assert!(!app.screen().matches("id:working-diff-unified-editor >> text:/SECOND_CHANGE_HERE/").unwrap());
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-old-editor >> text:baseline 042").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:baseline 042").unwrap();

    app.press("id:diff-collapse-btn").unwrap();
    app.press("id:diff-next-hunk").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:SECOND_CHANGE_HERE").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:+ SECOND_CHANGE_HERE").unwrap();
    assert!(!app.screen().matches("id:working-diff-unified-editor >> text:/baseline 210/").unwrap());
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:SECOND_CHANGE_HERE").unwrap();
    assert!(!app.screen().matches("id:working-diff-new-editor >> text:baseline 210").unwrap());
}

#[test]
fn ui_split_diff_synchronizes_matching_lines_on_either_pane() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-synchronized-scroll");
    let filename = "different-length.txt";
    let path = fx.repo_dir.join(filename);
    let baseline = (1..=240).map(|line| format!("baseline {line:03}\n")).collect::<String>();
    fs::write(&path, &baseline).unwrap();
    git_run(&fx.repo_dir, &["add", filename]);
    git_run(&fx.repo_dir, &["commit", "-m", "Baseline for paired scrolling"]);
    let inserted = (1..=50).map(|line| format!("added {line:03}\n")).collect::<String>();
    let comparison = baseline.replace("baseline 030\n", &format!("{inserted}baseline 030\n"));
    fs::write(&path, &comparison).unwrap();
    git_run(&fx.repo_dir, &["add", filename]);
    git_run(&fx.repo_dir, &["commit", "-m", "Insert fifty lines"]);
    fs::write(&path, &baseline).unwrap();
    git_run(&fx.repo_dir, &["add", filename]);
    git_run(&fx.repo_dir, &["commit", "-m", "Restore baseline"]);
    fs::write(&path, &comparison).unwrap();

    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.diff_single_column = false;
    config.diff_collapse_unchanged = false;
    config.git_log_diff_preview = false;
    nemorous::config::save_to(&fx.config_file, &config).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.wait_for("id:change-row-unstaged-different-length.txt").unwrap();
    app.press("id:change-row-unstaged-different-length.txt").unwrap();
    app.press_key("mod+d").unwrap();
    for kind in ["working", "commit"] {
        let old = format!("id:{kind}-diff-old-editor");
        let new = format!("id:{kind}-diff-new-editor");
        let in_pane = |pane: &str, text| format!("{pane} >> text:{text}");
        if kind == "commit" {
            app.press_key("mod+9").unwrap();
            app.wait_for("text:Insert fifty lines").unwrap();
            app.press("text:Insert fifty lines").unwrap();
            app.wait_for("id:git-commit-file-different-length.txt").unwrap();
            app.press("id:git-commit-file-different-length.txt").unwrap();
            app.press_key("mod+d").unwrap();
        }
        app.wait_for(&in_pane(&new, "added 001")).unwrap();
        if kind == "working" {
            app.press("id:diff-options-btn").unwrap();
            app.choose_menu_item("同步滚动").unwrap();
            assert!(nemorous::config::load_from(&fx.config_file).unwrap().diff_sync_scroll);
        }
        app.scroll(&old, px(1_620.0)).unwrap();
        app.wait_for(&in_pane(&old, "baseline 104")).unwrap();
        app.wait_for(&in_pane(&new, "baseline 104")).unwrap();

        app.scroll(&new, px(-10_000.0)).unwrap();
        app.wait_for(&in_pane(&new, "baseline 010")).unwrap();
        app.wait_for(&in_pane(&old, "baseline 010")).unwrap();
    }
    app.press("id:diff-options-btn").unwrap();
    app.choose_menu_item("同步滚动").unwrap();
    assert!(!nemorous::config::load_from(&fx.config_file).unwrap().diff_sync_scroll);
    app.scroll("id:commit-diff-old-editor", px(1_620.0)).unwrap();
    app.wait_for("id:commit-diff-old-editor >> text:baseline 094").unwrap();
    app.wait_for("id:commit-diff-new-editor >> text:baseline 010").unwrap();
    assert!(!app.screen().matches("id:commit-diff-new-editor >> text:baseline 094").unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn ui_opening_commit_diff_keeps_navigation_responsive_and_ignores_old_reads() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("commit-diff-worker");
    let prefix = (1..=145).map(|line| format!("baseline {line:03}\n")).collect::<String>();
    for name in ["a-first.txt", "b-slow.txt", "c-last.txt"] {
        fs::write(fx.repo_dir.join(name), &prefix).unwrap();
    }
    let files = ["add", "a-first.txt", "b-slow.txt", "c-last.txt"];
    git_run(&fx.repo_dir, &files);
    git_run(&fx.repo_dir, &["commit", "-m", "Baseline for delayed Diff"]);
    for (name, marker) in [
        ("a-first.txt", "FIRST_REVIEW_LINE"),
        ("b-slow.txt", "DELAYED_REVIEW_LINE"),
        ("c-last.txt", "LAST_REVIEW_LINE"),
    ] {
        fs::write(fx.repo_dir.join(name), format!("{prefix}{marker}\n")).unwrap();
    }
    git_run(&fx.repo_dir, &files);
    git_run(&fx.repo_dir, &["commit", "-m", "Review a delayed file read"]);
    let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    cfg.diff_single_column = true;
    cfg.git_log_diff_preview = false;
    nemorous::config::save_to(&fx.config_file, &cfg).unwrap();

    let control = fx.root.join("diff-control");
    let bin = fx.root.join("bin");
    fs::create_dir(&control).unwrap();
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
case "$3 $4 $5" in
    'cat-file blob '*':b-slow.txt') gate=blob ;;
    'diff-tree --no-commit-id --name-status') gate=list ;;
esac
if [ -n "$gate" ] && [ "$(cat ../diff-control/hold 2>/dev/null)" = "$gate" ] && mkdir ../diff-control/claimed 2>/dev/null; then
    : > ../diff-control/started
    count=0
    while [ -f ../diff-control/hold ] && [ "$count" -lt 240 ]; do
        sleep 0.02
        count=$((count + 1))
    done
    /usr/bin/git "$@"
    result=$?
    : > ../diff-control/finished
    exit "$result"
fi
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    app.press_key("mod+9").unwrap();
    app.wait_for("text:Review a delayed file read").unwrap();
    app.press("text:Review a delayed file read").unwrap();
    app.wait_for("id:git-commit-file-a-first.txt").unwrap();
    app.press("id:git-commit-file-a-first.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("+ FIRST_REVIEW_LINE").unwrap();

    let hold = control.join("hold");
    let started = control.join("started");
    let finished = control.join("finished");
    let wait_for_marker = |app: &mut Headless, marker: &Path| {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !marker.is_file() {
            assert!(std::time::Instant::now() < deadline, "missing Git marker {}", marker.display());
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    fs::write(&hold, "blob").unwrap();
    app.press("id:diff-next-file").unwrap();
    wait_for_marker(&mut app, &started);
    app.wait_for("id:diff-read-status").unwrap();
    assert!(!finished.is_file(), "reading the next commit file blocked the UI");
    app.press("id:git-commit-file-c-last.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("+ LAST_REVIEW_LINE").unwrap();
    assert!(!finished.is_file(), "the latest selection waited behind the previous read");
    fs::remove_file(&hold).unwrap();
    wait_for_marker(&mut app, &finished);
    app.wait(Duration::from_millis(200)).unwrap();
    app.wait_for("+ LAST_REVIEW_LINE").unwrap();
    assert!(!app.screen().matches("+ DELAYED_REVIEW_LINE").unwrap());

    // With a different commit selected, file navigation resolves the original commit's list.
    app.press("text:Baseline for delayed Diff").unwrap();
    app.wait_for("id:git-commit-file-a-first.txt").unwrap();
    fs::remove_file(&started).unwrap();
    fs::remove_file(&finished).unwrap();
    fs::remove_dir(control.join("claimed")).unwrap();
    fs::write(&hold, "list").unwrap();
    app.press("id:diff-prev-file").unwrap();
    wait_for_marker(&mut app, &started);
    app.wait_for("id:diff-read-status").unwrap();
    app.press("id:editor-tab-file-README.md").unwrap();
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    app.wait_for_gone("id:diff-read-status", Duration::from_secs(2)).unwrap();
    assert!(!finished.is_file(), "switching to source waited for the commit's file list");
    fs::remove_file(&hold).unwrap();
    wait_for_marker(&mut app, &finished);
    app.wait(Duration::from_millis(200)).unwrap();
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    app.press("id:editor-tab-commit-diff").unwrap();
    app.wait_for("+ LAST_REVIEW_LINE").unwrap();
    app.press("id:diff-prev-file").unwrap();
    app.wait_for("+ DELAYED_REVIEW_LINE").unwrap();

    app.press("text:Review a delayed file read").unwrap();
    let delayed = "id:git-commit-file-b-slow.txt";
    app.wait_for(delayed).unwrap();
    app.press("id:git-commit-file-c-last.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("+ LAST_REVIEW_LINE").unwrap();
    let toggle_preview = |app: &mut Headless| {
        app.press("id:git-file-view-options").unwrap();
        app.choose_menu_item("差异预览").unwrap();
    };
    let hold_blob = || {
        for marker in [&started, &finished] {
            fs::remove_file(marker).unwrap();
        }
        fs::remove_dir(control.join("claimed")).unwrap();
        fs::write(&hold, "blob").unwrap();
    };

    app.press(delayed).unwrap();
    hold_blob();
    toggle_preview(&mut app);
    wait_for_marker(&mut app, &started);
    app.wait_for("id:diff-read-status").unwrap();
    toggle_preview(&mut app);
    assert!(!app.screen().matches("id:diff-read-status").unwrap());
    assert!(!finished.is_file());
    fs::remove_file(&hold).unwrap();
    wait_for_marker(&mut app, &finished);
    app.wait_for("+ LAST_REVIEW_LINE").unwrap();

    // Toggling preview during an explicit open must preserve the request.
    app.press(delayed).unwrap();
    hold_blob();
    app.press_key("mod+d").unwrap();
    wait_for_marker(&mut app, &started);
    toggle_preview(&mut app);
    toggle_preview(&mut app);
    assert!(app.screen().matches("id:diff-read-status").unwrap());
    assert!(!finished.is_file());
    fs::remove_file(&hold).unwrap();
    wait_for_marker(&mut app, &finished);
    app.wait_for("+ DELAYED_REVIEW_LINE").unwrap();

    // A subsequent selection with preview enabled replaces a pending open of another file.
    app.press(delayed).unwrap();
    hold_blob();
    app.press_key("mod+d").unwrap();
    wait_for_marker(&mut app, &started);
    toggle_preview(&mut app);
    app.press("id:git-commit-file-a-first.txt").unwrap();
    app.wait_for("+ FIRST_REVIEW_LINE").unwrap();
    assert!(!finished.is_file());
    fs::remove_file(&hold).unwrap();
    wait_for_marker(&mut app, &finished);
    app.wait_for("+ FIRST_REVIEW_LINE").unwrap();

    toggle_preview(&mut app);
    hold_blob();
    app.press("id:diff-next-file").unwrap();
    wait_for_marker(&mut app, &started);
    app.press("id:git-commit-file-c-last.txt").unwrap();
    assert!(!finished.is_file());
    fs::remove_file(&hold).unwrap();
    wait_for_marker(&mut app, &finished);
    app.wait_for("+ DELAYED_REVIEW_LINE").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("+ LAST_REVIEW_LINE").unwrap();
}

#[test]
fn ui_working_diff_tabs_and_history_keep_independent_pane_scroll() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-view-scroll");
    let path = fx.repo_dir.join("long.txt");
    let baseline = (1..=260).map(|line| format!("baseline {line:03}\n")).collect::<String>();
    fs::write(&path, &baseline).unwrap();
    git_run(&fx.repo_dir, &["add", "long.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Long file for Diff scrolling"]);
    fs::write(&path, baseline.replace("baseline 095", "worktree change")).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    app.press("id:change-row-unstaged-long.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:worktree change").unwrap();

    let left = "id:working-diff-old-editor";
    let right = "id:working-diff-new-editor";
    app.press_key("mod+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.type_text("id:goto-line-input", "190:1").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.scroll(left, px(-10_000.0)).unwrap();
    let expect_scroll = |app: &mut Headless| {
        app.wait_for(&format!("{left} >> text:baseline 001")).unwrap();
        app.wait_for(&format!("{right} >> text:baseline 190")).unwrap();
    };
    expect_scroll(&mut app);
    app.press("id:editor-tab-file-README.md").unwrap();
    app.wait_for("id:code-editor-view >> text:# Sample").unwrap();
    app.press("id:editor-tab-working-diff").unwrap();
    expect_scroll(&mut app);

    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:untracked note line 1").unwrap();
    app.press_key("mod+[").unwrap();
    expect_scroll(&mut app);
}

#[test]
fn ui_diff_blame_uses_the_visible_revision_and_dismisses_on_scroll() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-blame-scroll");
    let path = fx.repo_dir.join("long.txt");
    let body = |label| (1..=180)
        .map(|row| format!("{label} {row:03} {}\n", "abcdefghijklmnop".repeat(4)))
        .collect::<String>();
    fs::write(&path, body("baseline")).unwrap();
    git_run(&fx.repo_dir, &["add", "long.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Long baseline for attribution"]);
    fs::write(&path, body("committed")).unwrap();
    git_run(&fx.repo_dir, &["add", "long.txt"]);
    git_run(&fx.repo_dir, &["commit", "--author", "Review Author <review@example.com>", "-m", "Committed lines for attribution"]);
    fs::write(&path, body("working")).unwrap();

    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.editor_soft_wrap = false;
    nemorous::config::save_to(&fx.config_file, &config).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-long.txt").unwrap();
    app.press_key("mod+d").unwrap();
    let tooltip = "id:diff-blame-tooltip";
    let left = "id:working-diff-old-editor";
    let right = "id:working-diff-new-editor";
    app.wait_for(left).unwrap();
    app.hover("text:/^committed 008 /").unwrap();
    app.scroll(left, px(360.0)).unwrap();
    app.wait_for_gone("text:/^committed 008 /", Duration::from_secs(2)).unwrap();
    app.wait(Duration::from_millis(350)).unwrap();
    assert!(!app.screen().matches(tooltip).unwrap(), "a waiting hover survived a scrollbar move");
    app.scroll(left, px(-360.0)).unwrap();
    app.hover("id:diff-details").unwrap();
    app.hover("text:/^committed 008 /").unwrap();
    app.wait_for(tooltip).unwrap();
    app.wait_for("id:diff-blame-tooltip >> text:/Review Author · [0-9]{4}-[0-9]{2}-[0-9]{2}/").unwrap();
    app.wheel(left, point(px(0.0), px(280.0))).unwrap();
    app.wait_for_gone(tooltip, Duration::from_secs(2)).unwrap();
    app.wait_for_gone("text:/^committed 008 /", Duration::from_secs(2)).unwrap();
    app.wait_for(right).unwrap();
    app.hover("text:/^working 024 /").unwrap();
    app.wait_for("id:diff-blame-tooltip >> text:未提交的本地修改").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for_gone(tooltip, Duration::from_secs(2)).unwrap();

    app.press_key("mod+9").unwrap();
    app.wait_for("text:Committed lines for attribution").unwrap();
    app.press("text:Committed lines for attribution").unwrap();
    app.wait_for("id:git-commit-file-long.txt").unwrap();
    app.press("id:git-commit-file-long.txt").unwrap();
    app.press_key("mod+d").unwrap();
    let unified = "id:commit-diff-unified-editor";
    app.wait_for(unified).unwrap();
    app.hover("text:/^- baseline 008 /").unwrap();
    app.wait_for("id:diff-blame-tooltip >> text:Long baseline for attribution").unwrap();
    app.wheel(unified, point(px(0.0), px(3960.0))).unwrap();
    app.wait_for_gone(tooltip, Duration::from_secs(2)).unwrap();
    app.hover(r"text:/^\+ committed 032 /").unwrap();
    app.wait_for("id:diff-blame-tooltip >> text:/Review Author · [0-9]{4}-[0-9]{2}-[0-9]{2}/").unwrap();
}

#[test]
fn ui_diff_blame_rechecks_a_line_after_an_unsaved_edit() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-blame-working-edit");
    let file = fx.repo_dir.join("attribution.txt");
    let published = "header\npublished_marker\ntail\n";
    fs::write(&file, published).unwrap();
    git_run(&fx.repo_dir, &["add", "attribution.txt"]);
    git_run(&fx.repo_dir, &["commit", "--author", "Published Author <review@example.com>", "-m", "Publish marked line"]);
    fs::write(&file, "header\nstaged_marker\ntail\n").unwrap();
    git_run(&fx.repo_dir, &["add", "attribution.txt"]);
    fs::write(&file, published).unwrap();

    let row = "id:change-row-unstaged-attribution.txt";
    let editor = "id:working-diff-new-editor";
    let original = "id:working-diff-new-editor >> text:published_marker";
    let replacement = "id:working-diff-new-editor >> text:local_marker";
    let tooltip = "id:diff-blame-tooltip";
    let mut app = fx.start_headless_expect(1440.0, 900.0, row);
    app.press(row).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(original).unwrap();
    app.hover(original).unwrap();
    app.wait_for("id:diff-blame-tooltip >> text:/Published Author/").unwrap();

    app.press(original).unwrap();
    app.press_key("mod+a").unwrap();
    app.type_text(editor, "header\nlocal_marker\ntail\n").unwrap();
    app.wait_for(replacement).unwrap();
    app.wait_for_gone(tooltip, Duration::from_secs(2)).unwrap();
    app.hover("id:diff-details").unwrap();
    app.hover(replacement).unwrap();
    app.wait_for("id:diff-blame-tooltip >> text:未提交的本地修改").unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), published);
}

#[cfg(target_os = "macos")]
#[test]
fn ui_diff_blame_ignores_a_git_result_after_scroll_and_file_switch() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("diff-blame-late-result");
    let file = fx.repo_dir.join("attribution.txt");
    let body = |label| (1..=100).map(|row| format!("{label}_{row:03}\n")).collect::<String>();
    fs::write(&file, body("published")).unwrap();
    git_run(&fx.repo_dir, &["add", "attribution.txt"]);
    git_run(&fx.repo_dir, &["commit", "--author", "Earlier Author <review@example.com>", "-m", "Earlier file attribution"]);
    fs::write(&file, body("working")).unwrap();
    let bin = fx.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
case "$3 $4 $*" in
    '--literal-pathspecs blame '*"-- attribution.txt")
        if [ -f ../blame-hold ] && mkdir ../blame-claimed 2>/dev/null; then
            /usr/bin/git "$@" > ../blame-result || exit $?
            : > ../blame-started
            count=0
            while [ -f ../blame-hold ] && [ "$count" -lt 300 ]; do
                sleep 0.02
                count=$((count + 1))
            done
            cat ../blame-result
            : > ../blame-finished
            exit 0
        fi
        ;;
esac
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    let wait_marker = |app: &mut Headless, name: &str| {
        let marker = fx.root.join(name);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !marker.is_file() {
            assert!(std::time::Instant::now() < deadline, "missing Git marker {name}");
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    let old_editor = "id:working-diff-old-editor";
    let tooltip = "id:diff-blame-tooltip";
    let local_tooltip = "id:diff-blame-tooltip >> text:未提交的本地修改";
    let hold = fx.root.join("blame-hold");

    app.press("id:change-row-unstaged-attribution.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("text:published_008").unwrap();
    fs::write(&hold, "").unwrap();
    app.hover("text:published_008").unwrap();
    wait_marker(&mut app, "blame-started");
    app.scroll(old_editor, px(360.0)).unwrap();
    app.wait_for_gone("text:published_008", Duration::from_secs(2)).unwrap();
    assert!(!app.screen().matches(tooltip).unwrap());

    app.press("id:change-row-unstaged-notes.txt").unwrap();
    app.press_key("mod+d").unwrap();
    let note = "id:working-diff-new-editor >> text:untracked note line 1";
    app.wait_for(note).unwrap();
    app.hover(note).unwrap();
    app.wait_for(local_tooltip).unwrap();
    assert!(!fx.root.join("blame-finished").exists());
    fs::remove_file(hold).unwrap();
    wait_marker(&mut app, "blame-finished");
    app.wait(Duration::from_millis(220)).unwrap();
    app.wait_for(local_tooltip).unwrap();
    assert!(!app.screen().matches("id:diff-blame-tooltip >> text:/Earlier Author/").unwrap());
}

#[test]
fn ui_working_attribution_follows_head_with_identical_file_contents() {
    let _guard = lock_env();
    let fx = Fixture::new("blame-head-moves");
    let path = fx.repo_dir.join("attribution.txt");
    let published = "header\npublished_marker\ntail\n";
    fs::write(&path, published).unwrap();
    git_run(&fx.repo_dir, &["add", "attribution.txt"]);
    git_run(&fx.repo_dir, &["commit", "--author", "Earlier Author <earlier@example.com>", "-m", "Earlier line history"]);
    let rev_parse = |revision| nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", revision]).unwrap();
    let earlier = rev_parse("HEAD");
    let tree = rev_parse("HEAD^{tree}");
    let out = Command::new("git")
        .current_dir(&fx.repo_dir)
        .args(["-c", "user.name=Current Author", "commit-tree", tree.trim(), "-m", "Separate line history"])
        .output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let current = String::from_utf8(out.stdout).unwrap();
    fs::write(&path, "header\nstaged_marker\ntail\n").unwrap();
    git_run(&fx.repo_dir, &["add", "attribution.txt"]);
    let staged = rev_parse(":attribution.txt");
    fs::write(&path, published).unwrap();

    let row = "id:change-row-unstaged-attribution.txt";
    let diff_line = "id:working-diff-new-editor >> text:published_marker";
    let source_line = "id:code-editor-view >> text:published_marker";
    let mut app = fx.start_headless_expect(1440.0, 900.0, row);
    app.press(row).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(diff_line).unwrap();
    app.hover(diff_line).unwrap();
    app.wait_for("id:diff-blame-tooltip >> text:/Earlier Author/").unwrap();
    git_run(&fx.repo_dir, &["update-ref", "HEAD", current.trim(), earlier.trim()]);
    let diff_hidden = app.wait_for_gone("id:diff-blame-tooltip", Duration::from_secs(3));
    app.hover("id:diff-details").unwrap();
    app.hover(diff_line).unwrap();
    let diff_current = app.wait_for("id:diff-blame-tooltip >> text:/Current Author/");

    app.press("id:diff-jump-source-btn").unwrap();
    app.wait_for(source_line).unwrap();
    app.right_click(&format!("{source_line} >> nth=0")).unwrap();
    app.choose_menu_item("显示整文件 Git Blame").unwrap();
    app.wait_for("id:blame-line-2 >> text:/Current Author/").unwrap();
    git_run(&fx.repo_dir, &["update-ref", "HEAD", earlier.trim(), current.trim()]);
    let source_current = app.wait_for("id:blame-line-2 >> text:/Earlier Author/");
    assert_eq!(fs::read_to_string(path).unwrap(), published);
    assert_eq!(rev_parse(":attribution.txt"), staged);
    assert!(diff_hidden.is_ok() && diff_current.is_ok() && source_current.is_ok(), "Diff hide: {diff_hidden:?}\nDiff update: {diff_current:?}\nSource update: {source_current:?}");
}

#[test]
fn ui_single_column_diff_fold_f7_goto_line_and_context_menu_mapping() {
    let _guard = lock_env();
    let fx = Fixture::new("single-col-full");

    // 准备一个 28 行文件，修改第 2 行与第 26 行，使中间产生明显的未改动上下文（含独特标记 MIDDLE_CONTEXT_LINE_14）
    let mut base_lines = Vec::new();
    for i in 1..=28 {
        if i == 14 {
            base_lines.push("const MIDDLE_CONTEXT_LINE_14 = 14;".to_string());
        } else {
            base_lines.push(format!("const LINE_{i:02} = {i};"));
        }
    }
    let old_body = base_lines.join("\n") + "\n";
    fs::write(fx.repo_dir.join("src/long_single.go"), &old_body).unwrap();
    git_run(&fx.repo_dir, &["add", "src/long_single.go"]);
    git_run(&fx.repo_dir, &["commit", "-m", "add long_single.go"]);

    let mut new_lines = base_lines.clone();
    new_lines[1] = "const LINE_02 = 200; // hunk1".to_string();
    new_lines[25] = "const LINE_26 = 2600; // hunk2".to_string();
    fs::write(fx.repo_dir.join("src/long_single.go"), new_lines.join("\n") + "\n").unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);

    // 1. 打开 src/long_single.go 工作区 Diff 并切到单栏模式
    app.press("id:change-row-unstaged-src/long_single.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: long_single.go").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    let mid_line = "  const MIDDLE_CONTEXT_LINE_14 = 14;";
    app.wait_for(mid_line).unwrap();

    // 2. 单栏「折叠未改动」开关必须真实折叠并隐藏第 14 行上下文，再点一次恢复展示！
    app.press("id:diff-collapse-btn").unwrap();
    app.wait_for_gone(mid_line, Duration::from_secs(2))
        .unwrap();
    app.press("id:diff-collapse-btn").unwrap();
    app.wait_for(mid_line).unwrap();

    // 3. 单栏 F7 跳到第 2 个 hunk（2/2），并滚动定位到第 26 行
    app.wait_for("1/2").unwrap();
    app.press_key("f7").unwrap();
    app.wait_for("2/2").unwrap();
    app.wait_for("26:1").unwrap();

    // 4. 单栏 ⌘L 跳转到 26:4 -> 状态栏必须准确映射回 26:4（验证 +2/-2 前缀偏移互逆）
    app.press_key("mod+l").unwrap();
    app.wait_for("id:goto-line-modal").unwrap();
    app.type_text("id:goto-line-input", "26:4").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.wait_for_gone("id:goto-line-modal", Duration::from_secs(2))
        .unwrap();
    app.wait_for("26:4").unwrap();

    // 5. 历史提交重命名 Diff 单栏模式：右键旧侧 `-` 行与新侧 `+` 行，分别映射旧路径/旧行号与新路径/新行号！
    app.press_key("mod+9").unwrap();
    app.wait_for("id:git-bottom-pane").unwrap();
    app.wait_for("text:refactor: rename file to v2").unwrap();
    app.press("text:refactor: rename file to v2").unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.press("id:git-commit-file-src/rename_new.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:commit-diff-unified-editor").unwrap();

    app.right_click("- const Version = \"v1\"").unwrap();
    app.choose_menu_item("复制路径／引用 > 文件与行号").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src/rename_old.go:5"));

    app.right_click("+ const Version = \"v2\"").unwrap();
    app.choose_menu_item("复制路径／引用 > 文件与行号").unwrap();
    assert_eq!(app.screen().clipboard(), Some("src/rename_new.go:5"));
}

#[test]
fn ui_git_log_reviews_merge_files_with_separate_directories() {
    let _guard = lock_env();
    let fx = Fixture::new("git-log-merge");
    git_run(&fx.repo_dir, &["switch", "-c", "agent/review"]);
    for directory in ["config/development", "config/production"] {
        let directory = fx.repo_dir.join(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("application.yml"), "review: true\n").unwrap();
    }
    git_run(&fx.repo_dir, &["add", "config"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Update review settings"]);
    git_run(&fx.repo_dir, &["tag", "review-ready"]);
    git_run(&fx.repo_dir, &["switch", "main"]);
    fs::write(fx.repo_dir.join("main-only.txt"), "already on main\n").unwrap();
    git_run(&fx.repo_dir, &["add", "main-only.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Record main branch notes"]);
    git_run(&fx.repo_dir, &["merge", "--no-ff", "-m", "Merge agent review", "agent/review"]);

    let mut app = fx.start_headless(1040.0, 720.0);
    app.press_key("mod+9").unwrap();
    app.wait_for("id:git-bottom-pane >> text:/tag: review-ready/").unwrap();
    app.wait_for("id:git-bottom-pane >> text:/HEAD → main/").unwrap();
    let file = "id:git-commit-file-config/development/application.yml";
    app.wait_for(&format!("{file} >> text:application.yml")).unwrap();
    app.wait_for(&format!("{file} >> text:config/development")).unwrap();
    app.wait_for("id:git-commit-file-config/production/application.yml >> text:config/production")
        .unwrap();
    assert!(!app.screen().matches("id:git-commit-file-main-only.txt").unwrap());
    app.press("id:git-log-view-options").unwrap();
    app.choose_menu_item("列 > 提交哈希").unwrap();
    app.wait_for("id:git-commit-hash").unwrap();
    app.press("id:git-log-view-options").unwrap();
    app.choose_menu_item("列 > 日期").unwrap();
    app.wait_for_gone("id:git-commit-date", Duration::from_secs(2)).unwrap();
    app.wait_for("id:git-commit-author").unwrap();
    app.press("id:git-log-view-options").unwrap();
    app.choose_menu_item("列 > 作者").unwrap();
    app.wait_for_gone("id:git-commit-author", Duration::from_secs(2)).unwrap();
    app.press("id:git-log-view-options").unwrap();
    app.choose_menu_item("显示分支和标签").unwrap();
    app.wait_for_gone("id:git-bottom-pane >> text:/tag: review-ready/", Duration::from_secs(2)).unwrap();
    let columns = nemorous::config::load_from(&fx.config_file).unwrap().git_log_columns;
    assert!(columns.hash);
    assert!(!columns.date && !columns.author && !columns.refs);
    app.press("id:git-file-view-options").unwrap();
    app.choose_menu_item("按目录分组").unwrap();
    app.wait_for("id:git-file-directory-config/development").unwrap();
    app.wait_for("id:git-file-directory-config/production").unwrap();
    assert!(!app.screen().matches(&format!("{file} >> text:config/development")).unwrap());
    app.press(file).unwrap();
    assert!(!app.screen().matches("id:commit-diff-pane").unwrap());
    app.press_key("mod+d").unwrap();
    app.wait_for("id:commit-diff-pane").unwrap();
    app.wait_for("text:review: true").unwrap();
}

#[test]
fn ui_git_log_keeps_commit_subjects_readable_in_a_small_window() {
    let _guard = lock_env();
    let fx = Fixture::new("git-log-small-window");
    const SUBJECT: &str = "refactor: make renamed files easy to review and retain the full commit subject in history when details are hidden";
    git_run(&fx.repo_dir, &["commit", "--amend", "--no-gpg-sign", "-m", SUBJECT]);
    git_run(&fx.repo_dir, &["tag", "a-long-review-reference-that-remains-visible"]);
    let mut app = fx.start_headless(640.0, 500.0);
    app.press_key("mod+9").unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();

    let check_subject = |app: &Headless| {
        let nodes = app.screen().layout_nodes();
        let subject = nodes.iter().find_map(|node| {
            node.text.as_ref().filter(|text| text.text.text == SUBJECT)
        }).expect("commit subject");
        let list = nodes.iter().find(|node| node.test_id.as_deref() == Some("git-commit-list-scroll"))
            .expect("commit list").bounds;
        assert!(subject.content.size.width >= px(64.0), "commit subject got {:?}", subject.content);
        assert!(subject.content.origin.x >= list.origin.x);
        assert!(subject.content.origin.x + subject.content.size.width <= list.origin.x + list.size.width);
    };
    check_subject(&app);
    app.press("id:git-log-view-options").unwrap();
    app.choose_menu_item("列 > 提交哈希").unwrap();
    app.wait_for("id:git-commit-hash").unwrap();
    check_subject(&app);

    app.press("id:git-file-view-options").unwrap();
    app.choose_menu_item("显示提交详情").unwrap();
    app.wait_for_gone("id:git-commit-details", Duration::from_secs(2)).unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.hover(&format!("text:{SUBJECT}")).unwrap();
    app.wait_for("id:toolbar-tooltip").unwrap();
    let revision = nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", "--short", "HEAD"]).unwrap();
    let hint = format!("{SUBJECT}\nsample-repo · {}", revision.trim());
    assert!(app.screen().texts().contains(&hint.as_str()), "hover did not show {hint:?}");
    let tooltip = app.screen().layout_nodes().into_iter()
        .find(|node| node.test_id.as_deref() == Some("toolbar-tooltip"))
        .expect("subject tooltip");
    assert!(tooltip.bounds.size.width <= px(360.0));
    assert!(tooltip.bounds.size.height > px(44.0), "the long subject should wrap");
    assert!(tooltip.bounds.origin.x >= px(0.0));
    assert!(tooltip.bounds.origin.x + tooltip.bounds.size.width <= px(640.0));
    app.hover("id:git-log-title").unwrap();
    app.wait_for_gone("id:toolbar-tooltip", Duration::from_secs(2)).unwrap();
    app.set_viewport(size(px(1440.0), px(900.0))).unwrap();
    check_subject(&app);
    app.wait_for("id:git-commit-hash").unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
}

#[test]
fn ui_revert_and_reflog_soft_reset_preserve_the_expected_git_state() {
    let _guard = lock_env();
    let fx = Fixture::new("revert-and-reflog");
    git_run(&fx.repo_dir, &["stash", "push", "-u", "-m", "Prepare clean history fixture"]);
    let git = |args: &[&str]| {
        nemorous::git::run_git_string(&fx.repo_dir, args).unwrap().trim().to_owned()
    };
    let original = git(&["rev-parse", "HEAD"]);
    let original_short = git(&["rev-parse", "--short", "HEAD"]);
    let parent_tree = git(&["rev-parse", "HEAD^^{tree}"]);
    let mut app = fx.start_headless_expect(1440.0, 900.0, "id:changes-header-title-row");
    app.press_key("mod+9").unwrap();
    let original_row = format!("id:git-commit-row-{original_short}");
    app.wait_for(&original_row).unwrap();
    app.right_click(&original_row).unwrap();
    app.choose_menu_item("Revert 此提交…").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    assert_eq!(git(&["rev-parse", "HEAD"]), original);
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("Revert已完成").unwrap();
    assert_eq!(git(&["rev-parse", "HEAD^"]), original);
    assert_eq!(git(&["rev-parse", "HEAD^{tree}"]), parent_tree);
    assert_eq!(git(&["status", "--porcelain"]), "");
    let reverted_short = git(&["rev-parse", "--short", "HEAD"]);
    app.wait_for(&format!("id:git-commit-row-{reverted_short}")).unwrap();

    let staged = b"# Staged before soft reset\n";
    let unstaged = b"# Edited after staging\n";
    fs::write(fx.repo_dir.join("README.md"), staged).unwrap();
    git_run(&fx.repo_dir, &["add", "README.md"]);
    let index_tree = git(&["write-tree"]);
    fs::write(fx.repo_dir.join("README.md"), unstaged).unwrap();

    app.press("id:top-git-advanced-btn").unwrap();
    app.choose_menu_item("Reflog 与重置…").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    let reset_item = loop {
        let target = app.menu().into_iter().flatten().find(|item| {
            item.label.contains(&original[..8]) && item.label.ends_with(" > Soft 重置…")
        });
        if let Some(item) = target {
            break item.label;
        }
        assert!(std::time::Instant::now() < deadline, "original commit missing from reflog");
        app.wait(Duration::from_millis(25)).unwrap();
    };
    app.choose_menu_item(&reset_item).unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    assert_eq!(git(&["rev-parse", "--short", "HEAD"]), reverted_short);
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("重置已完成").unwrap();
    assert_eq!(git(&["rev-parse", "HEAD"]), original);
    assert_eq!(git(&["write-tree"]), index_tree);
    assert_eq!(fs::read(fx.repo_dir.join("README.md")).unwrap(), unstaged);
    app.wait_for_gone(&format!("id:git-commit-row-{reverted_short}"), Duration::from_secs(6)).unwrap();
    app.wait_for(&original_row).unwrap();
}

#[test]
fn ui_git_log_uses_local_author_dates_and_orders_repositories_by_instant() {
    let _guard = lock_env();
    let fx = Fixture::new("git-log-timezones");
    let commits = [
        ("older", "2025-01-02T02:00:00+14:00", "Earlier across the date line"),
        ("newer", "2025-01-01T18:00:00-12:00", "Later across the date line"),
    ];
    for (name, date, subject) in commits {
        let repo = fx.repo_dir.join(name);
        fs::create_dir(&repo).unwrap();
        git_run(&repo, &["init", "-b", "main"]);
        let file = format!("{name}.txt");
        fs::write(repo.join(&file), format!("{subject}\n")).unwrap();
        git_run(&repo, &["add", &file]);
        git_run(&repo, &["commit", "--date", date, "-m", subject]);
    }
    fs::write(fx.repo_dir.join("mani.yaml"), "projects:\n  older:\n    path: older\n  newer:\n    path: newer\n").unwrap();
    fs::write(&fx.config_file, serde_json::to_vec(&serde_json::json!({
        "workspace_path": fx.repo_dir,
        "include_root_repo": false,
    })).unwrap()).unwrap();

    let mut app = fx.start_headless_expect(1440.0, 900.0, "id:changes-header-title-row");
    app.press_key("mod+9").unwrap();
    for (_, _, subject) in commits {
        app.wait_for(&format!("id:git-bottom-pane >> text:{subject}")).unwrap();
    }
    let texts = app.screen().texts();
    let row = |subject| texts.iter().position(|text| *text == subject).unwrap();
    assert!(row(commits[1].2) < row(commits[0].2), "cross-repository order followed the author's wall clock");
    app.wait_for("id:git-commit-file-newer.txt").unwrap();

    let local_dates = commits.map(|(_, date, _)| {
        chrono::DateTime::parse_from_rfc3339(date).unwrap().with_timezone(&chrono::Local)
    });
    for date in &local_dates {
        app.wait_for(&format!("id:git-bottom-pane >> text:{}", date.format("%Y-%m-%d %H:%M"))).unwrap();
    }
    app.press("id:git-group-filter").unwrap();
    app.choose_menu_item("按日期").unwrap();
    let [earlier_day, later_day] = local_dates.map(|date| date.format("%Y-%m-%d").to_string());
    for day in [&earlier_day, &later_day] {
        app.wait_for(&format!("id:git-bottom-pane >> text:{day}")).unwrap();
    }
    let texts = app.screen().texts();
    let row = |text| texts.iter().position(|label| *label == text).unwrap();
    assert!(row(later_day.as_str()) < row(commits[1].2));
    if earlier_day == later_day {
        assert_eq!(texts.iter().filter(|text| **text == later_day).count(), 1);
    } else {
        assert!(row(commits[1].2) < row(earlier_day.as_str()));
        assert!(row(earlier_day.as_str()) < row(commits[0].2));
    }
}

#[test]
fn ui_git_forest_filters_previews_and_compares_file_history() {
    let _guard = lock_env();
    let fx = Fixture::new("git-forest");
    let short = |revision: &str| {
        let out = Command::new("git")
            .current_dir(&fx.repo_dir)
            .args(["rev-parse", "--short", revision])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    let current = format!("id:git-commit-row-{}", short("HEAD"));
    let initial = format!("id:git-commit-row-{}", short("HEAD^"));

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+9").unwrap();
    app.wait_for(current.as_str()).unwrap();
    app.wait_for(initial.as_str()).unwrap();
    app.type_text("id:git-filter-input", "INITIAL|missing").unwrap();
    app.wait_for_gone(current.as_str(), Duration::from_secs(4)).unwrap();
    app.wait_for(initial.as_str()).unwrap();
    for toggle in ["id:git-search-regex", "id:git-search-case"] {
        app.press(toggle).unwrap();
        app.wait_for_gone(initial.as_str(), Duration::from_secs(4)).unwrap();
        app.press(toggle).unwrap();
        app.wait_for(initial.as_str()).unwrap();
    }
    assert!(!app.screen().matches("id:git-author-filter").unwrap());
    app.press("id:git-author-filter-button").unwrap();
    app.type_text("id:git-author-filter", "我").unwrap();
    app.submit("id:git-author-filter").unwrap();
    app.wait_for_gone("id:git-author-filter", Duration::from_secs(2)).unwrap();
    app.press("id:git-path-filter-button").unwrap();
    app.type_text("id:git-path-filter", "*.go").unwrap();
    app.press("id:git-apply-filter").unwrap();
    app.wait_for_gone("id:git-path-filter", Duration::from_secs(2)).unwrap();
    app.wait_for(initial.as_str()).unwrap();
    app.press("id:git-clear-filters").unwrap();
    app.wait_for(current.as_str()).unwrap();
    app.press("id:git-group-filter").unwrap();
    app.choose_menu_item("按仓库").unwrap();
    app.press(current.as_str()).unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.press("id:git-file-view-options").unwrap();
    app.choose_menu_item("差异预览").unwrap();
    app.wait_for("id:commit-diff-pane").unwrap();
    app.press("id:git-file-view-options").unwrap();
    app.choose_menu_item("差异预览").unwrap();
    app.press("id:git-show-diff").unwrap();
    app.wait_for("id:commit-diff-pane").unwrap();

    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/rename_new.go").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.right_click(r#"const Version = "v2""#).unwrap();
    app.choose_menu_item("Git > 查看文件历史").unwrap();
    app.wait_for(current.as_str()).unwrap();
    app.wait_for(initial.as_str()).unwrap();
    app.press(current.as_str()).unwrap();
    app.press_key("down").unwrap();
    app.wait_for("id:git-commit-file-src/rename_old.go").unwrap();
    assert!(!app.screen().matches("id:git-commit-file-README.md").unwrap());
    app.press("id:git-file-view-options").unwrap();
    app.choose_menu_item("显示所有受影响的文件").unwrap();
    app.wait_for("id:git-commit-file-README.md").unwrap();
    app.press("id:git-commit-file-README.md").unwrap();
    app.press("id:git-file-view-options").unwrap();
    app.choose_menu_item("显示所有受影响的文件").unwrap();
    app.wait_for_gone("id:git-commit-file-README.md", Duration::from_secs(2)).unwrap();
    app.press("id:git-show-diff").unwrap();
    app.wait_for("id:commit-diff-new-editor").unwrap();
    app.wait_for(r#"const Version = "v1""#).unwrap();
    app.right_click(initial.as_str()).unwrap();
    app.choose_menu_item("设为比较基准").unwrap();
    app.right_click(current.as_str()).unwrap();
    app.choose_menu_item(&format!("与修订 {} 比较…", short("HEAD^"))).unwrap();
    app.wait_for("id:comparison-pane").unwrap();
    app.wait_for(r#"const Version = "v1""#).unwrap();
    app.wait_for(r#"const Version = "v2""#).unwrap();
}

#[test]
fn ui_git_log_view_options_keep_the_selected_file_and_diff_position() {
    let _guard = lock_env();
    let fx = Fixture::new("git-log-retains-selection");
    let directory = fx.repo_dir.join("review");
    fs::create_dir(&directory).unwrap();
    let path = directory.join("selected.txt");
    let original = (1..=230).map(|line| format!("baseline {line:03}\n")).collect::<String>();
    fs::write(&path, &original).unwrap();
    git_run(&fx.repo_dir, &["add", "review/selected.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Create the long review file"]);
    fs::write(&path, original.replace("baseline 092", "FIRST_VIEW_CHANGE").replace("baseline 197", "LAST_VIEW_CHANGE")).unwrap();
    fs::write(fx.repo_dir.join("a-first.txt"), "OTHER_REVIEW_FILE\n").unwrap();
    let extra = fx.repo_dir.join("other");
    fs::create_dir(&extra).unwrap();
    for index in 0..36 {
        fs::write(extra.join(format!("{index:02}.txt")), "an additional changed file\n").unwrap();
    }
    git_run(&fx.repo_dir, &["add", "a-first.txt", "other", "review/selected.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Review two files together"]);

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+9").unwrap();
    let commit = format!("id:git-commit-row-{}", nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", "--short", "HEAD"]).unwrap().trim());
    let selected = "id:git-commit-file-review/selected.txt";
    let last_change = "id:commit-diff-new-editor >> text:LAST_VIEW_CHANGE";
    app.wait_for("id:git-commit-file-a-first.txt").unwrap();
    app.scroll("id:git-commit-file-list-scroll", px(2_000.0)).unwrap();
    app.wait_for(selected).unwrap();
    app.press(selected).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:commit-diff-new-editor >> text:FIRST_VIEW_CHANGE").unwrap();
    app.press("id:diff-next-hunk").unwrap();
    app.wait_for(last_change).unwrap();

    let choose = |app: &mut Headless, option: &str| {
        app.press("id:git-file-view-options").unwrap();
        app.choose_menu_item(option).unwrap();
    };
    choose(&mut app, "差异预览");
    app.wait(Duration::from_millis(350)).unwrap();
    app.wait_for(last_change).unwrap();
    app.press(&commit).unwrap();
    app.wait(Duration::from_millis(350)).unwrap();
    app.wait_for(last_change).unwrap();
    app.press(selected).unwrap();
    app.wait(Duration::from_millis(350)).unwrap();
    app.wait_for(last_change).unwrap();
    app.wait_for("text:2/2").unwrap();

    app.wait_for("id:git-commit-details").unwrap();
    choose(&mut app, "显示提交详情");
    app.wait_for_gone("id:git-commit-details", Duration::from_secs(2)).unwrap();
    app.wait_for(selected).unwrap();
    assert!(!nemorous::config::load_from(&fx.config_file).unwrap().git_log_show_details);
    choose(&mut app, "按目录分组");
    app.wait_for("id:git-file-directory-review").unwrap();
    app.wait_for(last_change).unwrap();
    choose(&mut app, "显示提交详情");
    app.wait_for("id:git-commit-details").unwrap();
    app.wait_for(selected).unwrap();
    assert!(nemorous::config::load_from(&fx.config_file).unwrap().git_log_show_details);
    app.wait_for(last_change).unwrap();
    app.press("id:git-show-diff").unwrap();
    app.wait_for("id:commit-diff-new-editor >> text:FIRST_VIEW_CHANGE").unwrap();
}

#[test]
fn ui_git_log_refreshes_external_refs_and_preserves_the_active_diff() {
    let _guard = lock_env();
    let fx = Fixture::new("git-log-external-refs");
    let initial = nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", "HEAD"]).unwrap();
    let initial = initial.trim();
    git_run(&fx.repo_dir, &["pack-refs", "--all", "--prune"]);
    let git_dir = fx.repo_dir.join(".git");
    let packed_refs = git_dir.join("packed-refs");
    let packed_baseline = fs::read_to_string(&packed_refs).unwrap();
    let head = nemorous::git::run_git_string(&fx.repo_dir, &["rev-parse", "HEAD"]).unwrap();
    let head = head.trim();
    let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    cfg.git_log_diff_preview = true;
    nemorous::config::save_to(&fx.config_file, &cfg).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    let old_review = "id:commit-diff-new-editor >> text:const Version = \"v2\"";
    app.press_key("mod+9").unwrap();
    app.wait_for(old_review).unwrap();
    app.type_text("id:git-filter-input", "rename|audit")
        .unwrap();
    app.wait_for_gone("text:feat: initial fixture", Duration::from_secs(4))
        .unwrap();

    // This ref changes only through the atomic replacement of packed-refs.
    let replace_packed_refs = |contents: &str| {
        let lock = git_dir.join("packed-refs.lock");
        fs::write(&lock, contents).unwrap();
        fs::rename(lock, &packed_refs).unwrap();
    };
    replace_packed_refs(&format!("{packed_baseline}{head} refs/tags/zz-packed-review\n"));
    assert!(!git_dir.join("refs/tags/zz-packed-review").exists());
    let packed_tag = "id:git-bottom-pane >> text:/tag: zz-packed-review/";
    app.wait_for(packed_tag).unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.wait_for(old_review).unwrap();
    replace_packed_refs(&packed_baseline);
    app.wait_for_gone(packed_tag, Duration::from_secs(5)).unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.wait_for(old_review).unwrap();

    git_run(&fx.repo_dir, &["tag", "published.tmp"]);
    app.wait_for("id:git-bottom-pane >> text:/tag: published.tmp/")
        .unwrap();
    git_run(&fx.repo_dir, &["branch", "external-branch"]);
    app.wait_for("id:git-bottom-pane >> text:/external-branch/")
        .unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go")
        .unwrap();
    app.wait_for(old_review).unwrap();

    git_run(
        &fx.repo_dir,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "Excluded by the active filter",
        ],
    );
    fs::write(fx.repo_dir.join("audit.txt"), "external audit content\n").unwrap();
    git_run(&fx.repo_dir, &["add", "audit.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "audit external review"]);
    app.wait_for("text:audit external review").unwrap();
    assert!(
        !app.screen()
            .matches("text:Excluded by the active filter")
            .unwrap()
    );
    app.wait_for("id:git-commit-file-src/rename_new.go")
        .unwrap();
    app.wait_for(old_review).unwrap();

    app.press("text:audit external review").unwrap();
    let review = "id:commit-diff-new-editor >> text:external audit content";
    app.wait_for(review).unwrap();
    // Removing the selected row from the log should select a visible commit
    // for its file list while leaving the existing review on screen.
    git_run(&fx.repo_dir, &["update-ref", "HEAD", initial]);
    app.wait_for_gone("text:audit external review", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go")
        .unwrap();
    app.wait_for(review).unwrap();
}

#[test]
fn ui_file_history_scope_leaves_opened_diff_navigation_and_visible_selection_intact() {
    let _guard = lock_env();
    let fx = Fixture::new("file-history-navigation-scope");
    let tracked = fx.repo_dir.join("review.txt");
    fs::write(&tracked, "ORIGINAL_REVIEW\n").unwrap();
    git_run(&fx.repo_dir, &["add", "review.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add review target"]);
    for (path, line) in [
        ("a-before.txt", "NEIGHBOR_BEFORE"),
        ("review.txt", "HISTORY_CONTENT"),
        ("z-after.txt", "NEIGHBOR_AFTER"),
    ] {
        fs::write(fx.repo_dir.join(path), format!("{line}\n")).unwrap();
    }
    git_run(&fx.repo_dir, &["add", "a-before.txt", "review.txt", "z-after.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Review three affected files"]);

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "review.txt").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.right_click("id:code-editor-view >> text:HISTORY_CONTENT").unwrap();
    app.choose_menu_item("Git > 查看文件历史").unwrap();
    let visible_file = "id:git-commit-file-review.txt";
    let first_file = "id:git-commit-file-a-before.txt";
    let last_file = "id:git-commit-file-z-after.txt";
    let visible_diff = "id:commit-diff-new-editor >> text:HISTORY_CONTENT";
    let first_diff = "id:commit-diff-new-editor >> text:NEIGHBOR_BEFORE";
    let last_diff = "id:commit-diff-new-editor >> text:NEIGHBOR_AFTER";
    let toggle_scope = |app: &mut Headless| {
        app.press("id:git-file-view-options").unwrap();
        app.choose_menu_item("显示所有受影响的文件").unwrap();
    };
    app.wait_for(visible_file).unwrap();
    toggle_scope(&mut app);
    app.wait_for(first_file).unwrap();
    app.press(first_file).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(first_diff).unwrap();

    toggle_scope(&mut app);
    app.wait_for_gone(first_file, Duration::from_secs(2)).unwrap();
    app.wait_for(visible_file).unwrap();
    app.wait_for(first_diff).unwrap();
    app.press("id:diff-next-file").unwrap();
    app.wait_for(visible_diff).unwrap();
    app.press("id:diff-next-file").unwrap();
    app.wait_for(last_diff).unwrap();
    assert!(!app.screen().matches(last_file).unwrap());

    app.press("id:git-show-diff").unwrap();
    app.wait_for(visible_diff).unwrap();
    assert!(!app.screen().enabled("id:diff-next-file").unwrap());
    assert!(!app.screen().enabled("id:diff-prev-file").unwrap());
    toggle_scope(&mut app);
    app.wait_for(last_file).unwrap();
    app.press_key("alt+right").unwrap();
    assert!(app.screen().matches(visible_diff).unwrap());
    app.press_key("mod+[").unwrap();
    app.wait_for(last_diff).unwrap();
    app.press("id:diff-prev-file").unwrap();
    app.wait_for(visible_diff).unwrap();
}

#[test]
fn ui_file_history_follows_head_and_preserves_the_selected_revision() {
    let _guard = lock_env();
    let fx = Fixture::new("file-history-external-commit");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/rename_new.go")
        .unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view >> text:const Version = \"v2\"")
        .unwrap();
    app.right_click(r#"const Version = "v2""#).unwrap();
    app.choose_menu_item("Git > 查看文件历史").unwrap();
    app.wait_for("id:git-bottom-pane >> text:feat: initial fixture")
        .unwrap();
    app.press("text:feat: initial fixture").unwrap();
    app.wait_for("id:git-commit-file-src/rename_old.go")
        .unwrap();
    app.press("id:git-file-view-options").unwrap();
    app.choose_menu_item("显示所有受影响的文件").unwrap();
    app.wait_for("id:git-commit-file-README.md").unwrap();
    app.press("id:git-show-diff").unwrap();
    let review = r#"id:commit-diff-new-editor >> text:const Version = "v1""#;
    app.wait_for(review).unwrap();

    let path = fx.repo_dir.join("src/rename_new.go");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, original.replace("v2", "v3")).unwrap();
    git_run(&fx.repo_dir, &["add", "src/rename_new.go"]);
    git_run(
        &fx.repo_dir,
        &["commit", "-m", "External file history update"],
    );
    app.wait_for("id:git-bottom-pane >> text:External file history update")
        .unwrap();
    app.wait_for("id:git-commit-file-README.md").unwrap();
    app.wait_for(review).unwrap();

    app.right_click("id:git-commit-file-src/rename_old.go")
        .unwrap();
    app.choose_menu_item("查看截至此处的历史").unwrap();
    app.wait_for("文件历史：rename_old.go").unwrap();
    app.wait_for("id:git-bottom-pane >> text:feat: initial fixture")
        .unwrap();
    fs::write(
        fx.repo_dir.join("src/rename_old.go"),
        "external resurrection\n",
    )
    .unwrap();
    git_run(&fx.repo_dir, &["add", "src/rename_old.go"]);
    git_run(
        &fx.repo_dir,
        &["commit", "-m", "Commit after pinned revision"],
    );
    fs::write(
        fx.repo_dir.join("agent-file-history-check.txt"),
        "refresh marker\n",
    )
    .unwrap();
    app.wait_for("id:change-row-unstaged-agent-file-history-check.txt")
        .unwrap();
    assert!(
        !app.screen()
            .matches("text:Commit after pinned revision")
            .unwrap()
    );
    app.wait_for("id:git-bottom-pane >> text:feat: initial fixture")
        .unwrap();
}

#[test]
fn ui_blame_reveals_real_commit_and_marks_editor_only_changes() {
    let _guard = lock_env();
    let fx = Fixture::new("blame");
    let original = fs::read(fx.repo_dir.join("src/app.go")).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:3").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.right_click("func hello() string {").unwrap();
    app.choose_menu_item("显示整文件 Git Blame").unwrap();
    app.wait_for("id:blame-gutter").unwrap();
    app.wait_for("id:blame-line-3").unwrap();
    app.wait_for("未提交的本地修改").unwrap();
    app.press("id:blame-line-3").unwrap();
    app.wait_for("id:git-bottom-pane").unwrap();
    app.wait_for("text:feat: initial fixture").unwrap();
    app.wait_for("id:git-commit-file-src/app.go").unwrap();

    app.press("func hello() string {").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "3:1").unwrap();
    app.press_key("enter").unwrap();
    app.press_key("x").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    app.right_click("xfunc hello() string {").unwrap();
    app.choose_menu_item("只显示该行 Git Blame").unwrap();
    app.wait_for_gone("id:blame-gutter", Duration::from_secs(3)).unwrap();
    app.wait_for("id:blame-line-3").unwrap();
    app.wait_for("未提交的本地修改").unwrap();
    assert_eq!(fs::read(fx.repo_dir.join("src/app.go")).unwrap(), original);
}

#[test]
fn ui_source_attribution_follows_scroll_and_excludes_folded_rows() {
    let _guard = lock_env();
    let fx = Fixture::new("source-attribution-viewport");
    let mut source = String::from("fn review_scope() {\n    first_review_line();\n    folded_review_line();\n}\n");
    for row in 5..=140 {
        source.push_str(&format!("// Review line {row:03}\n"));
    }
    fs::write(fx.repo_dir.join("review.rs"), source).unwrap();
    git_run(&fx.repo_dir, &["add", "review.rs"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Source attribution test fixture"]);

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "review.rs:1").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.right_click("fn review_scope() {").unwrap();
    app.choose_menu_item("显示整文件 Git Blame").unwrap();
    app.wait_for("id:blame-line-3").unwrap();
    app.hover("id:blame-line-1").unwrap();
    app.wait_for("id:toolbar-tooltip >> text:/Source attribution test fixture/").unwrap();

    app.press("fn review_scope() {").unwrap();
    app.press_key("mod+alt+[").unwrap();
    app.wait_for_gone("folded_review_line();", Duration::from_secs(3)).unwrap();
    app.wait_for_gone("id:blame-line-3", Duration::from_secs(3)).unwrap();
    app.wait_for("id:blame-line-1").unwrap();
    app.press_key("mod+alt+]").unwrap();
    app.wait_for("id:blame-line-3").unwrap();

    app.wheel("id:code-editor-view", point(px(0.0), px(800.0))).unwrap();
    app.wait_for_gone("id:blame-line-1", Duration::from_secs(3)).unwrap();
    app.wait_for("id:blame-line-50").unwrap();
    app.wheel("id:code-editor-view", point(px(0.0), px(-1200.0))).unwrap();
    app.wait_for("id:blame-line-1").unwrap();

    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "notes.txt").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.right_click("untracked note line 1").unwrap();
    app.choose_menu_item("显示整文件 Git Blame").unwrap();
    app.wait_for("id:blame-line-1 >> text:未提交的本地修改").unwrap();
}

#[test]
fn ui_repository_settings_distinguishes_git_lfs_from_repository_setup() {
    let _guard = lock_env();
    let fx = Fixture::new("repository-lfs");
    let has_lfs = Command::new("git")
        .args(["lfs", "version"])
        .current_dir(&fx.repo_dir)
        .output()
        .unwrap()
        .status
        .success();
    if !has_lfs {
        git_run(&fx.repo_dir, &["config", "--local", "filter.lfs.clean", "git-lfs clean -- %f"]);
    }
    let mut app = fx.start_headless(1440.0, 900.0);
    let open_repository_settings = |app: &mut Headless| {
        app.press("id:top-git-advanced-btn").unwrap();
        app.choose_menu_item("标签、远端、子模块与仓库配置…").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while app.menu().is_none() {
            assert!(std::time::Instant::now() < deadline, "repository settings menu did not open");
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    open_repository_settings(&mut app);
    if !has_lfs {
        let items = app.menu().unwrap();
        assert!(items.iter().any(|item| item.label == "Git LFS · 未安装" && item.disabled));
        assert!(!items.iter().any(|item| item.label.contains("拉取 LFS 内容")));
        return;
    }

    app.choose_menu_item("Git LFS > 在此仓库初始化").unwrap();
    app.wait_for("初始化 Git LFS已完成").unwrap();
    open_repository_settings(&mut app);
    app.choose_menu_item("Git LFS · 仓库已初始化 > 跟踪模式…").unwrap();
    app.type_text("id:git-text-prompt-input", "*.reviewasset").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("Git LFS 跟踪模式已完成").unwrap();
    let attributes = fs::read_to_string(fx.repo_dir.join(".gitattributes")).unwrap();
    assert!(attributes.contains("*.reviewasset filter=lfs"));

    let payload = b"Nemorous Git LFS fixture\n";
    let asset = fx.repo_dir.join("sample.reviewasset");
    fs::write(&asset, payload).unwrap();
    git_run(&fx.repo_dir, &["add", ".gitattributes", "sample.reviewasset"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Track synthetic LFS payload"]);
    let origin = fx.root.join("lfs-origin.git");
    fs::create_dir(&origin).unwrap();
    git_run(&origin, &["init", "--bare", "-b", "main"]);
    git_run(&fx.repo_dir, &["remote", "add", "origin", &format!("file://{}", origin.display())]);
    git_run(&fx.repo_dir, &["push", "-u", "origin", "main"]);
    let pointer = Command::new("git")
        .args(["show", "HEAD:sample.reviewasset"])
        .current_dir(&fx.repo_dir)
        .output()
        .unwrap();
    assert!(pointer.status.success() && pointer.stdout.starts_with(b"version https://git-lfs.github.com/spec/v1\n"));
    let storage = fx.root.join("lfs-empty-cache");
    git_run(&fx.repo_dir, &["config", "--local", "lfs.storage", storage.to_str().unwrap()]);
    fs::write(&asset, &pointer.stdout).unwrap();
    open_repository_settings(&mut app);
    app.choose_menu_item("Git LFS · 仓库已初始化 > 拉取 LFS 内容").unwrap();
    app.wait_for("拉取 Git LFS已完成").unwrap();
    assert_eq!(fs::read(&asset).unwrap(), payload);
}

#[test]
fn ui_branch_matrix_shortcut_filter_create_and_switch() {
    let _guard = lock_env();
    let fx = Fixture::new("branch-matrix");
    git_run(&fx.repo_dir, &["branch", "known/topic"]);
    git_run(&fx.repo_dir, &["restore", "--", "src/app.go"]);
    let mut app = fx.start_headless_expect(1440.0, 900.0, "notes.txt");
    app.press_key("mod+b").unwrap();
    app.wait_for("id:branch-matrix").unwrap();
    app.wait_for("known/topic").unwrap();
    app.type_text("id:branch-filter-input", "known").unwrap();
    app.wait_for("known/topic").unwrap();
    assert!(!app.screen().matches("✓ main").unwrap());
    app.press_key("escape").unwrap();
    assert!(!app.screen().matches("id:branch-matrix").unwrap());

    app.press("id:top-branch-button").unwrap();
    app.press("id:branch-new-all").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    app.type_text("id:git-text-prompt-input", "feature/local").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("已切换 1 个仓库").unwrap();
    assert_eq!(
        nemorous::git::run_git_string(&fx.repo_dir, &["symbolic-ref", "--short", "HEAD"]).unwrap().trim(),
        "feature/local"
    );

    fs::remove_file(fx.repo_dir.join("notes.txt")).unwrap();
    app.press_key("mod+b").unwrap();
    app.press("id:branch-default-all").unwrap();
    for _ in 0..80 {
        if nemorous::git::run_git_string(&fx.repo_dir, &["symbolic-ref", "--short", "HEAD"]).unwrap().trim() == "main" {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
        let _ = app.screen();
    }
    panic!("workspace default branch switch did not complete");
}

#[test]
fn ui_change_lists_create_assign_filter_and_rename() {
    let _guard = lock_env();
    let fx = Fixture::new("change-lists");
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:changes-list-btn").unwrap();
    app.choose_menu_item("新建变更列表…").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    app.type_text("id:git-text-prompt-input", "本轮修改").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("没有匹配的本地变更").unwrap();

    app.press("id:changes-list-btn").unwrap();
    app.choose_menu_item("全部变更").unwrap();
    app.wait_for("id:change-row-unstaged-src/app.go").unwrap();
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("移到变更列表 > 本轮修改").unwrap();

    app.press("id:changes-list-btn").unwrap();
    app.choose_menu_item("本轮修改 (1 个文件)").unwrap();
    app.wait_for("id:change-row-unstaged-src/app.go").unwrap();
    assert!(!app.screen().matches("id:change-row-unstaged-notes.txt").unwrap());
    app.press("id:changes-list-btn").unwrap();
    app.choose_menu_item("重命名当前列表…").unwrap();
    app.clear("id:git-text-prompt-input").unwrap();
    app.type_text("id:git-text-prompt-input", "复核").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    let saved = nemorous::config::load_from(&fx.config_file).unwrap();
    assert_eq!(saved.active_change_list, "复核");
    assert_eq!(saved.change_lists[0].files.len(), 1);
}

#[test]
fn ui_diff_stages_and_unstages_only_the_clicked_line_in_both_layouts() {
    let _guard = lock_env();
    for single in [true, false] {
        let fx = Fixture::new("stage-clicked-line");
        git_run(&fx.repo_dir, &["config", "core.autocrlf", "false"]);
        let path = fx.repo_dir.join("lines.txt");
        let base = "top\r\nold A\r\nold B\r\nold C\r\nbridge\r\ntail\r\n";
        let initially_staged = base.replace("tail", "pre-staged tail");
        let working = initially_staged.replace("old A", "new A").replace("old B", "new B").replace("old C", "new C");
        fs::write(&path, base).unwrap();
        git_run(&fx.repo_dir, &["add", "lines.txt"]);
        git_run(&fx.repo_dir, &["commit", "-m", "Add line staging fixture"]);
        fs::write(&path, &initially_staged).unwrap();
        git_run(&fx.repo_dir, &["add", "lines.txt"]);
        fs::write(&path, &working).unwrap();
        let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
        cfg.diff_single_column = single;
        nemorous::config::save_to(&fx.config_file, &cfg).unwrap();

        let mut app = fx.start_headless(1440.0, 900.0);
        app.press("id:change-row-unstaged-lines.txt").unwrap();
        app.press_key("mod+d").unwrap();
        let (a, b, staged_b) = if single {
            ("id:working-diff-unified-editor >> text:+ new A", "id:working-diff-unified-editor >> text:+ new B",
                "id:working-diff-unified-editor >> text:- old B")
        } else {
            ("id:working-diff-new-editor >> text:new A", "id:working-diff-new-editor >> text:new B",
                "id:working-diff-old-editor >> text:old B")
        };
        let read_index = || {
            let blob = Command::new("git").current_dir(&fx.repo_dir).args(["show", ":lines.txt"]).output().unwrap();
            assert!(blob.status.success());
            blob.stdout
        };
        assert_eq!(read_index(), initially_staged.as_bytes());
        app.wait_for(a).unwrap();
        app.press(a).unwrap();
        app.right_click(b).unwrap();
        app.choose_menu_item("暂存选中行").unwrap();
        app.wait_for("已暂存所选变更").unwrap();
        assert_eq!(read_index(), initially_staged.replace("old B", "new B").as_bytes());
        assert_eq!(fs::read(&path).unwrap(), working.as_bytes());

        open_working_change(&mut app, "staged", "lines.txt");
        let staged_scope = if single {
            "id:working-diff-unified-editor >> text:+ pre-staged tail"
        } else {
            "id:diff-old-pane-bar >> text:HEAD · lines.txt"
        };
        app.wait_for(staged_scope).unwrap();
        app.wait_for(staged_b).unwrap();
        app.right_click(staged_b).unwrap();
        app.choose_menu_item("取消暂存选中行").unwrap();
        app.wait_for("已取消暂存所选变更").unwrap();
        assert_eq!(read_index(), initially_staged.as_bytes());
        assert_eq!(fs::read(&path).unwrap(), working.as_bytes());
    }
}

#[test]
fn ui_unified_diff_stages_a_mixed_selection_without_including_the_gap() {
    let _guard = lock_env();
    let fx = Fixture::new("stage-unified-mixed-selection");
    let path = fx.repo_dir.join("lines.txt");
    let base = "top\nold A\nold B\nold C\nkeep\nlast\n";
    let working = base.replace("old ", "new ");
    fs::write(&path, base).unwrap();
    git_run(&fx.repo_dir, &["add", "lines.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add mixed selection fixture"]);
    fs::write(&path, &working).unwrap();
    let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    cfg.diff_single_column = true;
    nemorous::config::save_to(&fx.config_file, &cfg).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-lines.txt").unwrap();
    app.press_key("mod+d").unwrap();
    let row = "id:working-diff-unified-editor >> text:- old C";
    app.wait_for(row).unwrap();
    app.press(row).unwrap();
    app.press_key("mod+left").unwrap();
    for _ in 0..2 {
        app.press_key("shift+down").unwrap();
    }
    app.press_key("mod+c").unwrap();
    assert_eq!(app.screen().clipboard(), Some("- old C\n+ new A\n"));
    app.right_click("id:working-diff-unified-editor >> text:+ new A").unwrap();
    app.choose_menu_item("暂存选中行").unwrap();
    app.wait_for("已暂存所选变更").unwrap();
    let index = Command::new("git").current_dir(&fx.repo_dir).args(["show", ":lines.txt"]).output().unwrap();
    assert!(index.status.success());
    assert_eq!(index.stdout, b"top\nnew A\nold B\nnew C\nkeep\nlast\n");
    assert_eq!(fs::read(&path).unwrap(), working.as_bytes());
}

#[test]
fn ui_diff_line_staging_rejects_a_stale_open_menu() {
    let _guard = lock_env();
    let fx = Fixture::new("stage-line-stale-menu");
    let path = fx.repo_dir.join("lines.txt");
    let base = "old A\nold B\nseparate\nlast\n";
    let working = "new A\nnew B\nseparate\nlast\n";
    let external_index = "old A\nold B\nseparate\nexternal last\n";
    fs::write(&path, base).unwrap();
    git_run(&fx.repo_dir, &["add", "lines.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add staging race fixture"]);
    fs::write(&path, working).unwrap();
    let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    cfg.diff_single_column = true;
    nemorous::config::save_to(&fx.config_file, &cfg).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:change-row-unstaged-lines.txt").unwrap();
    app.press_key("mod+d").unwrap();
    let row = "id:working-diff-unified-editor >> text:+ new A";
    app.wait_for(row).unwrap();
    app.right_click(row).unwrap();
    fs::write(&path, external_index).unwrap();
    git_run(&fx.repo_dir, &["add", "lines.txt"]);
    fs::write(&path, working).unwrap();
    app.choose_menu_item("暂存选中行").unwrap();
    app.wait_for("text:/(差异内容已变化，请重新选择|暂存区已变化，请刷新差异|工作区文件已变化，请刷新差异)/").unwrap();
    let index = Command::new("git").current_dir(&fx.repo_dir).args(["show", ":lines.txt"]).output().unwrap();
    assert!(index.status.success());
    assert_eq!(index.stdout, external_index.as_bytes());
    assert_eq!(fs::read(&path).unwrap(), working.as_bytes());
}

#[test]
fn ui_diff_stages_one_hunk_and_unstages_it_without_writing_worktree() {
    let _guard = lock_env();
    let fx = Fixture::new("stage-hunk");
    let context = (1..20).map(|n| format!("context {n}\n")).collect::<String>();
    let path = fx.repo_dir.join("many.txt");
    let old = format!("old first\n{context}old last\n");
    let new = format!("new first\n{context}new last\n");
    fs::write(&path, &old).unwrap();
    git_run(&fx.repo_dir, &["add", "many.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "fixture"]);
    fs::write(&path, &new).unwrap();

    let mut app = fx.start_headless_expect(1440.0, 900.0, "id:change-row-unstaged-many.txt");
    app.press("id:change-row-unstaged-many.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:diff-toolbar").unwrap();
    assert!(!app.screen().matches("id:diff-stage-hunk").unwrap());
    app.right_click("id:working-diff-new-editor >> context 1").unwrap();
    assert!(!app.menu().unwrap().iter().any(|item| matches!(item.label.as_str(), "暂存此块" | "暂存选中行")));
    app.close_menu().unwrap();
    app.right_click("new last").unwrap();
    app.choose_menu_item("暂存此块").unwrap();
    app.wait_for("id:change-row-staged-many.txt").unwrap();
    let read = || nemorous::git::run_git_string(&fx.repo_dir, &["show", ":many.txt"]).unwrap();
    assert_eq!(read(), format!("old first\n{context}new last\n"));
    assert_eq!(fs::read_to_string(&path).unwrap(), new);
    open_working_change(&mut app, "staged", "many.txt");
    app.wait_for("id:diff-old-pane-bar >> text:HEAD · many.txt").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor >> text:+ new last").unwrap();
    app.right_click("+ new last").unwrap();
    app.choose_menu_item("取消暂存此块").unwrap();
    app.wait_for("已取消暂存所选变更").unwrap();
    assert_eq!(read(), old);
    assert_eq!(fs::read_to_string(&path).unwrap(), new);
}

#[test]
#[cfg(target_os = "macos")]
fn ui_diff_keeps_last_comparison_when_git_objects_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("worktree-diff-baseline-error");
    let bin = fx.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, r#"#!/bin/sh
if [ "$3 $4" = 'cat-file blob' ] && [ -f ../fail-review-baseline ]; then
    case "$5" in
        *src/app.go|"$(/usr/bin/git -C . rev-parse :src/app.go 2>/dev/null)")
            echo "fatal: Not a valid object name $5" >&2
            exit 128
            ;;
    esac
fi
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0, 900.0);
    let row = "id:change-row-unstaged-src/app.go";
    let issue = "id:diff-disk-state >> text:Git 比较基准暂不可读";
    let old = "id:working-diff-old-editor >> text:/old-greeting/";
    let new = "id:working-diff-new-editor >> text:/new-greeting/";
    let fail = fx.root.join("fail-review-baseline");

    fs::write(&fail, "").unwrap();
    app.press(row).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("text:/读取工作区差异失败/").unwrap();
    assert!(!app.screen().matches("id:working-diff-pane").unwrap());
    fs::remove_file(&fail).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for(old).unwrap();
    app.wait_for(new).unwrap();

    fs::write(&fail, "").unwrap();
    app.press_key("mod+alt+y").unwrap();
    app.wait_for(issue).unwrap();
    app.wait_for(old).unwrap();
    app.wait_for(new).unwrap();
    app.right_click(new).unwrap();
    assert!(!app.menu().unwrap().iter().any(|item| item.label == "暂存此块"));
    app.close_menu().unwrap();

    fs::remove_file(&fail).unwrap();
    app.press_key("mod+alt+y").unwrap();
    app.wait_for_gone(issue, Duration::from_secs(4)).unwrap();
    app.wait_for(old).unwrap();
    app.wait_for(new).unwrap();
}

#[test]
fn ui_index_review_requires_a_readable_worktree_side_or_real_deletion() {
    let _guard = lock_env();
    let fx = Fixture::new("index-review-unavailable-worktree");
    git_run(&fx.repo_dir, &["add", "src/app.go"]);
    let path = fx.repo_dir.join("src/app.go");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    let row = "id:change-row-staged-src/app.go";
    let mut app = fx.start_headless_expect(1440.0, 900.0, row);
    let open = |app: &mut Headless| {
        app.right_click(row).unwrap();
        app.choose_menu_item("HEAD、暂存区与工作区对比").unwrap();
    };
    open(&mut app);
    app.wait_for("text:/无法读取工作区文件/").unwrap();
    assert!(!app.screen().matches("id:index-review-middle").unwrap());
    assert!(!app.screen().matches("id:index-save").unwrap());

    app.press("id:index-close").unwrap();
    fs::remove_dir(&path).unwrap();
    open(&mut app);
    app.wait_for("id:index-review-middle >> text:/new-greeting/").unwrap();
    app.wait_for("id:index-review-worktree").unwrap();
    assert!(!app.screen().matches("id:index-review-worktree >> text:/new-greeting/").unwrap());
    assert!(!path.exists());
}

#[test]
fn ui_index_review_saves_edits_and_preserves_worktree() {
    let _guard = lock_env();
    let fx = Fixture::new("index-review");
    git_run(&fx.repo_dir, &["add", "src/app.go"]);
    let path = fx.repo_dir.join("src/app.go");
    let worktree = "package main\n// local only\n";
    fs::write(&path, worktree).unwrap();
    let mut app = fx.start_headless_expect(1440.0, 900.0, "id:change-row-staged-src/app.go");
    app.right_click("id:change-row-staged-src/app.go").unwrap();
    app.choose_menu_item("HEAD、暂存区与工作区对比").unwrap();
    app.wait_for("id:index-review-head").unwrap();
    app.wait_for("id:index-review-worktree").unwrap();
    app.type_text("id:index-review-middle", "").unwrap();
    app.press_key("mod+a").unwrap();
    app.type_text("id:index-review-middle", "package main\n// saved index only\n").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for("已保存暂存版本").unwrap();
    let read = || nemorous::git::run_git_string(&fx.repo_dir, &["show", ":src/app.go"]).unwrap();
    assert_eq!(read(), "package main\n// saved index only\n");
    assert_eq!(fs::read_to_string(&path).unwrap(), worktree);
    app.press_key("escape").unwrap();
    assert!(!app.screen().matches("id:git-index-review").unwrap());
}

#[test]
fn ui_commit_checked_hunks_preserves_unselected_hunks_and_staged_files() {
    let _guard = lock_env();
    let _ = env_logger::builder().is_test(true).try_init();
    let fx = Fixture::new("commit-hunk");
    let context = (1..20).map(|n| format!("context {n}\n")).collect::<String>();
    let path = fx.repo_dir.join("many.txt");
    let old = format!("old first\n{context}old last\n");
    let new = format!("new first\n{context}new last\n");
    fs::write(&path, &old).unwrap();
    git_run(&fx.repo_dir, &["add", "many.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "fixture"]);
    git_run(&fx.repo_dir, &["add", "notes.txt"]);
    fs::write(&path, &new).unwrap();

    let mut cfg = nemorous::config::load_from(&fx.config_file).unwrap();
    cfg.create_change_list("This change").unwrap();
    cfg.assign_change_list("This change", &["sample-repo::many.txt".into()]);
    cfg.active_change_list = "This change".into();
    nemorous::config::save_to(&fx.config_file, &cfg).unwrap();
    let mut app = fx.start_headless_expect(1440.0, 900.0, "id:change-row-unstaged-many.txt");
    app.press("id:change-row-unstaged-many.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:diff-toolbar").unwrap();
    assert!(!app.screen().matches("id:diff-include-hunk").unwrap());
    app.right_click("new last").unwrap();
    assert!(app.menu().unwrap().iter().any(|item| {
        item.label == "纳入本次提交" && item.checked == Some(true)
    }));
    app.choose_menu_item("纳入本次提交").unwrap();
    app.type_text("id:git-commit-message", "Commit first hunk").unwrap();
    app.right_click("new last").unwrap();
    assert!(app.menu().unwrap().iter().any(|item| {
        item.label == "纳入本次提交" && item.checked == Some(false)
    }));
    app.close_menu().unwrap();
    app.press("id:git-commit-submit").unwrap();
    app.wait_for("提交成功 1 个仓库，失败 0 个").unwrap();

    let show = |spec: &str| nemorous::git::run_git_string(&fx.repo_dir, &["show", spec]).unwrap();
    assert_eq!(show("HEAD:many.txt"), format!("new first\n{context}old last\n"));
    assert_eq!(show(":notes.txt"), "untracked note line 1\n");
    assert_eq!(fs::read_to_string(path).unwrap(), new);
}

#[test]
fn ui_workspace_recent_switch_uses_new_repo_and_preserves_original_config_data() {
    let _guard = lock_env();
    let fx = Fixture::new("workspace-switch");
    let other = fx.root.join("another-workspace");
    fs::create_dir(&other).unwrap();
    git_run(&other, &["init", "-b", "main"]);
    fs::write(other.join("new-workspace.txt"), "fixture\n").unwrap();
    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.add_recent_workspace(other.to_str().unwrap());
    config.extra.insert("legacy_field".into(), serde_json::json!({"keep": true}));
    nemorous::config::save_to(&fx.config_file, &config).unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:top-workspace-button").unwrap();
    let label = nemorous::paths::display_path(&other.to_string_lossy());
    app.choose_menu_item(&format!("最近工作区 > {label} > 打开")).unwrap();
    app.wait_for("id:change-row-unstaged-new-workspace.txt").unwrap();
    assert!(!app.screen().matches("id:change-row-unstaged-src/app.go").unwrap());
    let cfg: serde_json::Value = serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert_eq!(cfg["legacy_field"]["keep"], true);
    assert_eq!(cfg["workspace_path"], other.canonicalize().unwrap().to_str().unwrap());
}

#[test]
fn ui_worktree_operations_target_the_selected_repository_and_preserve_dirty_files() {
    let _guard = lock_env();
    let fx = Fixture::new("worktree-menu");
    let second = fx.repo_dir.join("second-repo");
    fs::create_dir(&second).unwrap();
    git_run(&second, &["init", "-b", "main"]);
    fs::write(second.join("scope.txt"), "second repository\n").unwrap();
    git_run(&second, &["add", "scope.txt"]);
    git_run(&second, &["commit", "-m", "Second repository fixture"]);
    git_run(&second, &["branch", "review/topic"]);
    git_run(&fx.repo_dir, &["branch", "review/topic"]);
    let linked = fx.root.join("工作树 review path");
    let linked_path = linked.to_str().unwrap();

    let mut app = fx.start_headless(1440.0, 900.0);
    let open_worktrees = |app: &mut Headless| {
        app.press("id:top-git-advanced-btn").unwrap();
        app.choose_menu_item("second-repo > 工作树…").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while app.menu().is_none() {
            assert!(std::time::Instant::now() < deadline, "worktree menu did not open");
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    open_worktrees(&mut app);
    app.choose_menu_item("新建工作树…").unwrap();
    app.type_text("id:git-text-prompt-input", &format!("review/topic {linked_path}")).unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("创建工作树已完成").unwrap();
    assert_eq!(fs::read_to_string(linked.join("scope.txt")).unwrap(), "second repository\n");
    assert!(!linked.join("README.md").exists());

    git_run(&second, &["worktree", "lock", "--reason", "fixture", linked_path]);
    open_worktrees(&mut app);
    let items = app.menu().unwrap();
    assert!(items.iter().any(|item| item.label.contains("工作树 review path")
        && item.label.ends_with("已锁定 > 移除工作树…") && item.disabled));
    app.close_menu().unwrap();
    git_run(&second, &["worktree", "unlock", linked_path]);

    let work = linked.join("scope.txt");
    fs::write(&work, "uncommitted workspace data\n").unwrap();
    open_worktrees(&mut app);
    let remove = app.menu().unwrap().into_iter().find(|item| {
        item.label.contains("工作树 review path") && item.label.ends_with(" > 移除工作树…")
    }).unwrap().label;
    app.choose_menu_item(&remove).unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("text:/contains modified or untracked files/").unwrap();
    assert_eq!(fs::read_to_string(&work).unwrap(), "uncommitted workspace data\n");

    git_run(&linked, &["restore", "--", "scope.txt"]);
    open_worktrees(&mut app);
    app.choose_menu_item(&remove).unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("移除工作树已完成").unwrap();
    assert!(!linked.exists());
    git_run(&second, &["show-ref", "--verify", "refs/heads/review/topic"]);

    git_run(&second, &["worktree", "add", "--", linked_path, "review/topic"]);
    fs::write(linked.join("agent-worktree.txt"), "agent worktree change\n").unwrap();
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "README.md:1:1").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    app.press("# Sample").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "1:1").unwrap();
    app.submit("id:goto-line-input").unwrap();
    app.press_key("x").unwrap();
    app.wait_for("id:code-dirty-badge").unwrap();
    open_worktrees(&mut app);
    let open = app.menu().unwrap().into_iter().find(|item| {
        item.label.contains("工作树 review path") && item.label.ends_with(" > 打开工作树")
    }).unwrap().label;
    app.choose_menu_item(&open).unwrap();
    app.wait_for("请先保存当前工作区的未保存编辑").unwrap();
    app.press("x# Sample").unwrap();
    app.press_key("mod+z").unwrap();
    app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(3)).unwrap();

    open_worktrees(&mut app);
    app.choose_menu_item(&open).unwrap();
    app.wait_for("id:change-row-unstaged-agent-worktree.txt").unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert_eq!(saved["workspace_path"], linked.canonicalize().unwrap().to_str().unwrap());
}

#[cfg(target_os = "macos")]
#[test]
fn ui_switching_workspaces_dismisses_a_pending_repository_menu() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_env();
    let fx = Fixture::new("stale-repository-menu");
    let other = fx.root.join("another-workspace");
    fs::create_dir(&other).unwrap();
    git_run(&other, &["init", "-b", "main"]);
    fs::write(other.join("current-workspace.txt"), "current workspace\n").unwrap();
    let mut config = nemorous::config::load_from(&fx.config_file).unwrap();
    config.add_recent_workspace(other.to_str().unwrap());
    nemorous::config::save_to(&fx.config_file, &config).unwrap();

    let bin = fx.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let shim = bin.join("git");
    fs::write(&shim, r#"#!/bin/sh
case " $* " in
    *" worktree list "*)
        : > .git/menu-query-started
        tries=0
        while [ ! -e .git/resume-menu-query ] && [ "$tries" -lt 100 ]; do
            /bin/sleep 0.05
            tries=$((tries + 1))
        done
        /usr/bin/git "$@"
        result=$?
        : > .git/menu-query-finished
        exit "$result"
        ;;
esac
exec /usr/bin/git "$@"
"#).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
    struct RestorePath(Option<std::ffi::OsString>);
    impl Drop for RestorePath {
        fn drop(&mut self) {
            unsafe {
                if let Some(old) = &self.0 { std::env::set_var("PATH", old); }
                else { std::env::remove_var("PATH"); }
            }
        }
    }
    let original = RestorePath(std::env::var_os("PATH"));
    let inherited = original.0.as_deref().unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(inherited))).unwrap();
    unsafe { std::env::set_var("PATH", path); }

    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:top-git-advanced-btn").unwrap();
    app.choose_menu_item("工作树…").unwrap();
    let wait_for_file = |app: &mut Headless, name: &str| {
        let marker = fx.repo_dir.join(".git").join(name);
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !marker.exists() {
            assert!(std::time::Instant::now() < deadline, "missing Git query marker {name}");
            app.wait(Duration::from_millis(25)).unwrap();
        }
    };
    wait_for_file(&mut app, "menu-query-started");
    app.press("id:top-workspace-button").unwrap();
    let label = nemorous::paths::display_path(&other.to_string_lossy());
    app.choose_menu_item(&format!("最近工作区 > {label} > 打开")).unwrap();
    app.wait_for("id:change-row-unstaged-current-workspace.txt").unwrap();
    fs::write(fx.repo_dir.join(".git/resume-menu-query"), "").unwrap();
    wait_for_file(&mut app, "menu-query-finished");
    app.wait(Duration::from_millis(250)).unwrap();
    assert!(app.menu().is_none(), "the previous workspace's worktree menu reopened");
}

#[test]
fn ui_resolve_conflict_from_change_and_diff_opens_the_requested_file() {
    let _guard = lock_env();
    let fx = Fixture::new("resolve-selected-conflict");
    for path in ["a-first.txt", "z-target.txt"] {
        fs::write(fx.repo_dir.join(path), "baseline\n").unwrap();
    }
    git_run(&fx.repo_dir, &["add", "a-first.txt", "z-target.txt"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add two files to conflict"]);
    git_run(&fx.repo_dir, &["switch", "-c", "incoming"]);
    fs::write(fx.repo_dir.join("a-first.txt"), "incoming first\n").unwrap();
    fs::write(fx.repo_dir.join("z-target.txt"), "incoming target\n").unwrap();
    git_run(&fx.repo_dir, &["commit", "-am", "Change both from incoming"]);
    git_run(&fx.repo_dir, &["switch", "main"]);
    fs::write(fx.repo_dir.join("a-first.txt"), "current first\n").unwrap();
    fs::write(fx.repo_dir.join("z-target.txt"), "current target\n").unwrap();
    git_run(&fx.repo_dir, &["commit", "-am", "Change both from main"]);
    let merge = Command::new("git")
        .current_dir(&fx.repo_dir)
        .args(["merge", "--no-edit", "incoming"])
        .output().unwrap();
    assert_eq!(merge.status.code(), Some(1));

    let mut app = fx.start_headless(1440.0, 900.0);
    let row = "id:change-row-unstaged-z-target.txt";
    app.wait_for(row).unwrap();
    app.right_click(row).unwrap();
    app.choose_menu_item("解决冲突…").unwrap();
    app.wait_for("id:conflict-result >> text:current target").unwrap();
    app.press("id:conflict-close").unwrap();
    app.wait_for_gone("id:conflict-result", Duration::from_secs(2)).unwrap();

    app.press(row).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:diff-open-conflicts").unwrap();
    app.press("id:diff-open-conflicts").unwrap();
    app.wait_for("id:conflict-result >> text:current target").unwrap();
    app.press("id:conflict-take-theirs").unwrap();
    app.wait_for("id:conflict-result >> text:incoming target").unwrap();
    app.press("id:conflict-save").unwrap();
    app.wait_for("已标记为解决").unwrap();

    let unmerged = Command::new("git")
        .current_dir(&fx.repo_dir)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output().unwrap();
    assert!(unmerged.status.success());
    assert_eq!(unmerged.stdout, b"a-first.txt\n");
    assert_eq!(fs::read(fx.repo_dir.join("z-target.txt")).unwrap(), b"incoming target\n");
    assert!(!app.screen().matches("id:conflict-continue").unwrap());

    app.wait_for("id:conflict-result >> text:current first").unwrap();
    app.press("id:conflict-take-ours").unwrap();
    app.press("id:conflict-save").unwrap();
    app.wait_for("没有待处理的冲突").unwrap();
    app.wait_for("id:conflict-continue").unwrap();
    app.wait_for("继续合并").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hooks = fx.root.join("test-hooks");
        fs::create_dir(&hooks).unwrap();
        let hook = hooks.join("pre-commit");
        fs::write(&hook, "#!/bin/sh\nprintf '%s\\n' 'fixture blocks first attempt' >&2\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        git_run(&fx.repo_dir, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
        app.press("id:conflict-continue").unwrap();
        app.wait_for("id:conflict-finish-error").unwrap();
        assert!(fx.repo_dir.join(".git/MERGE_HEAD").is_file());
        app.wait_for("id:conflict-continue").unwrap();
        fs::remove_file(hook).unwrap();
    }

    app.press("id:conflict-continue").unwrap();
    app.wait_for("已完成 Git 操作").unwrap();
    app.wait_for_gone("id:conflict-close", Duration::from_secs(2)).unwrap();
    assert!(!fx.repo_dir.join(".git/MERGE_HEAD").exists());
    let parents = nemorous::git::run_git_string(&fx.repo_dir, &["show", "-s", "--format=%P", "HEAD"]).unwrap();
    assert_eq!(parents.split_whitespace().count(), 2);
    let git_text = |args: &[&str]| nemorous::git::run_git_string(&fx.repo_dir, args).unwrap();
    assert_eq!(git_text(&["show", "HEAD:a-first.txt"]).trim(), "current first");
    assert_eq!(git_text(&["show", "HEAD:z-target.txt"]).trim(), "incoming target");
}

#[test]
fn ui_rebase_continue_reloads_conflicts_from_the_next_commit() {
    let _guard = lock_env();
    let fx = Fixture::new("rebase-next-conflict");
    let first = fx.repo_dir.join("first.txt");
    let second = fx.repo_dir.join("second.txt");
    fs::write(&first, "base first\n").unwrap();
    fs::write(&second, "base second\n").unwrap();
    git_run(&fx.repo_dir, &["add", "first.txt", "second.txt", "src/app.go"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Base for rebase"]);
    git_run(&fx.repo_dir, &["switch", "-c", "topic"]);
    fs::write(&first, "topic first\n").unwrap();
    git_run(&fx.repo_dir, &["commit", "-am", "Topic first commit"]);
    fs::write(&second, "topic second\n").unwrap();
    git_run(&fx.repo_dir, &["commit", "-am", "Topic second commit"]);
    git_run(&fx.repo_dir, &["switch", "main"]);
    fs::write(&first, "main first\n").unwrap();
    fs::write(&second, "main second\n").unwrap();
    git_run(&fx.repo_dir, &["commit", "-am", "Main changes both files"]);
    git_run(&fx.repo_dir, &["switch", "topic"]);
    let rebase = Command::new("git").current_dir(&fx.repo_dir).args(["rebase", "main"]).output().unwrap();
    assert_eq!(rebase.status.code(), Some(1));

    let row = "id:change-row-unstaged-first.txt";
    let mut app = fx.start_headless_expect(1440.0, 900.0, row);
    app.right_click(row).unwrap();
    app.choose_menu_item("解决冲突…").unwrap();
    app.wait_for("id:conflict-result >> text:main first").unwrap();
    app.press("id:conflict-take-theirs").unwrap();
    app.press("id:conflict-save").unwrap();
    app.wait_for("id:conflict-continue").unwrap();
    app.wait_for("继续变基").unwrap();
    app.press("id:conflict-continue").unwrap();

    app.wait_for("id:conflict-result >> text:main second").unwrap();
    app.wait_for("✓ first.txt").unwrap();
    assert!(!app.screen().matches("id:conflict-finish-error").unwrap());
    app.press("id:conflict-take-theirs").unwrap();
    app.press("id:conflict-save").unwrap();
    app.wait_for("id:conflict-continue").unwrap();
    app.press("id:conflict-continue").unwrap();
    app.wait_for("已完成 Git 操作").unwrap();
    let git_text = |args: &[&str]| nemorous::git::run_git_string(&fx.repo_dir, args).unwrap();
    assert!(!fx.repo_dir.join(".git/rebase-merge").exists());
    assert!(!fx.repo_dir.join(".git/rebase-apply").exists());
    assert_eq!(git_text(&["symbolic-ref", "--short", "HEAD"]).trim(), "topic");
    assert_eq!(git_text(&["log", "--format=%s", "main..topic"]).trim(), "Topic second commit\nTopic first commit");
    assert_eq!(git_text(&["show", "topic:first.txt"]).trim(), "topic first");
    assert_eq!(git_text(&["show", "topic:second.txt"]).trim(), "topic second");
}

#[test]
fn ui_isolated_merge_reopens_and_retries_promotion_after_dirty_worktree() {
    let _guard = lock_env();
    let fx = Fixture::new("isolated-merge-retry");
    let file = fx.repo_dir.join("merge-target.txt");
    fs::write(&file, "common base\n").unwrap();
    git_run(&fx.repo_dir, &["add", "merge-target.txt", "src/app.go"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Base for isolated merge"]);
    git_run(&fx.repo_dir, &["switch", "-c", "incoming"]);
    fs::write(&file, "source lane\n").unwrap();
    git_run(&fx.repo_dir, &["commit", "-am", "Change incoming file"]);
    git_run(&fx.repo_dir, &["switch", "main"]);
    fs::write(&file, "target lane\n").unwrap();
    git_run(&fx.repo_dir, &["commit", "-am", "Change target file"]);
    let git_text = |args: &[&str]| nemorous::git::run_git_string(&fx.repo_dir, args).unwrap();
    let target_head = git_text(&["rev-parse", "main"]);
    let source_head = git_text(&["rev-parse", "incoming"]);

    let mut app = fx.start_headless_expect(1440.0, 900.0, "notes.txt");
    let open_plan = |app: &mut Headless| {
        app.press_key("mod+b").unwrap();
        app.wait_for("text:incoming").unwrap();
        app.right_click("text:incoming").unwrap();
        app.choose_menu_item("合并 incoming 到 main…").unwrap();
        app.wait_for("id:git-merge-planner").unwrap();
    };
    open_plan(&mut app);
    app.press("id:merge-run").unwrap();
    app.wait_for("id:conflict-result >> text:target lane").unwrap();
    assert_eq!(git_text(&["rev-parse", "main"]), target_head);
    assert_eq!(fs::read(&file).unwrap(), b"target lane\n");

    app.press("id:conflict-close").unwrap();
    app.wait_for_gone("id:git-conflict-workbench", Duration::from_secs(2)).unwrap();
    open_plan(&mut app);
    app.press("继续 incoming → main · 1 个冲突").unwrap();
    app.wait_for("id:conflict-result >> text:target lane").unwrap();
    app.press("id:conflict-take-theirs").unwrap();
    app.press("id:conflict-save").unwrap();
    app.wait_for("完成本地合并").unwrap();
    app.press("id:conflict-continue").unwrap();
    app.wait_for("id:conflict-finish-error").unwrap();
    app.wait_for("完成本地合并").unwrap();
    assert_eq!(git_text(&["rev-parse", "main"]), target_head);
    assert_eq!(fs::read(&file).unwrap(), b"target lane\n");
    assert_eq!(fs::read(fx.repo_dir.join("notes.txt")).unwrap(), b"untracked note line 1\n");

    fs::remove_file(fx.repo_dir.join("notes.txt")).unwrap();
    app.press("id:conflict-continue").unwrap();
    app.wait_for("合并校验通过").unwrap();
    let parents = git_text(&["show", "--format=%P", "-s", "main"]);
    assert_eq!(parents.split_whitespace().collect::<Vec<_>>(), vec![target_head.trim(), source_head.trim()]);
    assert_eq!(git_text(&["show", "main:merge-target.txt"]).trim(), "source lane");
    assert_eq!(fs::read(&file).unwrap(), b"source lane\n");
    assert_eq!(git_text(&["worktree", "list", "--porcelain"]).lines().filter(|line| line.starts_with("worktree ")).count(), 1);
}

#[test]
fn ui_same_named_repositories_keep_files_and_branch_actions_separate() {
    let _guard = lock_env();
    let fx = Fixture::new("same-repo-name");
    #[cfg(unix)]
    let fx = {
        let mut fx = fx;
        let workspace_alias = fx.root.join("workspace-alias");
        std::os::unix::fs::symlink(fx.repo_dir.canonicalize().unwrap(), &workspace_alias).unwrap();
        fx.repo_dir = workspace_alias;
        fx
    };
    let first = fx.repo_dir.join("projects/nested");
    let second = fx.repo_dir.join("shared");
    for (repo, content) in [(&first, "first edit\n"), (&second, "second edit\n")] {
        fs::create_dir_all(repo).unwrap();
        git_run(repo, &["init", "-b", "main"]);
        let file = repo.join("review.txt");
        fs::write(&file, "base\n").unwrap();
        git_run(repo, &["add", "review.txt"]);
        git_run(repo, &["commit", "-m", "Begin review"]);
        fs::write(file, content).unwrap();
    }
    fs::write(fx.repo_dir.join("mani.yaml"), "projects:\n  shared:\n    path: projects/nested\n").unwrap();
    fs::write(&fx.config_file, serde_json::to_vec(&serde_json::json!({
        "workspace_path": fx.repo_dir,
        "include_root_repo": false,
        "change_diff_preview": false,
    })).unwrap()).unwrap();

    let mut app = fx.start_headless_expect(1440.0, 900.0, "shared · shared");
    app.wait_for("shared · projects/nested").unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    let bindings = &saved["repository_id_bindings"][fx.repo_dir.to_str().unwrap()];
    assert_eq!(bindings[first.canonicalize().unwrap().to_str().unwrap()], "shared");
    let second_id = bindings[second.canonicalize().unwrap().to_str().unwrap()].as_str().unwrap();
    assert_ne!(second_id, "shared");

    let second_change = "id:change-row-unstaged-review.txt >> nth=1";
    app.press(second_change).unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-new-editor >> text:second edit").unwrap();
    app.right_click(second_change).unwrap();
    app.choose_menu_item("暂存").unwrap();
    app.wait_for("id:change-row-staged-review.txt").unwrap();
    let git_text = |repo: &Path, args: &[&str]| nemorous::git::run_git_string(repo, args).unwrap();
    assert!(git_text(&first, &["diff", "--cached", "--name-only"]).trim().is_empty());
    assert_eq!(git_text(&second, &["show", ":review.txt"]), "second edit\n");

    let open_file = |app: &mut Headless, repo: &Path, content: &str| {
        app.press_key("mod+shift+o").unwrap();
        let input = format!("{}:1:1", repo.join("review.txt").display());
        app.type_text("id:review-picker-input", &input).unwrap();
        app.submit("id:review-picker-input").unwrap();
        app.wait_for(&format!("id:code-editor-view >> text:{content}")).unwrap();
    };
    open_file(&mut app, &first, "first edit");
    open_file(&mut app, &second, "second edit");
    app.press_key("q").unwrap();
    app.wait_for("未保存").unwrap();
    app.press_key("mod+s").unwrap();
    app.wait_for_gone("id:code-dirty-badge", Duration::from_secs(4)).unwrap();
    assert_eq!(fs::read(first.join("review.txt")).unwrap(), b"first edit\n");
    assert_eq!(fs::read(second.join("review.txt")).unwrap(), b"qsecond edit\n");

    for repo in [&first, &second] {
        git_run(repo, &["commit", "-am", "Finish review"]);
    }
    app.press_key("mod+b").unwrap();
    let new_branch = format!("id:branch-new-{second_id}");
    app.wait_for(&new_branch).unwrap();
    app.press(&new_branch).unwrap();
    app.type_text("id:git-text-prompt-input", "review-only-second").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("已切换 1 个仓库").unwrap();
    assert_eq!(git_text(&first, &["branch", "--show-current"]).trim(), "main");
    assert_eq!(git_text(&second, &["branch", "--show-current"]).trim(), "review-only-second");

    app.press("id:top-workspace-button").unwrap();
    app.choose_menu_item("隐藏仓库 > shared · projects/nested").unwrap();
    app.press_key("mod+1").unwrap();
    let second_group = format!("id:project-repo-{second_id}");
    app.wait_for(&second_group).unwrap();
    app.wait_for_gone("id:project-repo-shared", Duration::from_secs(3)).unwrap();
    drop(app);

    let parked = fx.root.join("parked");
    fs::rename(&first, &parked).unwrap();
    let mut app = fx.start_app(1440.0, 900.0);
    app.wait_for("id:changes-header-title-row").unwrap();
    app.press_key("mod+1").unwrap();
    app.wait_for(&second_group).unwrap();
    app.press("id:top-workspace-button").unwrap();
    app.choose_menu_item("恢复隐藏仓库 > projects/nested").unwrap();
    fs::rename(&parked, &first).unwrap();
    app.press_key("mod+alt+y").unwrap();
    app.wait_for("id:project-repo-shared").unwrap();
    app.wait_for(&second_group).unwrap();
}

#[test]
fn ui_git_sync_reports_missing_remote_without_rolling_back_changes() {
    let _guard = lock_env();
    let fx = Fixture::new("remote-sync-report");
    let worktree = fs::read(fx.repo_dir.join("src/app.go")).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:top-git-sync-btn").unwrap();
    app.choose_menu_item("获取全部仓库").unwrap();
    app.wait_for("sample-repo · 跳过").unwrap();
    app.press("id:git-sync-close").unwrap();
    assert_eq!(fs::read(fx.repo_dir.join("src/app.go")).unwrap(), worktree);
}

#[test]
fn ui_rollback_code_block_is_undoable_and_file_rollback_requires_confirmation() {
    let _guard = lock_env();
    let _ = env_logger::builder().is_test(true).try_init();
    let fx = Fixture::new("rollback");
    let path = fx.repo_dir.join("src/app.go");
    let bytes = fs::read(&path).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/app.go:4:2").unwrap();
    app.submit("id:review-picker-input").unwrap();
    app.wait_for(r#"return "new-greeting""#).unwrap();
    app.type_text("id:code-editor-view", "").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "4:2").unwrap();
    app.submit("id:goto-line-input").unwrap();
    app.press_key("mod+alt+z").unwrap();
    app.wait_for(r#"return "old-greeting""#).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes, "rolling back a code block edits only the buffer");
    app.press_key("mod+z").unwrap();
    app.wait_for(r#"return "new-greeting""#).unwrap();
    app.right_click("id:change-row-unstaged-src/app.go").unwrap();
    app.choose_menu_item("回滚文件…").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("已回滚文件").unwrap();
    assert!(String::from_utf8_lossy(&fs::read(&path).unwrap()).contains("old-greeting"));
}

fn protocol_fixture(fixture: &Fixture, name: &str) -> (PathBuf, PathBuf) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workbench_mock_protocol.py");
    let proof = fixture.root.join(format!("{name}.log"));
    (script, proof)
}

#[test]
fn ui_debug_adapter_stops_and_displays_variables() {
    let _guard = lock_env();
    let fx = Fixture::new("debug-adapter");
    let (script, proof) = protocol_fixture(&fx, "dap");
    fs::write(&fx.config_file, serde_json::to_vec(&serde_json::json!({
        "workspace_path": fx.repo_dir,
        "theme": "light",
        "breakpoints": {"sample-repo::src/app.go": [2]},
        "run_configs": [{
            "id":"debug", "name":"Fixture debug", "kind":"debug", "adapter":"python3",
            "adapter_args":format!("\"{}\" dap \"{}\"",script.display(),proof.display()),
            "network":"stdio", "program":"src/app.go"
        }]
    })).unwrap()).unwrap();
    let mut app = fx.start_headless(1440.0,900.0);
    app.press("id:rail-run-btn").unwrap();
    app.press("id:run-start").unwrap();
    app.wait_for("已暂停 · breakpoint").unwrap();
    app.wait_for("fixture main:2").unwrap();
    app.wait_for("ticket = 42").unwrap();
    app.press("id:debug-continue").unwrap();
    app.wait_for("调试中").unwrap();
    app.press("id:run-stop").unwrap();
    app.wait_for_gone("id:debug-state",Duration::from_secs(2)).unwrap();
    let requests = fs::read_to_string(proof).unwrap();
    assert!(requests.contains("initialize "));
    assert!(requests.contains("configurationDone "));
    assert!(requests.contains("setBreakpoints "));
    assert!(requests.contains("continue "));
}

#[test]
fn ui_debug_adapter_reports_stderr_and_can_restart() {
    let _guard = lock_env();
    let fx = Fixture::new("debug-adapter-retry");
    let (script, proof) = protocol_fixture(&fx, "dap-retry");
    let config = serde_json::json!({
        "workspace_path": fx.repo_dir,
        "breakpoints": {"sample-repo::src/app.go": [2]},
        "run_configs": [{
            "id": "debug", "name": "Fixture debug", "kind": "debug", "adapter": "python3",
            "adapter_args": format!("\"{}\" dap-retry \"{}\"", script.display(), proof.display())
        }]
    });
    fs::write(&fx.config_file, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:rail-run-btn").unwrap();
    app.press("id:run-start").unwrap();
    app.wait_for("调试适配器已退出 · 退出码 7").unwrap();
    app.wait_for("synthetic adapter startup failure").unwrap();

    app.press("id:run-start").unwrap();
    app.wait_for("已暂停 · breakpoint").unwrap();
    app.wait_for("ticket = 42").unwrap();
    assert!(!app.screen().matches("synthetic adapter startup failure").unwrap());
    assert_eq!(fs::read_to_string(proof).unwrap().matches("\nlaunch ").count(), 2);
}

#[test]
#[ignore = "requires debugpy on the selected Python interpreter's module search path"]
fn ui_debug_adapter_runs_real_python_breakpoint() {
    let _guard = lock_env();
    let fx = Fixture::new("debug-python");
    let source = fx.repo_dir.join("debug_review.py");
    fs::write(
        &source,
        "def main():\n    ticket = 42\n    ticket += 1\n    print('debug ticket', ticket)\n\nmain()\n",
    ).unwrap();
    git_run(&fx.repo_dir, &["add", "debug_review.py"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add executable debug fixture"]);
    let python = std::env::var("NEMOROUS_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let launch = serde_json::json!({
        "console": "internalConsole", "redirectOutput": true, "justMyCode": true
    });
    let config = serde_json::json!({
        "workspace_path": fx.repo_dir,
        "breakpoints": {"sample-repo::debug_review.py": [3]},
        "run_configs": [{
            "id": "debug", "name": "Python debug test", "kind": "debug", "adapter": python,
            "adapter_args": "-B -Xfrozen_modules=off -m debugpy.adapter", "program": source,
            "launch": launch.to_string()
        }]
    });
    fs::write(&fx.config_file, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press("id:rail-run-btn").unwrap();
    app.press("id:run-start").unwrap();
    app.wait_for("已暂停 · breakpoint").unwrap();
    app.wait_for("text:main:3").unwrap();
    let frame = app.screen().press_target("text:main:3").unwrap().unwrap();
    app.wait_for("ticket = 42").unwrap();
    app.wait(Duration::from_millis(120)).unwrap();
    assert_eq!(app.screen().press_target("text:main:3").unwrap(), Some(frame));
    assert!(!app.screen().matches("ptvsd").unwrap());
    app.press("text:main:3").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
    app.press("id:debug-next").unwrap();
    app.wait_for("text:main:4").unwrap();
    app.wait_for("ticket = 43").unwrap();
    app.press("id:debug-continue").unwrap();
    app.wait_for("debug ticket 43").unwrap();
    app.wait_for("调试已结束").unwrap();
    assert!(!app.screen().matches("ticket = 43").unwrap());
}

#[test]
fn ui_lsp_requests_hover_and_stages_formatting_for_review() {
    let _guard = lock_env();
    let fx = Fixture::new("language-server");
    let (script, proof) = protocol_fixture(&fx,"lsp");
    fs::write(&fx.config_file, serde_json::to_vec(&serde_json::json!({
        "workspace_path":fx.repo_dir, "theme":"light", "lsp_enabled":true,
        "lsp_servers":{"go":format!("python3 \"{}\" lsp \"{}\"",script.display(),proof.display())}
    })).unwrap()).unwrap();
    let mut app = fx.start_headless(1440.0,900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input","src/app.go").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("id:code-editor-view").unwrap();
    for _ in 0..30 {
        if fs::read_to_string(&proof).is_ok_and(|text| text.contains("textDocument/didOpen")) { break; }
        app.idle(Duration::from_millis(60)).unwrap();
    }
    assert!(fs::read_to_string(&proof).unwrap().contains("textDocument/didOpen"));
    app.right_click(r#"return "new-greeting""#).unwrap();
    app.choose_menu_item("语言服务 > 悬停信息").unwrap();
    app.wait_for("synthetic hover").unwrap();
    app.press_key("escape").unwrap();
    app.press_key("alt+enter").unwrap();
    for _ in 0..25 {
        if app.menu().is_some_and(|items| items.iter().any(|item| item.label == "Synthetic source fix")) {
            break;
        }
        app.idle(Duration::from_millis(80)).unwrap();
    }
    app.choose_menu_item("Synthetic source fix").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    app.press_key("escape").unwrap();
    app.right_click(r#"return "new-greeting""#).unwrap();
    app.choose_menu_item("语言服务 > 格式化文件").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("已应用到编辑器，可审阅后保存").unwrap();
    assert!(!fs::read_to_string(fx.repo_dir.join("src/app.go")).unwrap().starts_with("// verified LSP"));
    app.press_key("mod+s").unwrap();
    let path = fx.repo_dir.join("src/app.go");
    for _ in 0..30 {
        if fs::read_to_string(&path).unwrap().starts_with("// verified LSP") { break; }
        app.idle(Duration::from_millis(60)).unwrap();
    }
    assert!(fs::read_to_string(path).unwrap().starts_with("// verified LSP"));
}

#[test]
#[ignore = "requires clangd on PATH or NEMOROUS_TEST_CLANGD"]
fn ui_lsp_real_clangd_uses_clicked_symbol_and_reveals_diagnostics() {
    let _guard = lock_env();
    let fx = Fixture::new("clangd");
    let source = "#include \"helper.h\"\nint main(void) {\n    return   shifted_value(41);\n}\nint broken(void) {\n    return undefined_ticket;\n}\n";
    let path = fx.repo_dir.join("src/main.c");
    fs::write(&path, source).unwrap();
    fs::write(
        fx.repo_dir.join("src/helper.h"),
        "// Adds one to the supplied amount.\n/* 🦀 */ static inline int shifted_value(int amount) { return amount + 1; }\n",
    ).unwrap();
    git_run(&fx.repo_dir, &["add", "src/main.c", "src/helper.h"]);
    git_run(&fx.repo_dir, &["commit", "-m", "Add C navigation fixture"]);
    let clangd = std::env::var("NEMOROUS_TEST_CLANGD").unwrap_or_else(|_| "clangd".into());
    let config = serde_json::json!({
        "workspace_path": fx.repo_dir,
        "lsp_enabled": true,
        "lsp_servers": {"c": format!("\"{clangd}\" --background-index=false --clang-tidy=false")}
    });
    fs::write(&fx.config_file, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut app = fx.start_headless(1440.0, 900.0);
    app.press_key("mod+shift+o").unwrap();
    app.type_text("id:review-picker-input", "src/main.c:1:1").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("return   shifted_value(41);").unwrap();

    let mut diagnostic = None;
    for _ in 0..20 {
        app.right_click("return undefined_ticket;").unwrap();
        app.choose_menu_item("语言服务 > 问题").unwrap();
        diagnostic = app.menu().into_iter().flatten().find(|entry| {
            entry.label.starts_with("src/main.c:6") && entry.label.contains("undefined_ticket")
        });
        if diagnostic.is_some() {
            break;
        }
        app.idle(Duration::from_millis(140)).unwrap();
    }
    app.choose_menu_item(&diagnostic.expect("clangd diagnostic for undefined_ticket").label).unwrap();
    app.wait_for("6:12").unwrap();

    app.right_click("return   shifted_value(41);").unwrap();
    app.choose_menu_item("语言服务 > 悬停信息").unwrap();
    app.wait_for("Adds one to the supplied amount.").unwrap();
    app.press_key("escape").unwrap();
    app.right_click("return   shifted_value(41);").unwrap();
    app.choose_menu_item("语言服务 > 转到定义").unwrap();
    app.wait_for("2:28").unwrap();
    app.wait_for("src/helper.h").unwrap();

    app.press_key("mod+[").unwrap();
    app.wait_for("return   shifted_value(41);").unwrap();
    app.press_key("mod+l").unwrap();
    app.type_text("id:goto-line-input", "3:15").unwrap();
    app.press("id:goto-line-submit").unwrap();
    app.wait_for("3:15").unwrap();
    app.press_key("alt+f7").unwrap();
    for _ in 0..25 {
        if app.menu().is_some_and(|items| items.iter().any(|item| item.label == "src/main.c:3")) {
            break;
        }
        app.idle(Duration::from_millis(80)).unwrap();
    }
    app.choose_menu_item("src/main.c:3").unwrap();
    app.right_click("return   shifted_value(41);").unwrap();
    app.choose_menu_item("语言服务 > 格式化文件").unwrap();
    app.wait_for("id:git-text-prompt").unwrap();
    app.press("id:git-text-prompt-submit").unwrap();
    app.wait_for("int main(void) { return shifted_value(41); }").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    app.press_key("mod+z").unwrap();
    app.wait_for("return   shifted_value(41);").unwrap();
}

#[test]
fn ui_mani_runs_configured_workspace_command() {
    let _guard = lock_env();
    let fx = Fixture::new("mani-workspace");
    fs::write(fx.repo_dir.join("mani.yaml"),
        "tasks:\n  verify:\n    desc: Fixture proof\n    target: {cwd: true}\n    env: {TOKEN: 'sample value'}\n    cmd: printf '%s' \"$TOKEN\" > mani-proof\n").unwrap();
    let mut app = fx.start_headless(1440.0,900.0);
    app.press_key("mod+4").unwrap();
    app.press("id:mani-select").unwrap();
    app.choose_menu_item("verify · Fixture proof").unwrap();
    app.wait_for("workspace (cwd) · 完成").unwrap();
    assert_eq!(fs::read_to_string(fx.repo_dir.join("mani-proof")).unwrap(),"sample value");
}

#[cfg(unix)]
#[test]
fn ui_terminal_wheel_reaches_shell_output_and_returns_to_prompt() {
    let _guard = lock_env();
    let fx = Fixture::new("terminal-wheel");
    let lines: String = (0..100).map(|i| format!("fixture_output_{i:03}\n")).collect();
    fs::write(fx.repo_dir.join("terminal-rows.txt"), lines).unwrap();
    // Headless forwards raw keystrokes to focusable views through its Linux input path.
    let mut app = fx.start_app_on(1440.0, 900.0, Os::Linux);
    app.wait_for("id:changes-header-title-row").unwrap();
    app.press("id:rail-terminal-btn").unwrap();
    app.wait_for("Shell 已启动").unwrap();
    app.press("id:terminal-screen").unwrap();
    app.set_clipboard(Some("cat terminal-rows.txt".into()));
    app.press_key("ctrl+shift+v").unwrap();
    app.wait_for(r"id:terminal-screen >> text:/cat terminal-rows\.txt\s*$/")
        .unwrap();
    app.press_key("enter").unwrap();
    app.wait_for("fixture_output_099").unwrap();
    assert!(!app.screen().matches("fixture_output_000").unwrap());

    app.wheel("id:terminal-screen", point(px(0.0), px(-1800.0))).unwrap();
    app.wait_for("fixture_output_000").unwrap();
    assert!(!app.screen().matches("fixture_output_099").unwrap());

    app.wheel("id:terminal-screen", point(px(0.0), px(1800.0))).unwrap();
    app.wait_for("fixture_output_099").unwrap();
    app.press_keys(&["e", "x", "i", "t", "enter"]).unwrap();
    app.wait_for("Shell 已退出 · 0").unwrap();
}

#[cfg(unix)]
#[test]
fn ui_github_uses_git_history_repository_for_lists_comments_and_drafts() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = lock_env();
    let fx = Fixture::new("mock-gh");
    git_run(&fx.repo_dir, &["remote", "add", "origin", "https://github.com/fixture/workspace"]);
    let review_repo = fx.repo_dir.join("review-app");
    fs::create_dir(&review_repo).unwrap();
    git_run(&review_repo, &["init", "-b", "main"]);
    fs::write(review_repo.join("review.txt"), "review\n").unwrap();
    git_run(&review_repo, &["add", "review.txt"]);
    git_run(&review_repo, &["commit", "-m", "Review application changes"]);
    git_run(&review_repo, &["remote", "add", "origin", "https://github.com/fixture/nemorous"]);
    let bin = fx.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, r#"#!/bin/sh
for repository in "$@"; do :; done
printf '%s\n' "$@" > "gh-$2-argv"
case "$2:$repository" in
list:fixture/workspace) printf '%s' '[{"number":5,"title":"Root repository PR"}]' ;;
list:fixture/nemorous)
    if [ -f .git/hold-gh-list ]; then
        : > .git/gh-list-started
        while [ -f .git/hold-gh-list ]; do sleep 0.02; done
    fi
    if [ -f gh-create-argv ]; then
        printf '%s' '[{"number":8,"title":"Draft fixture"},{"number":7,"title":"Fixture PR"}]'
    else
        printf '%s' '[{"number":7,"title":"Fixture PR"}]'
    fi ;;
view:fixture/nemorous)
    if [ -f .git/fail-gh-view ]; then exit 7; fi
    printf '%s' '{"number":7,"title":"Fixture PR","url":"https://github.com/fixture/nemorous/pull/7","body":"Fixture review body'
    line=1
    while [ "$line" -le 80 ]; do
        printf '\\nRelease note %s' "$line"
        line=$((line + 1))
    done
    printf '%s' '","statusCheckRollup":[{"name":"unit","status":"COMPLETED","conclusion":"SUCCESS"}],"comments":[{"author":{"login":"qa"},"body":"Fixture comment"}]}' ;;
comment:fixture/nemorous)
    printf '%s\n' 'accepted' >> .git/gh-comment-receipts
    while [ -f .git/hold-gh-comment ]; do sleep 0.02; done
    printf '%s' 'https://github.com/fixture/nemorous/pull/7#issuecomment-1' ;;
create:fixture/nemorous)
    if [ -f .git/fail-gh-create ]; then exit 8; fi
    printf '%s' 'https://github.com/fixture/nemorous/pull/8' ;;
*) exit 1 ;;
esac
"#).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
    let _path = TestPath::prepend(&bin);
    let mut app = fx.start_headless(1440.0,900.0);
    app.press("id:rail-git-btn").unwrap();
    let open_github = |app: &mut Headless| {
        app.press("id:git-log-actions").unwrap();
        app.choose_menu_item("GitHub PR / CI").unwrap();
    };
    app.wait_for("text:Review application changes").unwrap();
    app.press("text:Review application changes").unwrap();
    app.wait_for("id:git-commit-file-review.txt").unwrap();
    open_github(&mut app);
    app.wait_for("id:gh-pr-7").unwrap();
    assert!(!fx.repo_dir.join("gh-list-argv").exists());

    app.press("id:gh-close").unwrap();
    app.press("text:refactor: rename file to v2").unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.press("id:git-repo-filter").unwrap();
    app.choose_menu_item("review-app").unwrap();
    app.wait_for_gone("text:refactor: rename file to v2", Duration::from_secs(3)).unwrap();
    app.wait_for("id:git-commit-file-review.txt").unwrap();
    open_github(&mut app);
    app.wait_for("id:gh-pr-7").unwrap();
    app.press("id:gh-pr-7").unwrap();
    app.wait_for("unit · SUCCESS").unwrap();
    assert!(!app.screen().matches("Fixture comment").unwrap());
    app.wheel("id:gh-pr-detail", point(px(0.0), px(2600.0))).unwrap();
    app.wait_for("Fixture comment").unwrap();

    app.press("id:gh-comment").unwrap();
    app.type_text("id:gh-pr-body", "Only comment on the reviewed repository").unwrap();
    git_run(&review_repo, &["remote", "set-url", "origin", "https://github.com/fixture/replaced"]);
    app.press("id:gh-submit").unwrap();
    app.wait_for("origin 已更改，请重新打开此仓库").unwrap();
    assert!(!review_repo.join("gh-comment-argv").exists());
    git_run(&review_repo, &["remote", "set-url", "origin", "https://github.com/fixture/nemorous"]);
    let control = review_repo.join(".git");
    fs::write(control.join("hold-gh-comment"), "").unwrap();
    app.press("id:gh-submit").unwrap();
    let receipt = control.join("gh-comment-receipts");
    app.wait_until("the comment was accepted by the fixture", Duration::from_secs(4), |_| receipt.is_file()).unwrap();
    let _ = app.press("id:gh-cancel");
    app.press_key("escape").unwrap();
    assert!(app.screen().matches("id:github-panel").unwrap(), "closing lost an accepted comment's pending result");
    app.wait_for("正在提交…").unwrap();
    for id in ["gh-submit", "gh-close-form", "gh-select-repo", "gh-filter", "gh-refresh", "gh-cancel", "gh-close", "gh-new", "gh-pr-7"] {
        assert!(app.screen().press_target(&format!("id:{id}")).unwrap().is_none(), "{id} could replace a pending submission");
    }
    fs::write(control.join("fail-gh-view"), "").unwrap();
    fs::remove_file(control.join("hold-gh-comment")).unwrap();
    app.wait_for_gone("id:gh-submit", Duration::from_secs(2)).unwrap();
    app.wait_for("评论已发送 · GitHub CLI 请求失败（退出码 Some(7)）").unwrap();
    assert_eq!(fs::read_to_string(&receipt).unwrap(), "accepted\n");
    fs::remove_file(control.join("fail-gh-view")).unwrap();
    app.press("id:gh-pr-7").unwrap();
    app.wait_for("unit · SUCCESS").unwrap();
    let comment = fs::read_to_string(review_repo.join("gh-comment-argv")).unwrap();
    assert!(comment.contains("comment\n7\n--body\nOnly comment on the reviewed repository\n"), "{comment:?}");
    assert!(comment.contains("--repo\nfixture/nemorous\n"));

    fs::write(control.join("hold-gh-list"), "").unwrap();
    app.press("id:gh-refresh").unwrap();
    app.wait_until("the listing reached the fixture", Duration::from_secs(4), |_| control.join("gh-list-started").is_file()).unwrap();
    app.press("id:gh-cancel").unwrap();
    app.wait_for("评论已发送").unwrap();
    fs::remove_file(control.join("hold-gh-list")).unwrap();

    app.press("id:gh-new").unwrap();
    app.type_text("id:gh-pr-title","Draft fixture").unwrap();
    app.press("id:gh-pr-draft").unwrap();
    app.press("id:gh-submit").unwrap();
    app.wait_for_gone("id:gh-submit",Duration::from_secs(2)).unwrap();
    app.wait_for("text:https://github.com/fixture/nemorous/pull/8").unwrap();
    app.wait_for("id:gh-pr-8").unwrap();
    let command = fs::read_to_string(review_repo.join("gh-create-argv")).unwrap();
    assert!(command.contains("create\n--title\nDraft fixture\n"), "{command:?}");
    assert!(command.contains("--draft\n--repo\nfixture/nemorous\n"));

    fs::write(control.join("fail-gh-create"), "").unwrap();
    app.press("id:gh-new").unwrap();
    app.type_text("id:gh-pr-title", "Keep draft on uncertain response").unwrap();
    app.press("id:gh-submit").unwrap();
    app.wait_for("提交结果未确认，请先在 GitHub 核对后再试（gh 退出码 Some(8)）").unwrap();
    assert_eq!(app.screen().input_text("id:gh-pr-title").unwrap().as_deref(), Some("Keep draft on uncertain response"));
    app.press("id:gh-close-form").unwrap();

    app.press("id:gh-select-repo").unwrap();
    app.choose_menu_item("sample-repo").unwrap();
    app.wait_for("id:gh-pr-5").unwrap();
    assert!(!app.screen().matches("id:gh-pr-8").unwrap());
}

#[test]
fn ui_run_creates_edits_and_removes_legacy_compatible_configuration() {
    let _guard = lock_env();
    let fx = Fixture::new("run-config");
    let mut app = fx.start_headless(1440.0,900.0);
    app.press("id:rail-run-btn").unwrap();
    app.press("id:run-new-config").unwrap();
    app.type_text("id:run-field-name","Fixture Test").unwrap();
    app.press("id:run-kind-test").unwrap();
    app.type_text("id:run-field-command","/bin/sh").unwrap();
    app.type_text("id:run-field-args","-c 'echo configured-run'").unwrap();
    app.press("id:run-config-save").unwrap();
    app.wait_for_gone("id:run-config-modal",Duration::from_secs(2)).unwrap();
    app.press("id:run-start").unwrap();
    app.wait_for("configured-run").unwrap();
    app.wait_for("退出码 0").unwrap();
    app.press("id:run-edit-config").unwrap();
    app.press("id:run-config-delete").unwrap();
    app.wait_for_gone("id:run-config-modal",Duration::from_secs(2)).unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert_eq!(stored["run_configs"],serde_json::json!([]));
}

#[test]
fn ui_legacy_setting_choices_persist_without_losing_unknown_values() {
    let _guard = lock_env();
    let fx = Fixture::new("settings-legacy");
    fs::write(&fx.config_file,serde_json::to_vec(&serde_json::json!({
        "workspace_path":fx.repo_dir,
        "theme":"light",
        "app_icon":"aero",
        "zoom":1.0,
        "auto_update":false,
        "include_root_repo":true,
        "diff_ignore_mode":2,
        "diff_side_by_side":false,
        "mani_enabled":true,
        "future_setting":{"kept":true}
    })).unwrap()).unwrap();
    let mut app = fx.start_headless(1440.0,900.0);
    app.press_key("mod+,").unwrap();
    app.wait_for("id:settings-modal").unwrap();
    app.press("id:settings-theme").unwrap();
    app.choose_menu_item("Darcula").unwrap();
    app.press("id:settings-icon").unwrap();
    app.choose_menu_item("Prism").unwrap();
    app.press("id:settings-general").unwrap();
    app.press("id:mani_enabled").unwrap();
    app.press("id:settings-close").unwrap();
    app.wait_for_gone("id:rail-mani-btn",Duration::from_secs(3)).unwrap();
    let disk: serde_json::Value = serde_json::from_slice(&fs::read(&fx.config_file).unwrap()).unwrap();
    assert_eq!(disk["theme"],"darcula");
    assert_eq!(disk["app_icon"],"prism");
    assert_eq!(disk["auto_update"],false);
    assert_eq!(disk["diff_ignore_mode"],2);
    assert_eq!(disk["diff_side_by_side"],false);
    assert_eq!(disk["mani_enabled"],false);
    assert_eq!(disk["future_setting"]["kept"],true);
}
