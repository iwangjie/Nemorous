//! Local-branch listing, exact switching/creation, and default-branch resolution.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::git::run_git_string;

const RECENT_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCandidate {
    pub name: String,
    pub current: bool,
    pub mine: bool,
    pub commit_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchTarget {
    Existing(String),
    New(String),
}

/// Lists local branches only, ordered mine -> recent (7d) -> other, newest first.
pub fn list_branches(repo: &Path) -> Result<Vec<BranchCandidate>> {
    let current = current_branch(repo);
    let user_email = git_config(repo, "user.email");
    let now = unix_now();

    let raw = run_git_string(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(committerdate:unix)%00%(authoremail)",
            "refs/heads/",
        ],
    )
    .context("listing local branches")?;

    let mut rows: Vec<(u8, BranchCandidate)> = Vec::new();
    for line in raw.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\0').collect();
        if fields.len() != 3 {
            continue;
        }
        let Some(name) = fields[0].strip_prefix("refs/heads/") else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let commit_unix = fields[1].parse::<i64>().unwrap_or(0);
        let author_email = fields[2];
        let mine = email_matches(author_email, &user_email);
        let tier = if commit_unix < now - RECENT_SECS {
            2
        } else if mine {
            0
        } else {
            1
        };

        rows.push((
            tier,
            BranchCandidate {
                name: name.to_string(),
                current: current.as_deref() == Some(name),
                mine,
                commit_unix,
            },
        ));
    }

    rows.sort_by(|(ta, a), (tb, b)| {
        ta.cmp(tb)
            .then_with(|| b.commit_unix.cmp(&a.commit_unix))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(rows.into_iter().map(|(_, branch)| branch).collect())
}

/// Switches to an exact existing local branch, or creates `New` from HEAD.
/// Never force-checkouts, never DWIMs, never overwrites an existing branch.
pub fn switch_branch(repo: &Path, target: &BranchTarget) -> Result<()> {
    match target {
        BranchTarget::Existing(name) => {
            if !validate_branch_name(name) {
                bail!("invalid branch name: {name:?}");
            }
            if current_branch(repo).as_deref() == Some(name) {
                return Ok(());
            }
            ensure_clean(repo, false)?;
            if local_branch_exists(repo, name)? {
                run_git_string(repo, &["switch", "--no-guess", name])
                    .with_context(|| format!("切换分支 {name}"))?;
            } else {
                let remote = format!("refs/remotes/origin/{name}");
                run_git_string(repo, &["show-ref", "--verify", "--quiet", &remote])
                    .with_context(|| format!("找不到本地或 origin 上的分支 {name}"))?;
                run_git_string(repo, &["switch", "--create", name, "--track", &remote])
                    .with_context(|| format!("跟踪 origin/{name}"))?;
            }
        }
        BranchTarget::New(name) => {
            if !validate_branch_name(name) {
                bail!("invalid branch name: {name:?}");
            }
            if local_branch_exists(repo, name)? {
                bail!("branch already exists: {name}");
            }
            ensure_clean(repo, false)?;
            run_git_string(repo, &["switch", "--no-guess", "-c", name])
                .with_context(|| format!("creating branch {name} from HEAD"))?;
        }
    }
    Ok(())
}

pub(crate) fn ensure_clean(repo: &Path, include_untracked: bool) -> Result<()> {
    let mode = if include_untracked { "-uall" } else { "-uno" };
    let status = run_git_string(repo, &["status", "--porcelain", mode])?;
    if !status.is_empty() {
        bail!("工作区有未提交的改动");
    }
    Ok(())
}

/// Preferred local first, then a remote's default mapped to an existing local,
/// then `main`, then `master`.
pub fn default_branch(repo: &Path, preferred: &str) -> Result<String> {
    let preferred = preferred.trim();
    if !preferred.is_empty()
        && validate_branch_name(preferred)
        && local_branch_exists(repo, preferred)?
    {
        return Ok(preferred.to_string());
    }

    if let Some(remote_default) = remote_default_branch(repo)? {
        if local_branch_exists(repo, &remote_default)? {
            return Ok(remote_default);
        }
    }

    for candidate in ["main", "master"] {
        if local_branch_exists(repo, candidate)? {
            return Ok(candidate.to_string());
        }
    }

    bail!("no default branch found in {}", repo.display())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn current_branch(repo: &Path) -> Option<String> {
    run_git_string(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn git_config(repo: &Path, key: &str) -> String {
    run_git_string(repo, &["config", "--get", key])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn email_matches(author_email: &str, configured_email: &str) -> bool {
    let configured = configured_email.trim().to_ascii_lowercase();
    if configured.is_empty() {
        return false;
    }
    let author = author_email.trim().to_ascii_lowercase();
    if author == configured {
        return true;
    }
    // `%(authoremail)` may render as `Name <email>`.
    if let (Some(open), Some(close)) = (author.find('<'), author.rfind('>')) {
        if open < close {
            return &author[open + 1..close] == configured;
        }
    }
    false
}

fn local_branch_exists(repo: &Path, name: &str) -> Result<bool> {
    if !validate_branch_name(name) {
        return Ok(false);
    }
    let out = run_git_string(
        repo,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    )
    .context("checking local branch existence")?;
    Ok(out
        .split('\n')
        .any(|line| line.strip_prefix("refs/heads/").map(str::trim) == Some(name)))
}

fn remote_default_branch(repo: &Path) -> Result<Option<String>> {
    let remotes = match run_git_string(repo, &["remote"]) {
        Ok(out) => out,
        Err(_) => return Ok(None),
    };
    for remote in remotes.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let head_ref = format!("refs/remotes/{remote}/HEAD");
        let target = match run_git_string(repo, &["symbolic-ref", "--quiet", &head_ref]) {
            Ok(out) => out.trim().to_string(),
            Err(_) => continue,
        };
        let prefix = format!("refs/remotes/{remote}/");
        let Some(branch) = target.strip_prefix(&prefix) else {
            continue;
        };
        if validate_branch_name(branch) && local_branch_exists(repo, branch)? {
            return Ok(Some(branch.to_string()));
        }
    }
    Ok(None)
}

/// Strict subset of `git check-ref-format --branch` rules: no option-like names,
/// no ambiguous revisions, no invalid component characters.
fn validate_branch_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('-') || name == "@" || name == "HEAD" {
        return false;
    }
    if name.starts_with('/') || name.ends_with('/') || name.ends_with('.') {
        return false;
    }
    if name.contains("//") || name.contains("..") || name.contains("@{") || name.contains('\\') {
        return false;
    }
    for byte in name.bytes() {
        if byte < 0x20
            || byte == 0x7f
            || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[')
        {
            return false;
        }
    }
    for component in name.split('/') {
        if component.is_empty() || component.starts_with('.') || component.ends_with(".lock") {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempRepo(PathBuf);

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run(dir: &Path, args: &[&str], envs: &[(&str, String)]) -> Output {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", dir.join("gitconfig-global"))
            .env("GIT_TERMINAL_PROMPT", "0");
        for (key, value) in envs {
            cmd.env(*key, value);
        }
        cmd.output().expect("failed to spawn git")
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = run(dir, args, &[]);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(dir: &Path, ts: i64, author: (&str, &str), message: &str) {
        let date = format!("@{ts} +0000");
        let envs = [
            ("GIT_AUTHOR_NAME", author.0.to_string()),
            ("GIT_AUTHOR_EMAIL", author.1.to_string()),
            ("GIT_COMMITTER_NAME", author.0.to_string()),
            ("GIT_COMMITTER_EMAIL", author.1.to_string()),
            ("GIT_AUTHOR_DATE", date.clone()),
            ("GIT_COMMITTER_DATE", date),
        ];
        let out = run(
            dir,
            &["commit", "-q", "--allow-empty", "-m", message],
            &envs,
        );
        assert!(
            out.status.success(),
            "commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn branch_with_commit(
        dir: &Path,
        name: &str,
        from: &str,
        ts: i64,
        author: (&str, &str),
        message: &str,
    ) {
        git(dir, &["checkout", "-q", from]);
        git(dir, &["checkout", "-q", "-b", name]);
        commit(dir, ts, author, message);
    }

    fn head(dir: &Path) -> String {
        git(dir, &["symbolic-ref", "--short", "HEAD"])
    }

    #[test]
    fn branch_listing_switching_and_defaults() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = TempRepo(
            std::env::temp_dir().join(format!("omp-git-branches-{}-{nanos}", std::process::id())),
        );
        fs::create_dir_all(&tmp.0).unwrap();
        let dir = tmp.0.as_path();

        git(dir, &["init", "-q"]);
        git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(dir, &["config", "user.name", "Alice"]);
        git(dir, &["config", "user.email", "alice@example.com"]);

        let alice = ("Alice", "alice@example.com");
        let bob = ("Bob", "bob@example.com");
        let day = 86_400;

        let tracked = dir.join("file.txt");
        fs::write(&tracked, "base\n").unwrap();
        git(dir, &["add", "file.txt"]);

        let t_main = now - 7_200;
        let t_alice_work = now - 100;
        let t_alice_old = now - 40 * day;
        let t_bob_recent = now - 3_600;
        let t_bob_old = now - 40 * day;
        commit(dir, t_main, alice, "main base");

        branch_with_commit(dir, "alice-work", "main", t_alice_work, alice, "alice work");
        branch_with_commit(dir, "alice-old", "main", t_alice_old, alice, "alice old");
        branch_with_commit(dir, "recent-bob", "main", t_bob_recent, bob, "bob recent");
        branch_with_commit(dir, "old-bob", "main", t_bob_old, bob, "bob old");
        git(dir, &["checkout", "-q", "main"]);

        let branches = list_branches(dir).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            ["alice-work", "main", "recent-bob", "alice-old", "old-bob"]
        );
        for branch in &branches {
            let expect_current = branch.name == "main";
            let expect_mine = matches!(branch.name.as_str(), "alice-work" | "alice-old" | "main");
            assert_eq!(branch.current, expect_current, "{branch:?}");
            assert_eq!(branch.mine, expect_mine, "{branch:?}");
        }
        let ts = |name: &str| {
            branches
                .iter()
                .find(|b| b.name == name)
                .unwrap()
                .commit_unix
        };
        assert_eq!(ts("main"), t_main);
        assert_eq!(ts("alice-work"), t_alice_work);
        assert_eq!(ts("alice-old"), t_alice_old);
        assert_eq!(ts("recent-bob"), t_bob_recent);
        assert_eq!(ts("old-bob"), t_bob_old);

        // Exact existing switch.
        switch_branch(dir, &BranchTarget::Existing("alice-work".into())).unwrap();
        assert_eq!(head(dir), "alice-work");
        let tip = git(dir, &["rev-parse", "HEAD"]);

        // Create-from-HEAD without overwriting anything.
        switch_branch(dir, &BranchTarget::New("feature/x".into())).unwrap();
        assert_eq!(head(dir), "feature/x");
        assert_eq!(git(dir, &["rev-parse", "HEAD"]), tip);
        assert!(switch_branch(dir, &BranchTarget::New("alice-work".into())).is_err());
        assert!(switch_branch(dir, &BranchTarget::Existing("nope".into())).is_err());

        // Option-like and ambiguous/invalid names are rejected.
        assert!(switch_branch(dir, &BranchTarget::Existing("-x".into())).is_err());
        for bad in [
            "bad name", "a..b", "a.lock", "HEAD", "/abs", "trail/", "a//b", "a@{b", "a:b", "a?b",
            "a*b", "a[b", "a\\b", "a\tb", "--orphan",
        ] {
            assert!(
                switch_branch(dir, &BranchTarget::New(bad.into())).is_err(),
                "accepted invalid branch name {bad:?}"
            );
        }
        assert_eq!(head(dir), "feature/x");

        // Dirty tree conflict: Git refuses; state stays dirty and reported.
        switch_branch(dir, &BranchTarget::Existing("main".into())).unwrap();
        git(dir, &["checkout", "-q", "-b", "conflict"]);
        fs::write(&tracked, "changed\n").unwrap();
        git(dir, &["add", "file.txt"]);
        commit(dir, now, alice, "conflict change");
        git(dir, &["checkout", "-q", "main"]);
        fs::write(&tracked, "dirty\n").unwrap();
        assert!(switch_branch(dir, &BranchTarget::Existing("conflict".into())).is_err());
        assert_eq!(head(dir), "main");
        assert_eq!(fs::read_to_string(&tracked).unwrap(), "dirty\n");
        git(dir, &["checkout", "-q", "--", "file.txt"]);

        // Default: preferred, then main fallback.
        assert_eq!(default_branch(dir, "alice-work").unwrap(), "alice-work");
        assert_eq!(default_branch(dir, "missing").unwrap(), "main");
        assert_eq!(default_branch(dir, "").unwrap(), "main");

        // Remote default only counts when a matching local branch exists.
        git(dir, &["remote", "add", "origin", dir.to_str().unwrap()]);
        let main_sha = git(dir, &["rev-parse", "main"]);
        git(dir, &["checkout", "-q", "-b", "develop", "main"]);
        git(
            dir,
            &["update-ref", "refs/remotes/origin/develop", &main_sha],
        );
        git(
            dir,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/develop",
            ],
        );
        assert_eq!(default_branch(dir, "").unwrap(), "develop");

        git(dir, &["checkout", "-q", "main"]);
        git(dir, &["branch", "-D", "develop"]);
        git(dir, &["update-ref", "refs/remotes/origin/ghost", &main_sha]);
        git(
            dir,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/ghost",
            ],
        );
        assert_eq!(default_branch(dir, "").unwrap(), "main");
    }
}
