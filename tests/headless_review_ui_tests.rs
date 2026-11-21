use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use nsh_core::geometry::{px, size};
use nsh_core::platform::{Idiom, Os};
use nsh_engine::{EngineConfig, Environment, Root};
use nsh_testing::Headless;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
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
        unsafe {
            std::env::set_var("NEMOROUS_CONFIG", &self.config_file);
            std::env::set_var("NEMOROUS_WORKSPACE", &self.repo_dir);
        }
        let mut config = EngineConfig::new(Root::of::<nemorous::App>());
        let mut env = Environment::default();
        env.os = Os::MacOs;
        env.idiom = Idiom::Desktop;
        env.apple_keyboard = true;
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
        app.wait_for("本地变更").expect("wait for commit header");
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
    app.wait_for("返回差异").unwrap();

    // 按 Cmd+[ 返回工作区 Diff
    app.press_key("mod+[").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // 按 Cmd+] 前进回源码页
    app.press_key("mod+]").unwrap();
    app.wait_for("id:code-editor-pane").unwrap();
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
    app.press("id:diff-whitespace-btn").unwrap();
    app.choose_menu_item("忽略空白和空行").unwrap();
    app.wait_for("忽略空白和空行").unwrap();
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
    app.press("id:git-commit-file-src/rename_new.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:commit-diff-pane").unwrap();
    app.wait_for("src/rename_old.go → src/rename_new.go").unwrap();

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
}

#[test]
fn ui_narrow_pane_1040x660_and_100px_more_menu_stash() {
    let _guard = lock_env();
    let fx = Fixture::new("narrow");
    let mut app = fx.start_headless(1040.0, 660.0);

    // 1. 1040x660 默认左栏 320px：标题行与计数单行展示且按钮可见
    app.wait_for("id:changes-header-title-row").unwrap();
    app.wait_for("变更视图选项").unwrap();
    app.wait_for("Stash").unwrap();

    // 2. 通过配置以极窄左栏 100px 启动，验证工具栏自动收入 More 菜单且可直达 Stash -> 管理 Stash…
    let narrow_cfg = serde_json::json!({
        "workspace_path": fx.repo_dir.to_string_lossy(),
        "theme": "light",
        "commit_left_width": 100.0
    });
    fs::write(&fx.config_file, serde_json::to_string_pretty(&narrow_cfg).unwrap()).unwrap();
    drop(app);

    let mut narrow_app = fx.start_headless_expect(1040.0, 660.0, "id:changes-toolbar-more");
    narrow_app.press("id:changes-toolbar-more").unwrap();
    narrow_app.choose_menu_item("Stash > 管理 Stash… (0)").unwrap();
    narrow_app.wait_for("id:stash-modal").unwrap();
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
    let mut app = fx.start_headless(1440.0, 900.0);

    // 打开“变更视图选项”并启用“单击时预览差异”
    app.press("id:changes-view-options-btn").unwrap();
    app.choose_menu_item("单击时预览差异").unwrap();

    // 开启后单击 src/app.go 立即打开工作区 Diff
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();

    // 切换树形 -> 平铺，验证显示完整路径 src/app.go
    app.press("id:changes-flat-toggle-btn").unwrap();
    app.wait_for("平铺").unwrap();
    app.wait_for("src/app.go").unwrap();
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
    app.wait_for("查找：new-greeting").unwrap();
    app.wait_for("id:find-results-hit-0").unwrap();

    // 3. 点击命中直接打开源码（行/UTF-16 列来自快照）。
    app.press("id:find-results-hit-0").unwrap();
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
    app.wait_for("查找：new-greeting").unwrap();
    app.wait_for("id:find-results-hit-0").unwrap();
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
    app.wait_for(&format!("id:file-revision-{}", old_hash.trim())).unwrap();
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
    app.press_key("mod+d").unwrap();
    app.wait_for("id:comparison-pane").unwrap();
    assert!(app.screen().matches("id:comparison-left-editor").unwrap());
    app.wait_for("变更 2").unwrap();
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
fn ui_project_follow_editor_selects_open_file_without_focus_shift() {
    let _guard = lock_env();
    let fx = Fixture::new("project-follow-editor");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 变更行打开 src/app.go，再切到 Project 并打开 README。
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
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
    let mut app = fx.start_headless(1440.0, 900.0);

    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.wait_for("可轻编辑").unwrap();

    // 切到暂存区 -> 只读
    app.press("id:diff-scope-staged").unwrap();
    app.wait_for("只读").unwrap();

    // 切回未暂存 -> 切单栏模式 -> 切回双栏
    app.press("id:diff-scope-unstaged").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-unified-editor").unwrap();
    app.press("id:diff-layout-toggle").unwrap();
    app.wait_for("id:working-diff-new-editor").unwrap();

    // 折叠/展开未改动行按钮可点击无报错
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
    // 第一次按 F7 出现边界提示“再按 F7 切到下一文件”
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.press_key("f7").unwrap();
    app.wait_for("再按 F7 切到下一文件").unwrap();

    // 第二次按 F7 切换到下一个变更文件 notes.txt
    app.press_key("f7").unwrap();
    app.wait_for("Diff: notes.txt").unwrap();
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
fn ui_working_diff_right_pane_edit_and_autosave_on_switch() {
    let _guard = lock_env();
    let fx = Fixture::new("diff-right-edit");
    let mut app = fx.start_headless(1440.0, 900.0);

    // 打开 src/app.go 工作区 Diff（右侧可轻编辑）
    app.press("id:change-row-unstaged-src/app.go").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("id:working-diff-pane").unwrap();
    app.wait_for("可轻编辑").unwrap();

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
    app.wait_for("再按 ⇧F7 切到上一文件").unwrap();
    app.press_key("shift+f7").unwrap();
    app.wait_for("Diff: app.go").unwrap();
    app.wait_for("2/2").unwrap();

    // 2. 切到 ws_only.txt 并开启「忽略空白」使其变为 0 处差异，连按两次 ⇧F7 必须能跨回 notes.txt！
    app.press("id:change-row-unstaged-ws_only.txt").unwrap();
    app.press_key("mod+d").unwrap();
    app.wait_for("Diff: ws_only.txt").unwrap();
    app.press("id:diff-whitespace-btn").unwrap();
    app.choose_menu_item("忽略空白").unwrap();
    app.wait_for("0 处差异").unwrap();

    app.press_key("shift+f7").unwrap();
    app.wait_for("再按 ⇧F7 切到上一文件").unwrap();
    app.press_key("shift+f7").unwrap();
    app.wait_for("Diff: notes.txt").unwrap();
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
    app.type_text("id:git-filter-input", "initial|missing").unwrap();
    app.wait_for_gone(current.as_str(), Duration::from_secs(4)).unwrap();
    app.wait_for(initial.as_str()).unwrap();
    app.type_text("id:git-author-filter", "我").unwrap();
    app.type_text("id:git-path-filter", "*.go").unwrap();
    app.wait_for(initial.as_str()).unwrap();
    app.press("id:git-clear-filters").unwrap();
    app.wait_for(current.as_str()).unwrap();
    app.press("id:git-group-filter").unwrap();
    app.choose_menu_item("按仓库").unwrap();
    app.press(current.as_str()).unwrap();
    app.wait_for("id:git-commit-file-src/rename_new.go").unwrap();
    app.press("id:git-diff-preview").unwrap();
    app.wait_for("id:commit-diff-pane").unwrap();
    app.press("id:git-diff-preview").unwrap();

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
    app.press("id:git-history-affected").unwrap();
    app.wait_for("id:git-commit-file-README.md").unwrap();
    app.right_click(initial.as_str()).unwrap();
    app.choose_menu_item("设为比较基准").unwrap();
    app.right_click(current.as_str()).unwrap();
    app.choose_menu_item(&format!("与修订 {} 比较…", short("HEAD^"))).unwrap();
    app.wait_for("id:comparison-pane").unwrap();
    app.wait_for(r#"const Version = "v1""#).unwrap();
    app.wait_for(r#"const Version = "v2""#).unwrap();
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
    app.wait_for("id:diff-stage-hunk").unwrap();
    app.press("id:diff-stage-hunk").unwrap();
    app.wait_for("id:change-row-staged-many.txt").unwrap();
    let read = || nemorous::git::run_git_string(&fx.repo_dir, &["show", ":many.txt"]).unwrap();
    assert_eq!(read(), format!("new first\n{context}old last\n"));
    assert_eq!(fs::read_to_string(&path).unwrap(), new);
    app.press("id:diff-scope-staged").unwrap();
    app.press("id:diff-stage-hunk").unwrap();
    app.wait_for("已取消暂存所选变更").unwrap();
    assert_eq!(read(), old);
    assert_eq!(fs::read_to_string(&path).unwrap(), new);
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
    app.wait_for("✓ 纳入提交").unwrap();
    app.press("id:diff-include-hunk").unwrap();
    assert!(!app.screen().matches("✓ 纳入提交").unwrap(), "first hunk checkbox must be visibly cleared");
    app.type_text("id:git-commit-message", "Commit second hunk").unwrap();
    assert!(!app.screen().matches("✓ 纳入提交").unwrap(), "typing the message must preserve hunk exclusion");
    app.press("id:git-commit-submit").unwrap();
    app.wait_for("提交成功 1 个仓库，失败 0 个").unwrap();

    let show = |spec: &str| nemorous::git::run_git_string(&fx.repo_dir, &["show", spec]).unwrap();
    assert_eq!(show("HEAD:many.txt"), format!("old first\n{context}new last\n"));
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
    app.press("id:code-language-tools").unwrap();
    app.choose_menu_item("悬停信息").unwrap();
    app.wait_for("synthetic hover").unwrap();
    app.press_key("escape").unwrap();
    app.press("id:code-language-tools").unwrap();
    app.choose_menu_item("格式化文件").unwrap();
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
fn ui_github_lists_checks_and_creates_draft_via_mock_gh() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = lock_env();
    let fx = Fixture::new("mock-gh");
    git_run(&fx.repo_dir, &["remote","add","origin","https://github.com/fixture/nemorous"]);
    let bin = fx.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, r#"#!/bin/sh
printf '%s\n' "$@" > gh-argv
case "$2" in
list) printf '%s' '[{"number":7,"title":"Fixture PR"}]' ;;
view) printf '%s' '{"number":7,"title":"Fixture PR","url":"https://github.com/fixture/nemorous/pull/7","body":"Fixture review body","statusCheckRollup":[{"name":"unit","status":"COMPLETED","conclusion":"SUCCESS"}],"comments":[{"author":{"login":"qa"},"body":"Fixture comment"}]}' ;;
create) printf '%s' 'https://github.com/fixture/nemorous/pull/8' ;;
*) exit 1 ;;
esac
"#).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
    struct PathGuard(std::ffi::OsString);
    impl Drop for PathGuard {
        fn drop(&mut self) { unsafe { std::env::set_var("PATH",&self.0); } }
    }
    let path_guard = PathGuard(std::env::var_os("PATH").unwrap_or_default());
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path_guard.0))).unwrap();
    unsafe { std::env::set_var("PATH",path); }
    let mut app = fx.start_headless(1440.0,900.0);
    app.press("id:rail-git-btn").unwrap();
    app.press("id:github-pr-ci").unwrap();
    app.wait_for("id:gh-pr-7").unwrap();
    app.press("id:gh-pr-7").unwrap();
    app.wait_for("unit · SUCCESS").unwrap();
    app.wait_for("Fixture comment").unwrap();
    app.press("id:gh-new").unwrap();
    app.type_text("id:gh-pr-title","Draft fixture").unwrap();
    app.press("id:gh-pr-draft").unwrap();
    app.press("id:gh-submit").unwrap();
    app.wait_for_gone("id:gh-submit",Duration::from_secs(2)).unwrap();
    app.wait_for("text:https://github.com/fixture/nemorous/pull/8").unwrap();
    let command = fs::read_to_string(fx.repo_dir.join("gh-argv")).unwrap();
    assert!(command.contains("create\n--title\nDraft fixture\n"), "{command:?}");
    assert!(command.contains("--draft\n--repo\nfixture/nemorous\n"));
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
