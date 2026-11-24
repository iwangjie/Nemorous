use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use nemorous::diff_model::{
    InlineHighlightMode, WhitespaceMode, build_side_by_side_decorations, compute_diff,
};
use nemorous::git;
use nemorous::theme::light_palette;

#[cfg(unix)]
fn current_max_rss_kib() -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        tv_sec: i64,
        tv_usec: i64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rusage {
        ru_utime: Timeval,
        ru_stime: Timeval,
        ru_maxrss: i64,
        _rest: [i64; 13],
    }
    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
    }
    let mut usage = Rusage::default();
    let rc = unsafe { getrusage(0, &mut usage) };
    if rc == 0 && usage.ru_maxrss > 0 {
        #[cfg(target_os = "macos")]
        {
            (usage.ru_maxrss as u64) / 1024
        }
        #[cfg(not(target_os = "macos"))]
        {
            usage.ru_maxrss as u64
        }
    } else {
        0
    }
}

#[cfg(not(unix))]
fn current_max_rss_kib() -> u64 {
    0
}

#[test]
fn perf_large_diff_and_50_file_switches() {
    let tmp = std::env::temp_dir().join(format!("nemorous-perf-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // 1. 多 hunk 短行 (20,000 行，约 100 处修改)
    let mut old_short = String::new();
    let mut new_short = String::new();
    for i in 0..20_000 {
        old_short.push_str(&format!("fn line_{i:05}() -> i32 {{ {i} }}\n"));
        if i % 200 == 0 {
            new_short.push_str(&format!("fn line_{i:05}() -> i32 {{ {} }} // edited\n", i + 1));
        } else {
            new_short.push_str(&format!("fn line_{i:05}() -> i32 {{ {i} }}\n"));
        }
    }
    let t0 = Instant::now();
    let d_short = compute_diff(&old_short, &new_short, WhitespaceMode::None);
    let (_od, _nd) = build_side_by_side_decorations(
        &old_short,
        &new_short,
        &d_short.hunks,
        InlineHighlightMode::Word,
        light_palette(),
    );
    let short_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(d_short.hunks.len(), 100);

    // 2. 长行（单行 12,000 字符）
    let long_a = "a".repeat(12_000) + "\n";
    let long_b = "a".repeat(6_000) + "b" + &"a".repeat(5_999) + "\n";
    let t1 = Instant::now();
    let d_long = compute_diff(&long_a, &long_b, WhitespaceMode::None);
    let (_ol, _nl) = build_side_by_side_decorations(
        &long_a,
        &long_b,
        &d_long.hunks,
        InlineHighlightMode::Word,
        light_palette(),
    );
    let long_ms = t1.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(d_long.hunks.len(), 1);

    // 3. 接近 2 MiB (~1.9 MiB) 与超过 2 MiB 拦截
    let near_path = tmp.join("near_2mib.txt");
    let near_content = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n"
        .repeat(30_000); // ~1.92 MiB
    fs::write(&near_path, &near_content).unwrap();
    let t2 = Instant::now();
    let r_near = git::read_review_worktree(&near_path);
    let near_read_ms = t2.elapsed().as_secs_f64() * 1000.0;
    assert!(!r_near.too_large);
    assert!(r_near.editable);

    let over_path = tmp.join("over_2mib.txt");
    fs::write(&over_path, vec![b'x'; git::MAX_REVIEW_BYTES + 1024]).unwrap();
    let t3 = Instant::now();
    let r_over = git::read_review_worktree(&over_path);
    let over_ms = t3.elapsed().as_secs_f64() * 1000.0;
    assert!(r_over.too_large);
    assert!(!r_over.editable);

    // 4. 连续切文件 50 次读取 + Diff 计算后观测峰值 RSS
    let rss_before = current_max_rss_kib();
    let t4 = Instant::now();
    for i in 0..50 {
        let p: PathBuf = tmp.join(format!("switch_{i}.rs"));
        let txt = format!("// file {i}\n{}\n", "let x = 1;\n".repeat(400));
        fs::write(&p, &txt).unwrap();
        let read = git::read_review_worktree(&p);
        let edited = read.text.replace("let x = 1;", "let x = 2;");
        let d = compute_diff(&read.text, &edited, WhitespaceMode::None);
        assert!(!d.hunks.is_empty());
    }
    let switch50_ms = t4.elapsed().as_secs_f64() * 1000.0;
    let rss_after = current_max_rss_kib();

    // 5. 真实 Git 仓端到端大 Diff (~1.9 MiB 已跟踪文件 + 重命名 + 受检保存延迟)
    let git_repo = tmp.join("real-git-perf");
    fs::create_dir_all(&git_repo).unwrap();
    let run_git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&git_repo)
            .env("GIT_AUTHOR_NAME", "Perf")
            .env("GIT_AUTHOR_EMAIL", "perf@example.com")
            .env("GIT_COMMITTER_NAME", "Perf")
            .env("GIT_COMMITTER_EMAIL", "perf@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?} failed", args);
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Perf"]);
    run_git(&["config", "user.email", "perf@example.com"]);
    fs::write(git_repo.join("big.txt"), &near_content).unwrap();
    fs::write(
        git_repo.join("ren_a.rs"),
        "pub fn alpha() -> i32 {\n    let a = 1;\n    let b = 2;\n    a + b\n}\n",
    )
    .unwrap();
    run_git(&["add", "-A"]);
    run_git(&["commit", "-m", "initial"]);

    run_git(&["mv", "ren_a.rs", "ren_b.rs"]);
    fs::write(
        git_repo.join("ren_b.rs"),
        "pub fn alpha() -> i32 {\n    let a = 1;\n    let b = 3;\n    a + b\n}\n",
    )
    .unwrap();
    run_git(&["add", "-A"]);
    run_git(&["commit", "-m", "rename"]);

    // 修改 ~1.92 MiB 已跟踪文件，实测经 git cat-file blob 读取旧侧 + 读工作树 + imara-diff
    let modified_big = near_content.replacen("0123456789abcdef", "MODIFIED_HEADER_", 1);
    fs::write(git_repo.join("big.txt"), &modified_big).unwrap();
    let repos = git::discover_workspace_repos(&git_repo.to_string_lossy(), &[]);
    let status = git::inspect_repo(&repos[0]);
    let big_cf = status
        .files
        .iter()
        .find(|f| f.path == "big.txt")
        .expect("big.txt modified");
    let t_git_big = Instant::now();
    let big_diff = git::read_review_change(big_cf, false).unwrap();
    let big_computed = compute_diff(&big_diff.old_text, &big_diff.new_text, WhitespaceMode::None);
    let git_near_2mib_diff_ms = t_git_big.elapsed().as_secs_f64() * 1000.0;
    assert!(big_diff.can_compare_text() && !big_diff.old_text.is_empty());
    assert_eq!(big_computed.hunks.len(), 1);

    // 实测受检原子保存延迟
    let t_save = Instant::now();
    let save_out = git::save_review_text_in_repo(
        &repos[0].id,
        Some(&git_repo),
        "big.txt",
        &git_repo.join("big.txt"),
        &big_diff.version,
        big_diff.line_ending,
        &big_diff.new_text,
        None,
    );
    let checked_save_ms = t_save.elapsed().as_secs_f64() * 1000.0;
    assert!(matches!(save_out, git::SaveReviewOutcome::Saved { .. }));

    // 实测历史重命名提交 Diff 延迟
    let commits = git::repo_log(&repos[0], 5).unwrap();
    let cfiles = git::commit_files(&repos[0].path, &commits[0].hash).unwrap();
    let t_ren = Instant::now();
    let ren_diff = git::read_commit_diff(&commits[0], &cfiles[0]).unwrap();
    let git_rename_diff_ms = t_ren.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(ren_diff.old_path, "ren_a.rs");
    assert_eq!(ren_diff.new_path, "ren_b.rs");

    let rss_final = current_max_rss_kib();
    eprintln!(
        "PERF_SUMMARY short_20k_ms={short_ms:.2} long_12k_ms={long_ms:.2} near_2mib_read_ms={near_read_ms:.2} over_2mib_guard_ms={over_ms:.2} switch50_ms={switch50_ms:.2} git_near_2mib_diff_ms={git_near_2mib_diff_ms:.2} checked_save_2mib_ms={checked_save_ms:.2} git_rename_diff_ms={git_rename_diff_ms:.2} rss_before_kib={rss_before} rss_after_kib={rss_after} rss_final_kib={rss_final}"
    );
    let _ = fs::remove_dir_all(&tmp);
}
