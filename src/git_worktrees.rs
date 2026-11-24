//! git worktree management: list, create, remove, prune.
//!
//! Built strictly on top of `crate::git::run_git_string` and
//! `crate::git_index::index_git`; no extra dependencies, no UI.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::git::run_git_string;
use crate::git_index::index_git;

/// A single worktree entry as reported by `git worktree list --porcelain -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Filesystem path of the worktree.
    pub path: PathBuf,
    /// HEAD commit of the worktree, when Git reports one.
    pub hash: Option<String>,
    /// Branch ref (`refs/heads/...`) when the worktree is on a branch.
    pub branch: Option<String>,
    /// Worktree is locked by Git (`locked` / `locked <reason>`).
    pub locked: bool,
    /// Worktree has a detached HEAD.
    pub detached: bool,
    /// Worktree is prunable (its directory is gone or metadata is stale).
    pub prunable: bool,
}

/// List all worktrees registered for the repository at `repo`.
///
/// Parses `git worktree list --porcelain -z`. The `-z` form separates records
/// with NUL bytes and does not quote paths, so paths containing spaces or
/// non-ASCII/Unicode characters survive untouched. The optional ` <reason>`
/// suffix after `locked` is tolerated; blank lines between records are skipped.
pub fn list(repo: &Path) -> Result<Vec<Worktree>> {
    let out = run_git_string(repo, &["worktree", "list", "--porcelain", "-z"])
        .context("git worktree list --porcelain -z")?;

    let mut worktrees: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;

    for field in out.split('\0') {
        if field.is_empty() {
            // Empty field terminates a record (and the trailing NUL).
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            continue;
        }

        // A record begins with `worktree <path>`; every other line modifies
        // the current record.
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(Worktree {
                path: PathBuf::from(path),
                hash: None,
                branch: None,
                locked: false,
                detached: false,
                prunable: false,
            });
            continue;
        }

        let Some(wt) = current.as_mut() else {
            continue;
        };

        match field {
            "detached" => wt.detached = true,
            _ if field == "prunable" || field.starts_with("prunable ") => wt.prunable = true,
            "bare" => { /* primary/bare marker; no dedicated field */ }
            _ if field.starts_with("locked") => wt.locked = true,
            _ => {
                if let Some(hash) = field.strip_prefix("HEAD ") {
                    wt.hash = Some(hash.to_string());
                } else if let Some(branch) = field.strip_prefix("branch ") {
                    wt.branch = Some(branch.to_string());
                }
            }
        }
    }

    if let Some(wt) = current.take() {
        worktrees.push(wt);
    }

    Ok(worktrees)
}

/// Create a new linked worktree at `path` checked out on `branch`.
///
/// * `branch` MUST name an existing local branch. It is validated as an exact
///   full ref (`refs/heads/<branch>`); a bare name that resolves to anything
///   else is rejected before Git runs.
/// * `path` MUST not already exist on disk.
/// * If the branch is already checked out in another worktree, Git's native
///   refusal is preserved verbatim (no `--force`, no fallback).
pub fn create(repo: &Path, path: &Path, branch: &str) -> Result<()> {
    // Reject an empty branch up front rather than letting Git interpret "".
    if branch.is_empty() || branch.starts_with('-') {
        bail!("branch name must not be empty");
    }

    let full_ref = if branch.starts_with("refs/heads/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    };

    let short = full_ref
        .strip_prefix("refs/heads/")
        .context("请选择本地分支")?;
    ensure!(!short.starts_with('-'), "无效分支名");
    index_git(
        repo,
        &["show-ref", "--verify", "--quiet", &full_ref],
        None,
        None,
    )
    .context("所选本地分支不存在")?;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    };
    match std::fs::symlink_metadata(&path) {
        Ok(_) => bail!("工作树目录已存在：{}", path.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let display = path.to_str().context("工作树路径必须是 UTF-8")?;
    run_git_string(repo, &["worktree", "add", "--", display, short])?;
    Ok(())
}

/// Remove the linked worktree registered at exactly `path`.
///
/// * Only a registered linked worktree whose path matches `path` exactly may be
///   removed; the primary worktree is refused.
/// * Dirty worktrees are rejected by Git's default behavior unless `force` is
///   explicitly set, in which case `--force` is forwarded.
pub fn remove(repo: &Path, path: &Path, force: bool) -> Result<()> {
    // Determine whether `path` is the primary worktree and whether it is a
    // registered linked worktree at all.
    let worktrees = list(repo)?;

    let primary = worktrees
        .first()
        .map(|wt| canonical_or_self(&wt.path))
        .context("git reported no worktrees")?;
    let target = canonical_or_self(path);

    if target == primary {
        bail!("refusing to remove primary worktree: {}", path.display());
    }

    let registered = worktrees
        .iter()
        .skip(1)
        .any(|wt| canonical_or_self(&wt.path) == target);
    if !registered {
        bail!("not a registered linked worktree: {}", path.display());
    }

    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let path_str = path.to_string_lossy().to_string();
    args.extend(["--", &path_str]);

    run_git_string(repo, &args)?;

    Ok(())
}

/// Prune stale worktree administrative metadata.
///
/// Wraps `git worktree prune`, which only removes metadata for worktrees whose
/// directories are gone; it does not touch live worktrees or their contents.
pub fn prune(repo: &Path) -> Result<()> {
    run_git_string(repo, &["worktree", "prune"]).context("git worktree prune")?;
    Ok(())
}

/// Canonicalize when possible so path comparisons are stable; fall back to the
/// original path when the target no longer exists on disk.
fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::process::Command;

    /// Real fixture: create a repo, a branch, and a worktree, then assert that
    /// creating a second worktree on the already-checked-out branch is refused
    /// by Git natively, and that the existing worktree's index and working tree
    /// are left untouched.
    #[test]
    fn create_refuses_checked_out_branch_and_preserves_index_and_worktree() {
        let tmp = std::env::temp_dir().join(format!(
            "omp_git_worktrees_test_{}_{}",
            std::process::id(),
            unique_suffix()
        ));
        let repo = tmp.join("repo");
        let wt = tmp.join("wt");
        let wt2 = tmp.join("wt2");
        fs::create_dir_all(&repo).expect("mkdir repo");

        let run = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .expect("spawn git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            out
        };

        run(&["init", "-q"]);
        // Deterministic identity for commits.
        run(&["config", "user.email", "test@example.invalid"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "commit.gpgsign", "false"]);

        fs::write(repo.join("README"), "hello\n").expect("write README");
        run(&["add", "README"]);
        run(&["commit", "-q", "-m", "init"]);
        run(&["branch", "feature"]);

        // Create the first linked worktree on `feature`.
        create(&repo, &wt, "feature").expect("first worktree create");

        // Snapshot the existing worktree's index and working tree.
        let index_path = repo.join(".git/worktrees/wt/index");
        let index_before = fs::read(&index_path).expect("read worktree index");
        let readme_before = fs::read(wt.join("README")).expect("read worktree README");

        // A second worktree on the same branch MUST be refused natively.
        assert!(
            create(&repo, &wt2, "feature").is_err(),
            "duplicate checkout must fail"
        );

        // Destination was not created.
        assert!(
            !wt2.exists(),
            "refused create must not leave a worktree dir"
        );

        // Existing index and working tree are byte-for-byte preserved.
        let index_after = fs::read(&index_path).expect("re-read worktree index");
        let readme_after = fs::read(wt.join("README")).expect("re-read worktree README");
        assert_eq!(
            index_before, index_after,
            "existing index must be unchanged"
        );
        assert_eq!(
            readme_before, readme_after,
            "working tree must be unchanged"
        );

        // list() reports both entries (primary + linked).
        let listed = list(&repo).expect("list worktrees");
        assert_eq!(listed.len(), 2, "expected primary and linked worktree");
        assert!(
            listed
                .iter()
                .any(|w| w.branch.as_deref() == Some("refs/heads/feature")
                    && canonical_or_self(&w.path) == canonical_or_self(&wt)),
            "feature should remain checked out in the original linked worktree"
        );

        // Prune is a no-op for live worktrees.
        prune(&repo).expect("prune live worktrees");
        assert!(wt.exists(), "prune must not remove a live worktree");

        let _ = fs::remove_dir_all(&tmp);
    }

    fn unique_suffix() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }
}
