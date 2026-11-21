use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::git::{BoundedGitOutput, MAX_REVIEW_BYTES, run_git_limited_with_index};
use crate::git_index::{
    IndexEntry, IndexTransaction, hash_text, index_git, literal_file, oid_ok, put_entry,
    read_index_entry, regular_mode, tree_file,
};

#[derive(Debug)]
pub(crate) enum CommitSelection {
    All {
        paths: Vec<String>,
    },
    Partial {
        path: String,
        expected: Option<IndexEntry>,
        bytes: Vec<u8>,
        mode: String,
    },
}

pub(crate) struct CommitRequest {
    pub expected_head: Option<String>,
    pub selections: Vec<CommitSelection>,
    pub message: String,
    pub amend: bool,
}

#[derive(Debug)]
pub(crate) struct CommitOutcome {
    pub hash: String,
    pub index_warning: Option<String>,
}

fn text(repo: &Path, args: &[&str], index: Option<&Path>) -> Result<String> {
    Ok(String::from_utf8(index_git(repo, args, None, index)?)?
        .trim()
        .to_owned())
}

pub(crate) fn head_oid(repo: &Path) -> Result<Option<String>> {
    match run_git_limited_with_index(
        repo,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
        128,
        None,
        None,
    )? {
        BoundedGitOutput::Ok(bytes) => {
            let oid = String::from_utf8(bytes)?.trim().to_owned();
            ensure!(oid_ok(&oid), "无法识别 HEAD 对象 ID");
            Ok(Some(oid))
        }
        BoundedGitOutput::Failed {
            exit: Some(1),
            stderr,
        } if stderr.is_empty() => {
            let branch = text(repo, &["symbolic-ref", "--quiet", "HEAD"], None)?;
            let listed = text(
                repo,
                &["for-each-ref", "--format=%(refname)", &branch],
                None,
            )?;
            ensure!(
                !listed.lines().any(|reference| reference == branch),
                "HEAD 指向的提交不可用"
            );
            Ok(None)
        }
        BoundedGitOutput::Failed { stderr, .. } => bail!("无法读取 HEAD：{stderr}"),
        BoundedGitOutput::TooLarge => bail!("无法识别 HEAD"),
    }
}

fn reject_pending_operations(repo: &Path) -> Result<()> {
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "REBASE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        let args = ["rev-parse", "--path-format=absolute", "--git-path", name];
        let path = String::from_utf8(index_git(repo, &args, None, None)?)?;
        if Path::new(path.strip_suffix('\n').unwrap_or(&path)).try_exists()? {
            bail!("请先结束当前的 Git 操作（{name}）");
        }
    }
    Ok(())
}

/// Build a commit in a separate index and reconcile just the selected real-index entries.
pub(crate) fn commit_selected(repo: &Path, request: CommitRequest) -> Result<CommitOutcome> {
    let CommitRequest {
        expected_head,
        selections,
        message,
        amend,
    } = request;
    ensure!(
        !message.trim().is_empty() && message.len() <= MAX_REVIEW_BYTES,
        "请输入有效的提交信息"
    );
    ensure!(!amend || expected_head.is_some(), "仓库没有可修订的提交");
    ensure!(amend || !selections.is_empty(), "没有选择要提交的变更");
    let tx = IndexTransaction::begin(repo)?;
    reject_pending_operations(repo)?;
    ensure!(
        head_oid(repo)? == expected_head,
        "HEAD 已变化，请刷新后重试"
    );
    ensure!(
        index_git(repo, &["ls-files", "-u", "-z"], None, None)?.is_empty(),
        "索引含有未解决的冲突"
    );
    let head_branch = text(repo, &["rev-parse", "--symbolic-full-name", "HEAD"], None).ok();

    let alternate = tx.fresh_index()?;
    let index = Some(alternate.path());
    match &expected_head {
        Some(head) => {
            index_git(repo, &["read-tree", head], None, index)?;
        }
        None => {
            index_git(repo, &["read-tree", "--empty"], None, index)?;
        }
    }
    let original_tree = text(repo, &["write-tree"], index)?;
    let mut selected_paths = BTreeSet::new();
    for selection in selections {
        match selection {
            CommitSelection::All { paths } => {
                for path in paths {
                    literal_file(&path)?;
                    ensure!(
                        selected_paths.insert(path.clone()),
                        "同一文件被重复选中：{path}"
                    );
                    // If a tracked file was added to the index and then removed from the
                    // worktree, Git cannot match it in the HEAD-based alternate index.
                    let args = ["--literal-pathspecs", "add", "-A", "--", path.as_str()];
                    let in_head = tree_file(repo, &original_tree, &path)?.is_some();
                    if in_head || repo.join(&path).symlink_metadata().is_ok() {
                        index_git(repo, &args, None, index)?;
                    }
                }
            }
            CommitSelection::Partial {
                path,
                expected,
                bytes,
                mode,
            } => {
                literal_file(&path)?;
                ensure!(
                    selected_paths.insert(path.clone()),
                    "同一文件被重复选中：{path}"
                );
                ensure!(regular_mode(&mode), "该文件类型不支持按块提交：{path}");
                let actual = read_index_entry(repo, &path)?;
                ensure!(actual == expected, "暂存区已变化，请重新选择：{path}");
                ensure!(
                    tree_file(repo, &original_tree, &path)? == actual,
                    "文件含有已暂存的其他变更：{path}"
                );
                let entry = IndexEntry {
                    oid: hash_text(repo, bytes, None)?,
                    mode,
                };
                put_entry(repo, alternate.path(), &path, Some(&entry), None)?;
            }
        }
    }

    let proposed_tree = text(repo, &["write-tree"], index)?;
    ensure!(
        amend || proposed_tree != original_tree,
        "选中的文件没有可提交内容"
    );
    for path in &selected_paths {
        let next = tree_file(repo, &proposed_tree, path)?;
        let input = match next {
            Some(entry) => format!("{} {}\t{path}\0", entry.mode, entry.oid),
            None => format!("0 {}\t{path}\0", "0".repeat(proposed_tree.len())),
        };
        index_git(
            repo,
            &["update-index", "-z", "--index-info"],
            Some(input.into_bytes()),
            Some(tx.index_path()),
        )?;
    }
    ensure!(
        head_oid(repo)? == expected_head
            && text(repo, &["rev-parse", "--symbolic-full-name", "HEAD"], None).ok() == head_branch,
        "准备提交时 HEAD 已变化"
    );

    let mut args = vec!["commit", "--cleanup=verbatim", "-F", "-"];
    if amend {
        args.push("--amend");
    }
    let command = index_git(repo, &args, Some(message.into_bytes()), index);
    let new_head = head_oid(repo).context("提交命令结束，无法确认 HEAD，请先核对提交记录")?;
    if new_head == expected_head {
        if let Err(err) = &command {
            bail!("{err}");
        }
        // Two identical amendments in a single timestamp may produce the same object ID.
        ensure!(amend && selected_paths.is_empty(), "提交未生成新对象");
    }
    let hash = new_head.context("提交命令结束，HEAD 没有提交")?;
    let index_warning = if let Err(err) = command {
        Some(format!(
            "提交命令返回错误且 HEAD 已改变，请核对提交记录：{err}"
        ))
    } else if text(repo, &["rev-parse", &format!("{hash}^{{tree}}")], None)
        .ok()
        .as_deref()
        != Some(&proposed_tree)
    {
        Some("提交已生成，实际提交树与预期不符，暂存区未更新".to_owned())
    } else if selected_paths.is_empty() {
        None
    } else {
        tx.install()
            .err()
            .map(|err| format!("提交已生成，更新暂存区失败：{err}"))
    };
    Ok(CommitOutcome {
        hash,
        index_warning,
    })
}

pub(crate) fn undo_last_commit(repo: &Path, expected_head: &str) -> Result<()> {
    ensure!(oid_ok(expected_head), "无效的提交对象 ID");
    reject_pending_operations(repo)?;
    let branch = text(repo, &["symbolic-ref", "--quiet", "HEAD"], None)
        .context("分离 HEAD 无法撤销最近提交")?;
    ensure!(
        head_oid(repo)?.as_deref() == Some(expected_head),
        "HEAD 已变化，请刷新后重试"
    );
    let parents = text(
        repo,
        &["rev-list", "--parents", "-n", "1", expected_head],
        None,
    )?;
    let parts: Vec<_> = parents.split_whitespace().collect();
    ensure!(parts.len() == 2, "仅支持撤销具有一个父提交的最近提交");
    index_git(
        repo,
        &[
            "update-ref",
            "-m",
            "undo last commit",
            &branch,
            parts[1],
            expected_head,
        ],
        None,
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::{Command, Output};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempRepo {
        root: PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let counter = NEXT.fetch_add(1, Ordering::SeqCst);
            let root = std::env::temp_dir().join(format!(
                "nemorous-git-commit-test-{}-{tag}-{counter}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            let repo = Self { root };
            repo.git(&["init", "-q"]);
            repo.git(&["config", "user.email", "test@example.com"]);
            repo.git(&["config", "user.name", "Test"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }

        fn path(&self) -> &Path {
            &self.root
        }

        fn output(&self, args: &[&str]) -> Output {
            Command::new("git")
                .current_dir(&self.root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .args(args)
                .output()
                .unwrap()
        }

        fn git(&self, args: &[&str]) -> String {
            let output = self.output(args);
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, bytes).unwrap();
        }

        fn read(&self, relative: &str) -> Vec<u8> {
            fs::read(self.root.join(relative)).unwrap()
        }

        fn head(&self) -> String {
            self.git(&["rev-parse", "HEAD"]).trim().to_string()
        }

        fn stage(&self, relative: &str) {
            self.git(&["add", "--", relative]);
        }

        fn commit_all(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }

        fn show_head(&self, relative: &str) -> String {
            self.git(&["show", &format!("HEAD:{relative}")])
        }

        fn staged(&self, relative: &str) -> String {
            self.git(&["show", &format!(":{relative}")])
        }

        fn cached_names(&self) -> Vec<String> {
            self.git(&["diff", "--cached", "--name-only"])
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn partial_commit_keeps_unrelated_staged_and_unstaged_bytes() {
        let repo = TempRepo::new("partial");
        repo.write("a.txt", b"old\n");
        repo.write("n.txt", b"n\n");
        repo.commit_all("init");
        let head = repo.head();
        let entry = read_index_entry(repo.path(), "a.txt").unwrap().unwrap();

        repo.write("a.txt", b"new\n");
        repo.write("b.txt", b"staged\n");
        repo.stage("b.txt");
        repo.write("b.txt", b"staged-worktree\n");
        repo.write("n.txt", b"n2\n");

        let outcome = commit_selected(
            repo.path(),
            CommitRequest {
                expected_head: Some(head.clone()),
                selections: vec![CommitSelection::Partial {
                    path: "a.txt".into(),
                    expected: Some(IndexEntry {
                        oid: entry.oid.clone(),
                        mode: entry.mode.clone(),
                    }),
                    bytes: b"new\n".to_vec(),
                    mode: "100644".into(),
                }],
                message: "commit a only".into(),
                amend: false,
            },
        )
        .unwrap();

        assert_eq!(outcome.hash, repo.head());
        assert!(outcome.index_warning.is_none());
        assert_eq!(repo.show_head("a.txt"), "new\n");
        assert_eq!(repo.read("a.txt"), b"new\n");
        assert_eq!(repo.staged("b.txt"), "staged\n");
        assert_eq!(repo.read("b.txt"), b"staged-worktree\n");
        assert_eq!(repo.read("n.txt"), b"n2\n");
        assert_eq!(repo.cached_names(), vec!["b.txt".to_string()]);

        let installed = read_index_entry(repo.path(), "a.txt").unwrap().unwrap();
        assert_eq!(installed.oid, repo.git(&["rev-parse", "HEAD:a.txt"]).trim());
    }

    #[cfg(unix)]
    #[test]
    fn hook_failure_leaves_head_and_index_untouched() {
        use std::os::unix::fs::PermissionsExt;

        let repo = TempRepo::new("hookfail");
        repo.write("a.txt", b"old\n");
        repo.commit_all("init");
        let head = repo.head();
        let entry = read_index_entry(repo.path(), "a.txt").unwrap().unwrap();

        let hook = repo.path().join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\necho blocked >&2\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        repo.write("a.txt", b"new\n");
        repo.write("b.txt", b"b\n");
        repo.stage("b.txt");

        let error = commit_selected(
            repo.path(),
            CommitRequest {
                expected_head: Some(head.clone()),
                selections: vec![CommitSelection::Partial {
                    path: "a.txt".into(),
                    expected: Some(IndexEntry {
                        oid: entry.oid.clone(),
                        mode: entry.mode.clone(),
                    }),
                    bytes: b"new\n".to_vec(),
                    mode: "100644".into(),
                }],
                message: "blocked".into(),
                amend: false,
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("blocked"));
        assert_eq!(repo.head(), head);
        let current = read_index_entry(repo.path(), "a.txt").unwrap().unwrap();
        assert_eq!(current.oid, entry.oid);
        assert_eq!(current.mode, entry.mode);
        assert_eq!(repo.cached_names(), vec!["b.txt".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn hook_rewritten_tree_reports_warning_without_installing_index() {
        use std::os::unix::fs::PermissionsExt;

        let repo = TempRepo::new("hookrewrite");
        repo.write("a.txt", b"old\n");
        repo.commit_all("init");
        let head = repo.head();
        let entry = read_index_entry(repo.path(), "a.txt").unwrap().unwrap();

        let script = concat!(
            "#!/bin/sh\n",
            "set -e\n",
            "blob=$(printf 'hook\\n' | git hash-object -w --stdin)\n",
            "tree=$(printf '100644 blob %s\\thook.txt\\n' \"$blob\" | git mktree)\n",
            "new=$(printf 'hook rewrite\\n' | git commit-tree \"$tree\" -p \"$(git rev-parse HEAD^)\")\n",
            "git update-ref HEAD \"$new\"\n"
        );
        let hook = repo.path().join(".git/hooks/post-commit");
        fs::write(&hook, script).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

        repo.write("a.txt", b"new\n");

        let outcome = commit_selected(
            repo.path(),
            CommitRequest {
                expected_head: Some(head.clone()),
                selections: vec![CommitSelection::Partial {
                    path: "a.txt".into(),
                    expected: Some(IndexEntry {
                        oid: entry.oid.clone(),
                        mode: entry.mode.clone(),
                    }),
                    bytes: b"new\n".to_vec(),
                    mode: "100644".into(),
                }],
                message: "rewritten".into(),
                amend: false,
            },
        )
        .unwrap();

        assert!(outcome.index_warning.is_some());
        assert_eq!(outcome.hash, repo.head());
        let untouched = read_index_entry(repo.path(), "a.txt").unwrap().unwrap();
        assert_eq!(untouched.oid, entry.oid);
        assert_eq!(repo.read("a.txt"), b"new\n");
    }

    #[test]
    fn all_selection_handles_literal_unicode_and_tab_paths() {
        let repo = TempRepo::new("literal");
        let name = "tab\there ü.txt";
        repo.write(name, b"one\n");
        repo.commit_all("init");
        let head = repo.head();

        repo.write(name, b"two\n");
        repo.write("keep.txt", b"staged\n");
        repo.stage("keep.txt");

        let outcome = commit_selected(
            repo.path(),
            CommitRequest {
                expected_head: Some(head),
                selections: vec![CommitSelection::All {
                    paths: vec![name.to_string()],
                }],
                message: "literal".into(),
                amend: false,
            },
        )
        .unwrap();

        assert!(outcome.index_warning.is_none());
        assert_eq!(repo.show_head(name), "two\n");
        assert_eq!(repo.read(name), b"two\n");
        assert_eq!(repo.cached_names(), vec!["keep.txt".to_string()]);
    }

    #[test]
    fn all_selection_commits_deletion_and_rename_paths() {
        let repo = TempRepo::new("rename");
        repo.write("old.txt", b"content\n");
        repo.write("gone.txt", b"bye\n");
        repo.commit_all("init");
        let head = repo.head();

        fs::rename(repo.path().join("old.txt"), repo.path().join("renamed.txt")).unwrap();
        fs::remove_file(repo.path().join("gone.txt")).unwrap();

        let outcome = commit_selected(
            repo.path(),
            CommitRequest {
                expected_head: Some(head),
                selections: vec![CommitSelection::All {
                    paths: vec![
                        "old.txt".to_string(),
                        "renamed.txt".to_string(),
                        "gone.txt".to_string(),
                    ],
                }],
                message: "rename and delete".into(),
                amend: false,
            },
        )
        .unwrap();

        assert!(outcome.index_warning.is_none());
        assert_eq!(repo.show_head("renamed.txt"), "content\n");
        let names = repo.git(&["ls-tree", "--name-only", "HEAD"]);
        assert!(!names.contains("old.txt"));
        assert!(!names.contains("gone.txt"));
        assert!(names.contains("renamed.txt"));
    }

    #[test]
    fn undo_moves_head_without_touching_index_or_worktree() {
        let repo = TempRepo::new("undo");
        repo.write("a.txt", b"a\n");
        repo.commit_all("first");
        let first = repo.head();

        repo.write("b.txt", b"b\n");
        repo.commit_all("second");
        let second = repo.head();

        undo_last_commit(repo.path(), &second).unwrap();
        assert_eq!(repo.head(), first);
        assert_eq!(repo.cached_names(), vec!["b.txt".to_string()]);
        assert_eq!(repo.read("b.txt"), b"b\n");

        assert!(undo_last_commit(repo.path(), &first).is_err());
        assert!(undo_last_commit(repo.path(), "0000000000000000000000000000000000000000").is_err());
        assert_eq!(repo.head(), first);
    }
}
