//! Worktree-aware, non-interactive Git history commands.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::git::{BoundedGitOutput, MAX_REVIEW_BYTES, run_git_limited_with_env, run_git_string};
use crate::git_index::oid_ok;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
    Keep,
}

impl ResetMode {
    fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
            ResetMode::Keep => "--keep",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoOperation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl RepoOperation {
    fn command(self) -> &'static str {
        match self {
            RepoOperation::Merge => "merge",
            RepoOperation::Rebase => "rebase",
            RepoOperation::CherryPick => "cherry-pick",
            RepoOperation::Revert => "revert",
        }
    }

    fn label(self) -> &'static str {
        match self {
            RepoOperation::Merge => "merge",
            RepoOperation::Rebase => "rebase",
            RepoOperation::CherryPick => "cherry-pick",
            RepoOperation::Revert => "revert",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflogEntry {
    pub oid: String,
    pub selector: String,
    pub subject: String,
}

fn run_local(repo: &Path, args: &[&str], what: &str) -> Result<Vec<u8>> {
    let env = [
        ("GIT_EDITOR", "true"),
        ("GIT_SEQUENCE_EDITOR", "true"),
        ("GIT_MERGE_AUTOEDIT", "no"),
    ];
    match run_git_limited_with_env(repo, args, MAX_REVIEW_BYTES, None, None, &env)? {
        BoundedGitOutput::Ok(bytes) => Ok(bytes),
        BoundedGitOutput::TooLarge => bail!("Git {what} 输出超过大小上限"),
        BoundedGitOutput::Failed { stderr, exit } => bail!("Git {what} 失败（{exit:?}）：{stderr}"),
    }
}

fn require_oid(oid: &str) -> Result<&str> {
    if !oid_ok(oid) {
        bail!("not a validated object id: {oid:?}");
    }
    Ok(oid)
}

/// Returns the worktree-aware absolute Git directory, including linked worktrees.
fn absolute_git_dir(repo: &Path) -> Result<PathBuf> {
    let raw = run_git_string(repo, &["rev-parse", "--absolute-git-dir"])
        .context("locating the Git directory")?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("git rev-parse --absolute-git-dir produced no path");
    }
    let dir = PathBuf::from(trimmed);
    if !dir.is_absolute() {
        bail!(
            "git returned a non-absolute Git directory: {}",
            dir.display()
        );
    }
    Ok(dir)
}

fn state_present(git_dir: &Path, name: &str) -> bool {
    std::fs::symlink_metadata(git_dir.join(name)).is_ok()
}

/// Resolves `input` to the object id of exactly one commit.
///
/// `--end-of-options` guarantees that a caller-supplied string can never be
/// interpreted as a Git option, and the result is validated as an object id.
pub fn resolve_commit(repo: &Path, input: &str) -> Result<String> {
    if input.is_empty() {
        bail!("empty commit specification");
    }
    if input.contains('\0') {
        bail!("commit specification contains a NUL byte");
    }

    let spec = format!("{input}^{{commit}}");
    let raw = run_git_string(
        repo,
        &["rev-parse", "--verify", "--end-of-options", spec.as_str()],
    )
    .with_context(|| format!("resolving commit {input:?}"))?;

    let oid = raw.trim();
    if !oid_ok(oid) {
        bail!("git resolved {input:?} to a non-commit value {oid:?}");
    }
    Ok(oid.to_string())
}

/// Reads `HEAD`'s reflog, newest first, as NUL-separated fields.
pub fn reflog(repo: &Path, limit: usize) -> Result<Vec<ReflogEntry>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let limit_arg = limit.min(1000).to_string();
    let output = run_local(
        repo,
        &[
            "reflog",
            "show",
            "-z",
            "--format=%H%x00%gD%x00%gs",
            "-n",
            limit_arg.as_str(),
            "HEAD",
        ],
        "reflog",
    )?;

    let mut fields: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    if fields.last().is_some_and(|field| field.is_empty()) {
        fields.pop();
    }
    if fields.len() % 3 != 0 {
        bail!(
            "git reflog returned {} NUL-separated fields, expected a multiple of three",
            fields.len()
        );
    }

    let mut entries = Vec::with_capacity(fields.len() / 3);
    for record in fields.chunks(3) {
        let oid = String::from_utf8_lossy(record[0]).into_owned();
        if !oid_ok(&oid) {
            bail!("git reflog returned an invalid object id {oid:?}");
        }
        let selector = String::from_utf8_lossy(record[1]).into_owned();
        let subject = String::from_utf8_lossy(record[2])
            .trim_end_matches('\n')
            .to_string();
        entries.push(ReflogEntry {
            oid,
            selector,
            subject,
        });
    }
    Ok(entries)
}

/// Produces a diff between two resolved commits without external diff drivers
/// or text conversion filters.
pub fn ref_diff(repo: &Path, base: &str, target: &str, three_dot: bool) -> Result<String> {
    let base_oid = resolve_commit(repo, base)?;
    let target_oid = resolve_commit(repo, target)?;
    let range = format!("{base_oid}...{target_oid}");

    let mut args: Vec<&str> = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color"];
    if three_dot {
        args.push(range.as_str());
    } else {
        args.push(base_oid.as_str());
        args.push(target_oid.as_str());
    }

    args.push("--");
    let patch = run_local(repo, &args, "diff")?;
    Ok(String::from_utf8_lossy(&patch).into_owned())
}

pub fn reset(repo: &Path, oid: &str, mode: ResetMode) -> Result<()> {
    let oid = require_oid(oid)?;
    run_local(repo, &["reset", mode.flag(), oid, "--"], "reset")?;
    Ok(())
}

pub fn checkout_detached(repo: &Path, oid: &str) -> Result<()> {
    let oid = require_oid(oid)?;
    run_local(
        repo,
        &["checkout", "--detach", oid, "--"],
        "checkout --detach",
    )?;
    Ok(())
}

pub fn cherry_pick(repo: &Path, oid: &str) -> Result<()> {
    let oid = require_oid(oid)?;
    run_local(repo, &["cherry-pick", oid], "cherry-pick")?;
    Ok(())
}

pub fn revert(repo: &Path, oid: &str, mainline: Option<u32>) -> Result<()> {
    let oid = require_oid(oid)?;

    let mut owned: Vec<String> = vec!["revert".to_string(), "--no-edit".to_string()];
    if let Some(mainline) = mainline {
        if mainline == 0 {
            bail!("revert mainline must be greater than zero");
        }
        owned.push("-m".to_string());
        owned.push(mainline.to_string());
    }
    owned.push(oid.to_string());

    let args: Vec<&str> = owned.iter().map(|arg| arg.as_str()).collect();
    run_local(repo, &args, "revert")?;
    Ok(())
}

pub fn rebase(repo: &Path, upstream: &str) -> Result<()> {
    let upstream_oid = resolve_commit(repo, upstream)?;
    run_local(repo, &["rebase", upstream_oid.as_str()], "rebase")?;
    Ok(())
}

/// Reports the operation currently recorded in this worktree's Git directory.
pub fn active_operation(repo: &Path) -> Result<Option<RepoOperation>> {
    let git_dir = absolute_git_dir(repo)?;

    if state_present(&git_dir, "rebase-merge") || state_present(&git_dir, "rebase-apply") {
        return Ok(Some(RepoOperation::Rebase));
    }
    if state_present(&git_dir, "MERGE_HEAD") {
        return Ok(Some(RepoOperation::Merge));
    }
    if state_present(&git_dir, "CHERRY_PICK_HEAD") {
        return Ok(Some(RepoOperation::CherryPick));
    }
    if state_present(&git_dir, "REVERT_HEAD") {
        return Ok(Some(RepoOperation::Revert));
    }
    Ok(None)
}

/// Aborts or continues the operation that is actually in progress.
///
/// The reported state is verified against `expected` first; the operation is
/// never silently redirected at a different operation, and a failed Git
/// command leaves whatever state Git recorded untouched.
pub fn control_operation(repo: &Path, expected: RepoOperation, abort: bool) -> Result<()> {
    match active_operation(repo)? {
        None => bail!(
            "cannot {} {}: no operation is in progress",
            if abort { "abort" } else { "continue" },
            expected.label()
        ),
        Some(actual) if actual != expected => bail!(
            "expected to control a {} operation, but a {} operation is in progress",
            expected.label(),
            actual.label()
        ),
        Some(_) => {}
    }

    run_local(
        repo,
        &[
            expected.command(),
            if abort { "--abort" } else { "--continue" },
        ],
        expected.command(),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "omp-git-history-{}-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
                stamp
            ));
            fs::create_dir_all(&path).expect("create fixture directory");
            let fixture = Fixture { path };
            fixture.git(&["init", "-q"]);
            fixture.git(&["config", "user.name", "Fixture"]);
            fixture.git(&["config", "user.email", "fixture@example.invalid"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
            fixture
        }

        fn git(&self, args: &[&str]) -> String {
            let output =
                run_local(&self.path, args, "test fixture").expect("fixture git command failed");
            String::from_utf8_lossy(&output).trim().to_string()
        }

        fn write(&self, name: &str, contents: &str) {
            fs::write(self.path.join(name), contents).expect("write fixture file");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn cherry_pick_records_true_tree_and_parent_or_reports_conflict() {
        let fixture = Fixture::new();

        fixture.write("shared.txt", "base\n");
        fixture.git(&["add", "shared.txt"]);
        fixture.git(&["commit", "-q", "-m", "base"]);
        let base = fixture.git(&["rev-parse", "HEAD"]);
        let main_branch = fixture.git(&["symbolic-ref", "--short", "HEAD"]);

        fixture.git(&["checkout", "-q", "-b", "feature"]);
        fixture.write("feature.txt", "feature\n");
        fixture.git(&["add", "feature.txt"]);
        fixture.git(&["commit", "-q", "-m", "feature"]);
        let feature = fixture.git(&["rev-parse", "HEAD"]);

        fixture.git(&["checkout", "-q", main_branch.as_str()]);
        fixture.write("main.txt", "main\n");
        fixture.git(&["add", "main.txt"]);
        fixture.git(&["commit", "-q", "-m", "main"]);
        let main = fixture.git(&["rev-parse", "HEAD"]);

        assert_eq!(resolve_commit(&fixture.path, &base).unwrap(), base);
        assert!(resolve_commit(&fixture.path, "HEAD:shared.txt").is_err());
        assert!(resolve_commit(&fixture.path, "--help").is_err());

        let recent = reflog(&fixture.path, 2).unwrap();
        assert_eq!(recent.len(), 2);
        assert!(recent.iter().all(|entry| oid_ok(&entry.oid)));
        assert!(recent.iter().all(|entry| !entry.selector.is_empty()));

        cherry_pick(&fixture.path, &feature).unwrap();

        let picked = fixture.git(&["rev-parse", "HEAD"]);
        let parent = fixture.git(&["rev-parse", "HEAD^"]);
        assert_ne!(picked, feature);
        assert_eq!(parent, main);

        let picked_tree = fixture.git(&["rev-parse", "HEAD^{tree}"]);
        let parent_tree_spec = format!("{parent}^{{tree}}");
        let parent_tree = fixture.git(&["rev-parse", parent_tree_spec.as_str()]);
        assert_ne!(picked_tree, parent_tree);
        for path in ["shared.txt", "main.txt", "feature.txt"] {
            assert_eq!(
                fixture.git(&["cat-file", "-t", &format!("HEAD:{path}")]),
                "blob"
            );
        }

        fixture.git(&["checkout", "-q", "-b", "conflicting"]);
        fixture.write("shared.txt", "conflict side\n");
        fixture.git(&["add", "shared.txt"]);
        fixture.git(&["commit", "-q", "-m", "conflicting shared change"]);
        let conflicting = fixture.git(&["rev-parse", "HEAD"]);

        fixture.git(&["checkout", "-q", main_branch.as_str()]);
        fixture.write("shared.txt", "main side\n");
        fixture.git(&["add", "shared.txt"]);
        fixture.git(&["commit", "-q", "-m", "main shared change"]);

        let result = cherry_pick(&fixture.path, &conflicting);
        assert!(result.is_err(), "conflicting cherry-pick must fail");
        assert_eq!(
            active_operation(&fixture.path).unwrap(),
            Some(RepoOperation::CherryPick)
        );

        control_operation(&fixture.path, RepoOperation::CherryPick, true).unwrap();
        assert_eq!(active_operation(&fixture.path).unwrap(), None);
    }
}
