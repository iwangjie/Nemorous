use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use nemorous::config::{self, Config};
use nemorous::diff_model::{
    DiffSide, InlineHighlightMode, WhitespaceMode, compute_diff, compute_inline_ranges,
    compute_unchanged_fold_ranges, source_position,
};
use nemorous::git::{self, LineEnding, SaveReviewOutcome};
use nemorous::local_history::LocalHistoryStore;
use nemorous::paths::{
    byte_offset_to_utf16_col, display_path_with_home, parse_file_location, parse_goto_line_col,
    path_with_line_reference, utf16_col_to_byte_offset,
};
use nemorous::watcher::{classify_watch_hints, should_ignore_watch_path};
use nsh_base::EditorPosition;

struct TempDirGuard(PathBuf);

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "nemorous-test-{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git_cmd(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.com")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.com")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_sample_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git_cmd(dir, &["init", "-b", "main"]);
    git_cmd(dir, &["config", "user.name", "Tester"]);
    git_cmd(dir, &["config", "user.email", "tester@example.com"]);
}

#[test]
fn config_compatibility_preserves_unknown_fields_and_defaults() {
    let tmp = TempDirGuard::new("cfg");
    let path = tmp.path().join("config.json");
    let legacy = r#"{
        "workspace_path": "/tmp/ws-demo",
        "theme": "darcula",
        "change_diff_preview": true,
        "diff_side_by_side": false,
        "diff_ignore_mode": 2,
        "legacy_ai_model": "gpt-test",
        "db_connections": [{"name": "local"}]
    }"#;
    fs::write(&path, legacy).unwrap();

    let mut cfg = config::load_from(&path).unwrap();
    assert_eq!(cfg.workspace_path, "/tmp/ws-demo");
    assert_eq!(cfg.theme, "darcula");
    assert!(cfg.change_diff_preview);
    assert!(cfg.diff_single_column);
    assert_eq!(cfg.diff_whitespace_mode, "all");
    assert_eq!(
        WhitespaceMode::from_config(&cfg.diff_whitespace_mode),
        WhitespaceMode::All
    );
    assert_eq!(cfg.editor_font_size, 14.0);
    assert!(cfg.extra.contains_key("legacy_ai_model"));
    assert!(cfg.extra.contains_key("db_connections"));

    cfg.changes_flat = true;
    config::save_to(&path, &cfg).unwrap();
    let reloaded = config::load_from(&path).unwrap();
    assert!(reloaded.changes_flat);
    assert_eq!(
        reloaded.extra.get("legacy_ai_model").and_then(|v| v.as_str()),
        Some("gpt-test")
    );
    let _default_cfg = Config::default();
}

#[test]
fn paths_display_tilde_boundary_utf16_and_location_parsing() {
    assert_eq!(
        display_path_with_home("/Users/alice/work/a.go", Some("/Users/alice")),
        "~/work/a.go"
    );
    assert_eq!(
        display_path_with_home("/Users/alice", Some("/Users/alice")),
        "~"
    );
    // 绝不把 /Users/alice2 误折叠成 ~/2
    assert_eq!(
        display_path_with_home("/Users/alice2/work/a.go", Some("/Users/alice")),
        "/Users/alice2/work/a.go"
    );
    assert_eq!(
        display_path_with_home("/tmp/outside/note.txt", Some("/Users/alice")),
        "/tmp/outside/note.txt"
    );

    assert_eq!(
        path_with_line_reference("/ws", "/ws/sub/main.rs", "main.rs", 12),
        "sub/main.rs:12"
    );

    let loc = parse_file_location("~/repo/src/lib.rs:42:7");
    assert_eq!(loc.path, "~/repo/src/lib.rs");
    assert_eq!(loc.line, Some(42));
    assert_eq!(loc.col, Some(7));

    let loc2 = parse_file_location("src/app.go#L18C3");
    assert_eq!(loc2.path, "src/app.go");
    assert_eq!(loc2.line, Some(18));
    assert_eq!(loc2.col, Some(3));

    assert_eq!(parse_goto_line_col("15:4"), Some((14, 3)));
    assert_eq!(parse_goto_line_col("9"), Some((8, 0)));
    assert_eq!(parse_goto_line_col("0"), None);

    // 中文 + Emoji 代理对 UTF-16 坐标转换
    let line = "你好😀world";
    // "你好" = 2 UTF-16 units (6 bytes), "😀" = 2 UTF-16 units (4 bytes)
    assert_eq!(byte_offset_to_utf16_col(line, 6), 2);
    assert_eq!(byte_offset_to_utf16_col(line, 10), 4);
    assert_eq!(utf16_col_to_byte_offset(line, 4), 10);
}

#[test]
fn diff_model_four_whitespace_modes_inline_and_source_mapping() {
    let old = "fn main() {\n    let a = 1;\n\n    println!(\"old\");\n}\n";
    let new = "fn main() {\n  let a = 1;  \n    println!(\"new\");\n}\n";

    let d_none = compute_diff(old, new, WhitespaceMode::None);
    assert!(!d_none.hunks.is_empty());

    let d_trim = compute_diff("  hello  \n", "hello\n", WhitespaceMode::Trim);
    assert!(d_trim.hunks.is_empty());

    let d_all = compute_diff("let a = 1;\n", "let   a=1;\n", WhitespaceMode::All);
    assert!(d_all.hunks.is_empty());

    let d_empty = compute_diff("a\n\n\nb\n", "a\nb\n", WhitespaceMode::AllAndEmptyLines);
    assert!(d_empty.hunks.is_empty());

    // 行内词级 / 字符级高亮
    let (old_w, new_w) =
        compute_inline_ranges("let value = old_name;", "let value = new_name;", InlineHighlightMode::Word);
    assert_eq!(old_w.len(), 1);
    assert_eq!(new_w.len(), 1);

    let (old_c, new_c) = compute_inline_ranges("abc", "axc", InlineHighlightMode::Char);
    assert_eq!(old_c, vec![1..2]);
    assert_eq!(new_c, vec![1..2]);

    // 旧侧行号到新侧源码位置映射
    let old_doc = "L1\nL2\nL3\nL4\nL5";
    let new_doc = "L1\nADDED1\nADDED2\nL2\nL3\nL4\nL5";
    let diff = compute_diff(old_doc, new_doc, WhitespaceMode::None);
    // 旧侧第 3 行 (0-based idx 2 = "L3") 在新侧变为 idx 4
    let mapped = source_position(new_doc, &diff.hunks, DiffSide::Old, EditorPosition::new(2, 1));
    assert_eq!(mapped, EditorPosition::new(4, 1));

    // 未改动行折叠范围
    let folds = compute_unchanged_fold_ranges(30, &[15..16], 3);
    assert_eq!(folds.len(), 2);
    assert_eq!(folds[0].start_line, 0);
    assert_eq!(folds[0].end_line, 11);
}

#[test]
fn git_multi_repo_mani_status_unicode_spaces_and_rename() {
    let tmp = TempDirGuard::new("multi");
    let ws = tmp.path();
    let repo_a = ws.join("repo-a");
    let repo_b = ws.join("repo-b");
    init_sample_repo(&repo_a);
    init_sample_repo(&repo_b);

    fs::write(
        ws.join("mani.yaml"),
        "projects:\n  repo-a:\n    path: repo-a\n    desc: Core A\n  repo-b:\n    path: repo-b\n",
    )
    .unwrap();

    // 中文与空格文件名 + 重命名
    fs::write(repo_a.join("中文 文件.txt"), "第一行\n第二行\n").unwrap();
    fs::write(repo_a.join("old_name.go"), "package main\nvar x = 1\n").unwrap();
    git_cmd(&repo_a, &["add", "."]);
    git_cmd(&repo_a, &["commit", "-m", "initial"]);

    // 暂存重命名 + 修改中文文件 + 未跟踪文件
    git_cmd(&repo_a, &["mv", "old_name.go", "new_name.go"]);
    fs::write(repo_a.join("new_name.go"), "package main\nvar x = 2\n").unwrap();
    git_cmd(&repo_a, &["add", "new_name.go"]);
    fs::write(repo_a.join("中文 文件.txt"), "第一行\n第二行修改\n").unwrap();
    fs::write(repo_a.join("未跟踪.md"), "hello\n").unwrap();

    fs::write(repo_b.join("b.txt"), "from b\n").unwrap();

    let repos = git::discover_workspace_repos(&ws.to_string_lossy(), &[]);
    assert_eq!(repos.len(), 2);

    let st_a = git::inspect_repo(&repos[0]);
    assert!(st_a.error.is_none());
    let renamed = st_a
        .files
        .iter()
        .find(|f| f.path == "new_name.go")
        .expect("find renamed file");
    assert_eq!(renamed.kind, "R");
    assert_eq!(renamed.orig_path, "old_name.go");
    assert!(renamed.staged);

    // 暂存重命名 Diff 必须使用 orig_path 读取 HEAD 旧版本！
    let staged_diff = git::read_review_change(renamed, true);
    assert_eq!(staged_diff.old_path, "old_name.go");
    assert_eq!(staged_diff.new_path, "new_name.go");
    assert!(staged_diff.old_text.contains("var x = 1"));
    assert!(staged_diff.new_text.contains("var x = 2"));
    assert!(!staged_diff.editable);

    let cn = st_a
        .files
        .iter()
        .find(|f| f.path == "中文 文件.txt")
        .expect("find chinese filename with space");
    assert!(cn.unstaged);

    let untracked = st_a
        .files
        .iter()
        .find(|f| f.path == "未跟踪.md")
        .expect("find untracked");
    assert!(untracked.untracked);
}

#[test]
fn git_checked_save_keeps_crlf_permissions_rejects_deleted_and_records_history() {
    let tmp = TempDirGuard::new("save");
    let repo = tmp.path().join("repo");
    init_sample_repo(&repo);
    let hist = LocalHistoryStore::new(tmp.path().join("history"));

    let crlf_path = repo.join("script.sh");
    fs::write(&crlf_path, b"line1\r\nline2\r\n").unwrap();
    fs::set_permissions(&crlf_path, fs::Permissions::from_mode(0o755)).unwrap();

    let read = git::read_review_worktree(&crlf_path);
    assert_eq!(read.line_ending, LineEnding::CrLf);
    assert_eq!(read.text, "line1\nline2\n");
    assert!(read.editable);

    // 受检保存：保持 CRLF、末尾空行与 0755 权限
    let saved = git::save_review_text(
        "repo",
        "script.sh",
        &crlf_path,
        &read.version,
        read.line_ending,
        "line1\nline2-edited\n",
        Some(&hist),
    );
    let new_ver = match saved {
        SaveReviewOutcome::Saved { version } => version,
        other => panic!("expected Saved, got {other:?}"),
    };
    let on_disk = fs::read(&crlf_path).unwrap();
    assert_eq!(on_disk, b"line1\r\nline2-edited\r\n");
    let mode = fs::metadata(&crlf_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);

    // 验证本地历史快照已记录旧内容
    let snaps = hist.list("repo", "script.sh").unwrap();
    assert_eq!(snaps.len(), 1);
    assert_eq!(
        hist.read_content("repo", "script.sh", &snaps[0].id).unwrap(),
        b"line1\r\nline2\r\n"
    );

    // 外部修改磁盘后保存必须触发 Conflict
    fs::write(&crlf_path, b"external change\r\n").unwrap();
    let conflict = git::save_review_text(
        "repo",
        "script.sh",
        &crlf_path,
        &new_ver,
        read.line_ending,
        "my edit\n",
        Some(&hist),
    );
    match conflict {
        SaveReviewOutcome::Conflict {
            disk_text,
            disk_deleted,
            ..
        } => {
            assert!(!disk_deleted);
            assert_eq!(disk_text, "external change\n");
        }
        other => panic!("expected Conflict, got {other:?}"),
    }

    // 外部删除磁盘文件后保存绝不能静默重建
    fs::remove_file(&crlf_path).unwrap();
    let deleted = git::save_review_text(
        "repo",
        "script.sh",
        &crlf_path,
        &new_ver,
        read.line_ending,
        "should not recreate\n",
        Some(&hist),
    );
    match deleted {
        SaveReviewOutcome::Conflict { disk_deleted, .. } => assert!(disk_deleted),
        other => panic!("expected deleted Conflict, got {other:?}"),
    }
    assert!(!crlf_path.exists());
}

#[test]
fn git_read_worktree_rejects_over_2mib_and_binary() {
    let tmp = TempDirGuard::new("limits");
    let big = tmp.path().join("big.txt");
    let big_bytes = vec![b'a'; git::MAX_REVIEW_BYTES + 16];
    fs::write(&big, &big_bytes).unwrap();
    let r_big = git::read_review_worktree(&big);
    assert!(r_big.too_large);
    assert!(!r_big.editable);

    let bin = tmp.path().join("bin.dat");
    fs::write(&bin, b"abc\0def").unwrap();
    let r_bin = git::read_review_worktree(&bin);
    assert!(r_bin.binary);
    assert!(!r_bin.editable);
}

#[test]
fn git_commit_history_and_commit_diff_rename_delete_add() {
    let tmp = TempDirGuard::new("history");
    let repo = tmp.path().join("repo");
    init_sample_repo(&repo);

    fs::write(repo.join("keep.txt"), "v1\n").unwrap();
    fs::write(repo.join("drop.txt"), "bye\n").unwrap();
    fs::write(
        repo.join("before.rs"),
        "pub fn alpha() -> i32 {\n    let a = 1;\n    let b = 2;\n    a + b\n}\n",
    )
    .unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-m", "c1"]);

    fs::remove_file(repo.join("drop.txt")).unwrap();
    git_cmd(&repo, &["mv", "before.rs", "after.rs"]);
    fs::write(
        repo.join("after.rs"),
        "pub fn alpha() -> i32 {\n    let a = 1;\n    let b = 3;\n    a + b\n}\n",
    )
    .unwrap();
    fs::write(repo.join("added.rs"), "fn added() {}\n").unwrap();
    git_cmd(&repo, &["add", "-A"]);
    git_cmd(&repo, &["commit", "-m", "c2 rename delete add"]);

    let repos = git::discover_workspace_repos(&repo.to_string_lossy(), &[]);
    let log = git::repo_log(&repos[0], 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].subject, "c2 rename delete add");

    let files = git::commit_files(&repos[0].path, &log[0].hash).unwrap();
    let ren = files.iter().find(|f| f.path == "after.rs").unwrap();
    assert_eq!(ren.status, "R");
    assert_eq!(ren.orig_path, "before.rs");
    let diff_ren = git::read_commit_diff(&log[0], ren).unwrap();
    assert_eq!(diff_ren.old_path, "before.rs");
    assert_eq!(diff_ren.new_path, "after.rs");
    assert!(diff_ren.old_text.contains("let b = 2;"));
    assert!(diff_ren.new_text.contains("let b = 3;"));

    let del = files.iter().find(|f| f.path == "drop.txt").unwrap();
    assert_eq!(del.status, "D");
    let diff_del = git::read_commit_diff(&log[0], del).unwrap();
    assert!(diff_del.old_text.contains("bye"));
    assert!(diff_del.new_text.is_empty());

    let add = files.iter().find(|f| f.path == "added.rs").unwrap();
    assert_eq!(add.status, "A");
    let diff_add = git::read_commit_diff(&log[0], add).unwrap();
    assert!(diff_add.old_text.is_empty());
    assert!(diff_add.new_text.contains("fn added()"));
}

#[test]
fn watcher_classifies_multi_repo_hints_and_ignores_build_dirs() {
    let tmp = TempDirGuard::new("watch");
    let ws = tmp.path();
    let r1 = ws.join("svc-a");
    let r2 = ws.join("svc-b");
    init_sample_repo(&r1);
    init_sample_repo(&r2);

    assert!(should_ignore_watch_path(&r1.join("node_modules/a/index.js")));
    assert!(should_ignore_watch_path(&r1.join(".git/objects/ab/123")));
    assert!(!should_ignore_watch_path(&r1.join("src/main.rs")));

    let repos = git::discover_workspace_repos(&ws.to_string_lossy(), &[]);
    let hint = classify_watch_hints(
        ws,
        &repos,
        &[
            r1.join("src/main.rs"),
            r1.join("node_modules/x.js"),
            r2.join(".git/index"),
        ],
    );
    assert!(hint.worktree_changed);
    assert!(hint.git_meta_changed);
    assert!(hint.repo_ids.contains("svc-a"));
    assert!(hint.repo_ids.contains("svc-b"));

    // 工作区自身位于名为 build/ 的祖先目录时，内部普通文件不得被误忽略，但内部 build/ 产物仍应忽略
    let nested_ws = tmp.path().join("build/nested-ws");
    init_sample_repo(&nested_ws);
    let nested_repos = git::discover_workspace_repos(&nested_ws.to_string_lossy(), &[]);
    let h_nested = classify_watch_hints(
        &nested_ws,
        &nested_repos,
        &[nested_ws.join("agent.txt"), nested_ws.join("build/out.bin")],
    );
    assert!(h_nested.worktree_changed);
    assert_eq!(h_nested.changed_paths.len(), 1);
    assert!(h_nested.changed_paths[0].ends_with("agent.txt"));
}

#[test]
fn config_whitespace_four_modes_roundtrip_with_legacy_json() {
    let tmp = TempDirGuard::new("ws-modes");
    let path = tmp.path().join("config.json");

    let cases = [
        (0u64, WhitespaceMode::None, "none"),
        (1u64, WhitespaceMode::Trim, "trim"),
        (2u64, WhitespaceMode::All, "all"),
        (3u64, WhitespaceMode::AllAndEmptyLines, "all_and_empty"),
    ];
    for (legacy_code, expected_mode, expected_str) in cases {
        // 1. 从旧版 Go JSON 整数 diff_ignore_mode 读取 -> 必须映射为有效 WhitespaceMode
        let raw = serde_json::json!({
            "workspace_path": "/tmp/ws",
            "diff_ignore_mode": legacy_code,
            "diff_side_by_side": true
        });
        fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();
        let mut cfg = config::load_from(&path).unwrap();
        assert_eq!(cfg.diff_whitespace_mode, expected_str);
        assert_eq!(
            WhitespaceMode::from_config(&cfg.diff_whitespace_mode),
            expected_mode
        );

        // 2. 新版 UI 写入 as_config() -> 原子保存 -> 核对旧键 diff_ignore_mode 与再次加载不归零
        cfg.diff_whitespace_mode = expected_mode.as_config().to_string();
        config::save_to(&path, &cfg).unwrap();
        let saved_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved_json.get("diff_ignore_mode").and_then(|v| v.as_u64()),
            Some(legacy_code)
        );
        let reloaded = config::load_from(&path).unwrap();
        assert_eq!(
            WhitespaceMode::from_config(&reloaded.diff_whitespace_mode),
            expected_mode
        );
    }
}

#[test]
fn diff_model_trailing_newline_counts_and_unified_all_and_empty_keeps_all_lines() {
    // 1. 单行末尾换行符不应计为 +2 -0，真实末尾空行仍应保留
    let d_new = compute_diff("", "untracked\n", WhitespaceMode::None);
    assert_eq!(d_new.added_lines, 1);
    assert_eq!(d_new.deleted_lines, 0);
    assert_eq!(d_new.new_line_count, 1);

    let d_del = compute_diff("removed\n", "", WhitespaceMode::None);
    assert_eq!(d_del.added_lines, 0);
    assert_eq!(d_del.deleted_lines, 1);
    assert_eq!(d_del.old_line_count, 1);

    let d_trailing_blank = compute_diff("a\n", "a\n\n", WhitespaceMode::None);
    assert_eq!(d_trailing_blank.old_line_count, 1);
    assert_eq!(d_trailing_blank.new_line_count, 2);
    assert_eq!(d_trailing_blank.added_lines, 1);

    // 2. 单栏模式在 AllAndEmptyLines 下，新侧在 hunk 前多出空行时绝不能丢弃真实代码行
    let old_text = "alpha\nbeta\ngamma\nold_tail\n";
    let new_text = "alpha\n\n\nbeta\n\ngamma\nnew_tail\n";
    let d_empty = compute_diff(old_text, new_text, WhitespaceMode::AllAndEmptyLines);
    assert_eq!(d_empty.hunks.len(), 1);
    let unified = nemorous::diff_model::build_unified_diff(
        old_text,
        new_text,
        &d_empty.hunks,
        nemorous::theme::light_palette(),
    );
    assert!(
        unified.text.contains("  alpha")
            && unified.text.contains("  beta")
            && unified.text.contains("  gamma")
            && unified.text.contains("- old_tail")
            && unified.text.contains("+ new_tail"),
        "unified diff under AllAndEmptyLines dropped context lines:\n{}",
        unified.text
    );

    // 3. 单栏行映射与 hunk 起始行一致性验证（旧侧 - 行与新侧 + 行映射）
    let (lmap, hstarts, _) =
        nemorous::diff_model::compute_unified_layout(old_text, new_text, &d_empty.hunks);
    assert_eq!(lmap, unified.line_map);
    assert_eq!(hstarts, unified.hunk_start_lines);
    let u_old = nemorous::diff_model::map_side_line_to_unified(
        old_text,
        new_text,
        &d_empty.hunks,
        DiffSide::Old,
        3,
    );
    assert_eq!(lmap[u_old].side, DiffSide::Old);
    assert_eq!(lmap[u_old].orig_line, 3);
    let u_new = nemorous::diff_model::map_side_line_to_unified(
        old_text,
        new_text,
        &d_empty.hunks,
        DiffSide::New,
        6,
    );
    assert_eq!(lmap[u_new].side, DiffSide::New);
    assert_eq!(lmap[u_new].orig_line, 6);
}

#[test]
fn git_large_outputs_over_64kib_and_near_2mib_tracked_diff() {
    let tmp = TempDirGuard::new("large-git");
    let repo = tmp.path().join("big-repo");
    init_sample_repo(&repo);

    // 1. 生成 ~1.9 MiB 已跟踪文本文件（远超 64 KiB 管道缓冲）并修改其中一行
    let mut big_v1 = String::with_capacity(1_950_000);
    for i in 0..30_000 {
        big_v1.push_str(&format!(
            "line_{i:05}: 0123456789abcdef0123456789abcdef0123456789abcdef\n"
        ));
    }
    assert!(big_v1.len() > 1_800_000 && big_v1.len() < git::MAX_REVIEW_BYTES);
    fs::write(repo.join("big_tracked.txt"), &big_v1).unwrap();

    // 2. 同时提交一个 >2 MiB 的大文件，检验 git cat-file blob 流式 2 MiB+1 拦截
    let over_blob = vec![b'z'; git::MAX_REVIEW_BYTES + 65_536];
    fs::write(repo.join("over_limit.txt"), &over_blob).unwrap();

    git_cmd(&repo, &["add", "-A"]);
    git_cmd(&repo, &["commit", "-m", "initial big files"]);

    // 3. 修改 1.9 MiB 已跟踪文件，验证 read_review_change 无死锁且旧/新内容完整
    let big_v2 = big_v1.replacen("line_15000:", "line_15000_EDITED:", 1);
    fs::write(repo.join("big_tracked.txt"), &big_v2).unwrap();

    let t0 = std::time::Instant::now();
    let repos = git::discover_workspace_repos(&repo.to_string_lossy(), &[]);
    let status = git::inspect_repo(&repos[0]);
    let cf = status
        .files
        .iter()
        .find(|f| f.path == "big_tracked.txt")
        .expect("big_tracked.txt modified");
    let diff = git::read_review_change(cf, false);
    assert!(
        t0.elapsed().as_secs_f64() < 5.0,
        "read_review_change >64KiB should never hit 20s pipe deadlock, took {:?}",
        t0.elapsed()
    );
    assert!(!diff.too_large && !diff.binary);
    assert_eq!(diff.old_text.len(), big_v1.len());
    assert_eq!(diff.new_text.len(), big_v2.len());
    assert!(diff.old_text.contains("line_15000:"));
    assert!(diff.new_text.contains("line_15000_EDITED:"));

    // 4. 验证 >2 MiB Git blob 流式截断并标记 too_large=true
    let (_txt, _bin, too_large) =
        git::read_git_object_text(&repo, "HEAD:over_limit.txt").unwrap();
    assert!(too_large, ">2 MiB git blob must be flagged too_large");

    // 5. 生成 2,500 个已跟踪文件（git ls-files -z > 130 KiB），验证 list_workspace_files 无死锁
    let bulk = repo.join("bulk");
    fs::create_dir_all(&bulk).unwrap();
    for i in 0..2500 {
        fs::write(bulk.join(format!("item_{i:04}_with_longer_filename_padding.txt")), "ok\n")
            .unwrap();
    }
    git_cmd(&repo, &["add", "bulk"]);
    git_cmd(&repo, &["commit", "-m", "add 2500 files"]);

    let t_files = std::time::Instant::now();
    let listed = git::list_workspace_files(&repos);
    assert!(
        listed.len() >= 2500,
        "expected >=2500 listed files, got {}",
        listed.len()
    );
    assert!(
        t_files.elapsed().as_secs_f64() < 5.0,
        "2500-file git ls-files took {:?}",
        t_files.elapsed()
    );

    // 6. 生成 45 条长说明提交（git log > 85 KiB），验证 repo_log 无死锁
    let long_subject = "commit_payload_".repeat(130);
    for i in 0..45 {
        git_cmd(
            &repo,
            &["commit", "--allow-empty", "-m", &format!("{i:02} {long_subject}")],
        );
    }
    let t_log = std::time::Instant::now();
    let commits = git::repo_log(&repos[0], 100).unwrap();
    assert!(commits.len() >= 47);
    assert!(
        t_log.elapsed().as_secs_f64() < 5.0,
        ">85KiB git log took {:?}",
        t_log.elapsed()
    );
}

#[test]
fn save_review_text_in_repo_enforces_2mib_limit_and_repo_boundary() {
    let tmp = TempDirGuard::new("save-guard");
    let repo = tmp.path().join("repo");
    let outside = tmp.path().join("outside");
    init_sample_repo(&repo);
    fs::create_dir_all(&outside).unwrap();

    let inside_file = repo.join("inside.txt");
    fs::write(&inside_file, "ok\n").unwrap();
    let outside_file = outside.join("secret.txt");
    fs::write(&outside_file, "secret\n").unwrap();

    let wt = git::read_review_worktree(&inside_file);

    // 1. 超过 2 MiB 待写入文本必须被拒绝
    let huge = "a".repeat(git::MAX_REVIEW_BYTES + 10);
    match git::save_review_text_in_repo(
        "r1",
        Some(&repo),
        "inside.txt",
        &inside_file,
        &wt.version,
        LineEnding::Lf,
        &huge,
        None,
    ) {
        SaveReviewOutcome::Rejected { reason } => assert!(reason.contains("上限")),
        other => panic!("expected >2MiB write rejected, got {other:?}"),
    }

    // 2. 相对路径含 `..` 越出仓库边界必须被拒绝
    match git::save_review_text_in_repo(
        "r1",
        Some(&repo),
        "../outside/secret.txt",
        &outside_file,
        &wt.version,
        LineEnding::Lf,
        "pwned\n",
        None,
    ) {
        SaveReviewOutcome::Rejected { reason } => assert!(reason.contains("仓库边界")),
        other => panic!("expected path traversal rejected, got {other:?}"),
    }

    // 3. 父目录符号链接逃逸仓库边界必须被拒绝
    let link_dir = repo.join("escape_link");
    std::os::unix::fs::symlink(&outside, &link_dir).unwrap();
    let escaped = link_dir.join("secret.txt");
    let wt_out = git::read_review_worktree(&outside_file);
    match git::save_review_text_in_repo(
        "r1",
        Some(&repo),
        "escape_link/secret.txt",
        &escaped,
        &wt_out.version,
        LineEnding::Lf,
        "pwned\n",
        None,
    ) {
        SaveReviewOutcome::Rejected { reason } => assert!(reason.contains("仓库边界")),
        other => panic!("expected symlink parent escape rejected, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(&outside_file).unwrap(), "secret\n");
}
