//! Listing and executing interactive rebase plans.
//!
//! `list_plan` returns the commits in `upstream..HEAD` in the order Git would
//! replay them (oldest first). `run_plan` validates a plan against the exact
//! commit set and replays it through a real `git rebase --interactive` using a
//! private, scripted todo/message editor. Nothing is written to Git config and
//! a failed run is left in progress (no implicit `git rebase --abort`).

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail, ensure};

use crate::git::{BoundedGitOutput, run_git_limited_with_env, run_git_string};
use crate::git_index::oid_ok;

/// Maximum captured stdout/stderr kept for diagnostics.
const MAX_CAPTURE: usize = 64 * 1024;
/// Maximum accepted reword message size.
const MAX_MESSAGE: usize = 1024 * 1024;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub(crate) struct RebaseCommit {
    pub hash: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub(crate) enum StepAction {
    Pick,
    Reword(String),
    Squash,
    Drop,
}

#[derive(Debug, Clone)]
pub(crate) struct RebaseStep {
    pub hash: String,
    pub action: StepAction,
}

/// Lists `upstream..HEAD` oldest first, with the raw commit message of each.
pub(crate) fn list_plan(repo: &Path, upstream: &str) -> Result<Vec<RebaseCommit>> {
    let upstream_oid = resolve_commit(repo, upstream)?;
    let head_oid = resolve_commit(repo, "HEAD")?;
    let range = format!("{upstream_oid}..{head_oid}");
    let hashes = run_git_string(
        repo,
        &["rev-list", "--reverse", "--no-merges", range.as_str()],
    )
    .with_context(|| format!("listing commits in {range}"))?;

    let mut commits = Vec::new();
    for hash in hashes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let raw = run_git_string(
            repo,
            &["log", "-1", "--no-show-signature", "--format=%B", hash],
        )
        .with_context(|| format!("reading message of {hash}"))?;
        let message = raw.strip_suffix('\n').unwrap_or(&raw).to_string();
        commits.push(RebaseCommit {
            hash: hash.to_string(),
            message,
        });
    }
    Ok(commits)
}

/// Replays exactly the commits in `upstream..expected_head`, in the order given
/// by `steps`, using real interactive Git rebase machinery.
pub(crate) fn run_plan(
    repo: &Path,
    expected_head: &str,
    upstream: &str,
    steps: &[RebaseStep],
) -> Result<()> {
    ensure!(oid_ok(expected_head), "交互式 rebase 需要固定 HEAD 提交");
    let expected = resolve_commit(repo, expected_head)?;
    ensure!(
        crate::git_history_actions::active_operation(repo)?.is_none(),
        "请先完成当前 Git 操作"
    );
    let upstream_oid = resolve_commit(repo, upstream)?;
    let actual = resolve_commit(repo, "HEAD")?;
    if actual != expected {
        bail!("HEAD is {actual}, but the plan expects {expected}");
    }

    let range_spec = format!("{upstream_oid}..{expected}");
    let range_raw = run_git_string(
        repo,
        &["rev-list", "--reverse", "--no-merges", range_spec.as_str()],
    )
    .with_context(|| format!("reading commit range {range_spec}"))?;
    let range: Vec<String> = range_raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let range_set: HashSet<&str> = range.iter().map(String::as_str).collect();

    if steps.len() != range.len() {
        bail!(
            "plan has {} step(s) but {range_spec} has {} commit(s)",
            steps.len(),
            range.len()
        );
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut kept = false;
    for step in steps {
        if !is_full_oid(&step.hash) {
            bail!("step hash is not a full object id: {:?}", step.hash);
        }
        if !seen.insert(step.hash.as_str()) {
            bail!("commit {} appears more than once in the plan", step.hash);
        }
        if !range_set.contains(step.hash.as_str()) {
            bail!("commit {} is not in {range_spec}", step.hash);
        }
        match &step.action {
            StepAction::Pick => kept = true,
            StepAction::Reword(message) => {
                validate_reword_message(message)?;
                kept = true;
            }
            StepAction::Squash => {
                if !kept {
                    bail!(
                        "squash step for {} has no earlier kept (pick/reword) commit",
                        step.hash
                    );
                }
            }
            StepAction::Drop => {}
        }
    }

    if steps.is_empty() {
        return Ok(());
    }

    let scratch = create_private_dir()?;
    let todo_path = scratch.0.join("todo");
    let mut todo = String::new();
    let mut rewords: Vec<(String, PathBuf)> = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        match &step.action {
            StepAction::Pick => todo.push_str(&format!("pick {}\n", step.hash)),
            StepAction::Drop => todo.push_str(&format!("drop {}\n", step.hash)),
            StepAction::Squash => todo.push_str(&format!("squash {}\n", step.hash)),
            StepAction::Reword(message) => {
                // `reword` on an empty message/commit is unreliable on Git 2.39;
                // stop on the commit with `edit` and amend it ourselves.
                let path = scratch.0.join(format!("message-{index}"));
                write_private_file(&path, message.as_bytes(), 0o600)?;
                rewords.push((step.hash.clone(), path));
                todo.push_str(&format!("edit {}\n", step.hash));
            }
        }
    }
    write_private_file(&todo_path, todo.as_bytes(), 0o600)?;

    let env = editor_env(&scratch.0, &todo_path)?;
    let rebase_args = [
        "-c",
        "rebase.missingCommitsCheck=ignore",
        "rebase",
        "--interactive",
        "--empty=keep",
        "--reapply-cherry-picks",
        "--no-rebase-merges",
        "--no-fork-point",
        "--no-autosquash",
        "--no-autostash",
        upstream_oid.as_str(),
    ];

    let mut outcome = run_git_bounded(repo, &rebase_args, &env)?;
    let mut next_reword = 0usize;
    let max_rounds = steps.len().saturating_mul(4).saturating_add(16);

    for _ in 0..max_rounds {
        let Some(state) = rebase_state_dir(repo)? else {
            ensure!(
                outcome.success(),
                "交互式 rebase 失败：{}",
                outcome.summary()
            );
            ensure!(next_reword == rewords.len(), "交互式 rebase 未执行全部改写");
            return Ok(());
        };

        let unmerged = run_git_string(repo, &["ls-files", "--unmerged"])?;
        if !unmerged.trim().is_empty() {
            bail!(
                "rebase stopped with unresolved conflicts ({}); no automatic abort was performed\n{}",
                state.display(),
                outcome.summary()
            );
        }

        let Some((expected_reword, message_path)) = rewords.get(next_reword) else {
            bail!(
                "rebase stopped unexpectedly ({}) with no reword left to apply; no automatic abort was performed\n{}",
                state.display(),
                outcome.summary()
            );
        };
        ensure!(
            outcome.success(),
            "交互式 rebase 暂停：{}",
            outcome.summary()
        );
        let stopped =
            fs::read_to_string(state.join("stopped-sha")).context("Git 未确认当前改写步骤")?;
        ensure!(
            resolve_commit(repo, stopped.trim())? == *expected_reword,
            "Git 暂停在计划外的提交"
        );
        let message_path = message_path
            .to_str()
            .context("temporary message path is not valid UTF-8")?;

        let amend = run_git_bounded(
            repo,
            &[
                "commit",
                "--amend",
                "--allow-empty",
                "--allow-empty-message",
                "--cleanup=whitespace",
                "--file",
                message_path,
            ],
            &env,
        )?;
        if !amend.success() {
            bail!(
                "failed to apply reword message for {expected_reword} (rebase left in progress; no automatic abort)\n{}",
                amend.summary()
            );
        }
        next_reword += 1;
        outcome = run_git_bounded(repo, &["rebase", "--continue"], &env)?;
    }

    bail!("rebase plan did not converge after {max_rounds} steps")
}

fn validate_reword_message(message: &str) -> Result<()> {
    if message.as_bytes().contains(&0) {
        bail!("reword message contains a NUL byte");
    }
    if message.len() > MAX_MESSAGE {
        bail!("reword message exceeds {MAX_MESSAGE} bytes");
    }
    Ok(())
}

fn is_full_oid(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && oid_ok(value)
}

fn resolve_commit(repo: &Path, revision: &str) -> Result<String> {
    if revision.is_empty() {
        bail!("empty revision");
    }
    let spec = format!("{revision}^{{commit}}");
    let out = run_git_string(
        repo,
        &["rev-parse", "--verify", "--end-of-options", spec.as_str()],
    )
    .with_context(|| format!("resolving revision {revision:?}"))?;
    let oid = out.trim();
    if !oid_ok(oid) {
        bail!("git returned an invalid object id for {revision:?}: {oid:?}");
    }
    Ok(oid.to_ascii_lowercase())
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn create_private_dir() -> Result<TempDir> {
    let mut last_error = None;
    for _ in 0..32 {
        let counter = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "nemorous-rebase-plan-{}-{counter}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return Ok(TempDir(path)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(err);
                continue;
            }
            Err(err) => return Err(err).with_context(|| format!("creating {}", path.display())),
        }
    }
    bail!(
        "could not create a private temporary directory: {}",
        last_error
            .map(|err| err.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    )
}

fn write_private_file(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("writing {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flushing {}", path.display()))?;
    Ok(())
}

const SEQUENCE_EDITOR: &str = "#!/bin/sh\nset -eu\nif [ \"$#\" -ne 1 ]; then\n  exit 128\nfi\ncat \"$NEMOROUS_REBASE_TODO\" > \"$1\"\n";
const MESSAGE_EDITOR: &str = "#!/bin/sh\nexit 0\n";

fn editor_env(dir: &Path, todo: &Path) -> Result<Vec<(String, String)>> {
    let sequence = dir.join("sequence-editor.sh");
    let message = dir.join("message-editor.sh");
    write_private_file(&sequence, SEQUENCE_EDITOR.as_bytes(), 0o700)?;
    write_private_file(&message, MESSAGE_EDITOR.as_bytes(), 0o700)?;
    let todo = todo
        .to_str()
        .context("temporary todo path is not valid UTF-8")?;
    Ok(vec![
        ("GIT_SEQUENCE_EDITOR".to_string(), shell_quote(&sequence)?),
        ("GIT_EDITOR".to_string(), shell_quote(&message)?),
        ("NEMOROUS_REBASE_TODO".to_string(), todo.to_string()),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
        ("LC_ALL".to_string(), "C".to_string()),
    ])
}

fn shell_quote(path: &Path) -> Result<String> {
    let raw = path.to_str().context("temporary path is not valid UTF-8")?;
    let mut quoted = String::with_capacity(raw.len() + 2);
    quoted.push('\'');
    for ch in raw.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    Ok(quoted)
}

struct GitOutcome(Option<String>);

impl GitOutcome {
    fn success(&self) -> bool {
        self.0.is_none()
    }
    fn summary(&self) -> &str {
        self.0.as_deref().unwrap_or_default()
    }
}

fn run_git_bounded(repo: &Path, args: &[&str], env: &[(String, String)]) -> Result<GitOutcome> {
    let env = env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    match run_git_limited_with_env(repo, args, MAX_CAPTURE, None, None, &env)? {
        BoundedGitOutput::Ok(_) => Ok(GitOutcome(None)),
        BoundedGitOutput::Failed { stderr, exit } => {
            Ok(GitOutcome(Some(format!("退出码 {exit:?}：{stderr}"))))
        }
        BoundedGitOutput::TooLarge => bail!("Git 输出超过大小上限"),
    }
}

fn rebase_state_dir(repo: &Path) -> Result<Option<PathBuf>> {
    let git_dir = run_git_string(repo, &["rev-parse", "--absolute-git-dir"])?;
    let git_dir = PathBuf::from(git_dir.trim());
    for name in ["rebase-merge", "rebase-apply"] {
        let candidate = git_dir.join(name);
        if candidate.is_dir() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{RebaseStep, StepAction, TempDir, create_private_dir, list_plan, run_plan};
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Stdio};

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn git_out(repo: &Path, args: &[&str]) -> String {
        crate::git::run_git_string(repo, args)
            .unwrap_or_else(|err| panic!("git {args:?} in {} failed: {err:#}", repo.display()))
    }

    struct Fixture {
        dir: TempDir,
    }

    impl Fixture {
        fn path(&self) -> &Path {
            &self.dir.0
        }
    }

    fn init_repo() -> Fixture {
        let dir = create_private_dir().expect("private temporary directory");
        git_out(&dir.0, &["init", "-q"]);
        git_out(&dir.0, &["config", "user.name", "Test"]);
        git_out(&dir.0, &["config", "user.email", "test@example.com"]);
        git_out(&dir.0, &["config", "commit.gpgsign", "false"]);
        Fixture { dir }
    }

    fn commit(fixture: &Fixture, file: &str, contents: &str, message: &str) -> String {
        fs::write(fixture.path().join(file), contents).expect("write fixture file");
        git_out(fixture.path(), &["add", file]);
        git_out(fixture.path(), &["commit", "-q", "-m", message]);
        git_out(fixture.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }

    fn empty_commit(fixture: &Fixture, message: &str) -> String {
        git_out(
            fixture.path(),
            &["commit", "-q", "--allow-empty", "-m", message],
        );
        git_out(fixture.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string()
    }

    #[test]
    fn reword_nonempty_and_empty_messages() {
        if !git_available() {
            eprintln!("git is not available; skipping");
            return;
        }
        let fixture = init_repo();
        let repo = fixture.path();

        commit(&fixture, "a.txt", "a\n", "base");
        let upstream = git_out(repo, &["rev-parse", "HEAD"]).trim().to_string();
        let second = commit(&fixture, "b.txt", "b\n", "second");
        let empty = empty_commit(&fixture, "explicit empty");

        let plan = list_plan(repo, &upstream).expect("list plan");
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].hash, second);
        assert_eq!(plan[0].message.trim_end_matches('\n'), "second");
        assert_eq!(plan[1].hash, empty);
        assert_eq!(plan[1].message.trim_end_matches('\n'), "explicit empty");

        let expected = git_out(repo, &["rev-parse", "HEAD"]).trim().to_string();
        let steps = vec![
            RebaseStep {
                hash: second.clone(),
                action: StepAction::Reword("second reworded\n\nbody text".to_string()),
            },
            RebaseStep {
                hash: empty.clone(),
                action: StepAction::Reword(String::new()),
            },
        ];
        run_plan(repo, &expected, &upstream, &steps).expect("run reword plan");

        let range = format!("{upstream}..HEAD");
        let hashes: Vec<String> = git_out(
            repo,
            &["rev-list", "--reverse", "--no-merges", range.as_str()],
        )
        .lines()
        .map(str::to_string)
        .collect();
        assert_eq!(hashes.len(), 2);

        let first_message = git_out(repo, &["log", "-1", "--format=%B", hashes[0].as_str()]);
        assert_eq!(
            first_message.trim_end_matches('\n'),
            "second reworded\n\nbody text"
        );

        let empty_message = git_out(repo, &["log", "-1", "--format=%B", hashes[1].as_str()]);
        assert!(empty_message.trim().is_empty());

        let parent = format!("{}^", hashes[1]);
        let parent = git_out(repo, &["rev-parse", parent.as_str()]);
        let parent = parent.trim().to_string();
        let parent_tree = format!("{parent}^{{tree}}");
        let commit_tree = format!("{}^{{tree}}", hashes[1]);
        assert_eq!(
            git_out(repo, &["rev-parse", commit_tree.as_str()]).trim(),
            git_out(repo, &["rev-parse", parent_tree.as_str()]).trim(),
            "explicit empty commit must stay empty"
        );
    }

    #[test]
    fn reorders_commits() {
        if !git_available() {
            eprintln!("git is not available; skipping");
            return;
        }
        let fixture = init_repo();
        let repo = fixture.path();

        commit(&fixture, "base.txt", "base\n", "base");
        let upstream = git_out(repo, &["rev-parse", "HEAD"]).trim().to_string();
        let one = commit(&fixture, "one.txt", "1\n", "one");
        let two = commit(&fixture, "two.txt", "2\n", "two");
        let three = commit(&fixture, "three.txt", "3\n", "three");

        let expected = git_out(repo, &["rev-parse", "HEAD"]).trim().to_string();
        let steps = vec![
            RebaseStep {
                hash: three.clone(),
                action: StepAction::Pick,
            },
            RebaseStep {
                hash: one,
                action: StepAction::Pick,
            },
            RebaseStep {
                hash: two,
                action: StepAction::Pick,
            },
        ];
        run_plan(repo, &expected, &upstream, &steps).expect("run reordered plan");

        let range = format!("{upstream}..HEAD");
        let subjects = git_out(repo, &["log", "--reverse", "--format=%s", range.as_str()]);
        let subjects: Vec<&str> = subjects.lines().collect();
        assert_eq!(subjects, ["three", "one", "two"]);
        assert_eq!(fs::read_to_string(repo.join("one.txt")).unwrap(), "1\n");
        assert_eq!(fs::read_to_string(repo.join("two.txt")).unwrap(), "2\n");
        assert_eq!(fs::read_to_string(repo.join("three.txt")).unwrap(), "3\n");
    }

    #[test]
    fn rejects_invalid_plans_without_touching_head() {
        if !git_available() {
            eprintln!("git is not available; skipping");
            return;
        }
        let fixture = init_repo();
        let repo = fixture.path();

        commit(&fixture, "base.txt", "base\n", "base");
        let upstream = git_out(repo, &["rev-parse", "HEAD"]).trim().to_string();
        let first = commit(&fixture, "first.txt", "1\n", "first");
        let second = commit(&fixture, "second.txt", "2\n", "second");
        let expected = git_out(repo, &["rev-parse", "HEAD"]).trim().to_string();

        let missing = vec![RebaseStep {
            hash: first.clone(),
            action: StepAction::Pick,
        }];
        assert!(run_plan(repo, &expected, &upstream, &missing).is_err());

        let duplicate = vec![
            RebaseStep {
                hash: first.clone(),
                action: StepAction::Pick,
            },
            RebaseStep {
                hash: first.clone(),
                action: StepAction::Pick,
            },
        ];
        assert!(run_plan(repo, &expected, &upstream, &duplicate).is_err());

        let squash_first = vec![
            RebaseStep {
                hash: first.clone(),
                action: StepAction::Squash,
            },
            RebaseStep {
                hash: second.clone(),
                action: StepAction::Pick,
            },
        ];
        assert!(run_plan(repo, &expected, &upstream, &squash_first).is_err());

        let abbreviated = vec![
            RebaseStep {
                hash: first[..7].to_string(),
                action: StepAction::Pick,
            },
            RebaseStep {
                hash: second.clone(),
                action: StepAction::Pick,
            },
        ];
        assert!(run_plan(repo, &expected, &upstream, &abbreviated).is_err());

        let foreign = vec![
            RebaseStep {
                hash: upstream.clone(),
                action: StepAction::Pick,
            },
            RebaseStep {
                hash: second.clone(),
                action: StepAction::Pick,
            },
        ];
        assert!(run_plan(repo, &expected, &upstream, &foreign).is_err());

        let head_now = git_out(repo, &["rev-parse", "HEAD"]).trim().to_string();
        assert_eq!(head_now, expected);
        assert!(!repo.join(".git/rebase-merge").exists());
        assert!(!repo.join(".git/rebase-apply").exists());
    }
}
