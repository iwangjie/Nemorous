use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};

use crate::git::{RepoInfo, run_git_string};
use crate::git_index::oid_ok;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteAction {
    Fetch,
    Pull,
    Push,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteStatus {
    Done,
    Skipped,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    pub repo: RepoInfo,
    pub status: RemoteStatus,
    pub detail: String,
}

const WORKERS: usize = 3;
const DEADLINE: Duration = Duration::from_secs(120);
static STASH_ID: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn run_batch(
    repos: &[RepoInfo],
    action: RemoteAction,
    cancel: &AtomicBool,
) -> Vec<Report> {
    let next = AtomicUsize::new(0);
    let reports = Mutex::new(Vec::with_capacity(repos.len()));
    std::thread::scope(|scope| {
        for _ in 0..WORKERS.min(repos.len()) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(repo) = repos.get(index) else {
                        break;
                    };
                    let (status, detail) = if cancel.load(Ordering::Acquire) {
                        (RemoteStatus::Cancelled, "已取消".to_owned())
                    } else {
                        match run(repo, action, cancel) {
                            Ok(Some(detail)) => (RemoteStatus::Done, detail),
                            Ok(None) => {
                                (RemoteStatus::Skipped, "当前分支没有上游或已同步".to_owned())
                            }
                            Err(err) => {
                                let status = if err.is::<Cancelled>() {
                                    RemoteStatus::Cancelled
                                } else {
                                    RemoteStatus::Failed
                                };
                                (status, err.to_string())
                            }
                        }
                    };
                    reports.lock().unwrap_or_else(|err| err.into_inner()).push((
                        index,
                        Report {
                            repo: repo.clone(),
                            status,
                            detail,
                        },
                    ));
                }
            });
        }
    });
    let mut reports = reports.into_inner().unwrap_or_else(|err| err.into_inner());
    reports.sort_by_key(|(index, _)| *index);
    reports.into_iter().map(|(_, report)| report).collect()
}

fn run(repo: &RepoInfo, action: RemoteAction, cancel: &AtomicBool) -> Result<Option<String>> {
    let dir = Path::new(&repo.path);
    if action == RemoteAction::Fetch {
        if run_git_string(dir, &["remote"])?.trim().is_empty() {
            return Ok(None);
        }
        network(dir, &["fetch", "--all", "--prune"], cancel)?;
        return Ok(Some("已获取远端更新".to_owned()));
    }
    let Some(upstream) = upstream(dir)? else {
        if action != RemoteAction::Push {
            return Ok(None);
        }
        let branch = run_git_string(dir, &["rev-parse", "--symbolic-full-name", "HEAD"])?;
        let branch = branch.trim();
        if !branch.starts_with("refs/heads/")
            || !run_git_string(dir, &["remote"])?
                .lines()
                .any(|remote| remote == "origin")
        {
            return Ok(None);
        }
        let spec = format!("{branch}:{branch}");
        network(
            dir,
            &["push", "--set-upstream", "--", "origin", &spec],
            cancel,
        )?;
        return Ok(Some("已推送并设置上游分支".to_owned()));
    };
    if action == RemoteAction::Push {
        let heads = run_git_string(
            dir,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
        )?;
        if heads.split_whitespace().next() == Some("0") {
            return Ok(None);
        }
        let spec = format!("{}:{}", upstream.local, upstream.remote_branch);
        network(dir, &["push", "--", &upstream.remote, &spec], cancel)?;
        return Ok(Some("已推送上游分支".to_owned()));
    }

    fast_forward(dir, &upstream, cancel)?;
    Ok(Some("已快进到上游分支".to_owned()))
}

struct Upstream {
    local: String,
    remote: String,
    remote_branch: String,
}

fn upstream(dir: &Path) -> Result<Option<Upstream>> {
    let branch = run_git_string(dir, &["rev-parse", "--symbolic-full-name", "HEAD"])?;
    let branch = branch.trim();
    if !branch.starts_with("refs/heads/") {
        return Ok(None);
    }
    let info = run_git_string(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(upstream:remotename)%00%(upstream:remoteref)",
            branch,
        ],
    )?;
    let fields = info.lines().find_map(|row| {
        let (name, rest) = row.split_once('\0')?;
        (name == branch).then_some(rest)
    });
    let Some((remote, remote_branch)) = fields.and_then(|row| row.split_once('\0')) else {
        return Ok(None);
    };
    if remote.is_empty() || !remote_branch.starts_with("refs/heads/") {
        return Ok(None);
    }
    Ok(Some(Upstream {
        local: branch.to_owned(),
        remote: remote.to_owned(),
        remote_branch: remote_branch.to_owned(),
    }))
}

fn fast_forward(dir: &Path, upstream: &Upstream, cancel: &AtomicBool) -> Result<()> {
    network(dir, &["fetch", "--prune", "--", &upstream.remote], cancel)?;
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let head = run_git_string(dir, &["symbolic-ref", "--quiet", "HEAD"])?;
    ensure!(head.trim() == upstream.local, "拉取期间当前分支已变化");
    let new = run_git_string(dir, &["rev-parse", "--verify", "@{upstream}^{commit}"])?;
    let new = new.trim();
    ensure!(oid_ok(new), "无法定位上游提交");
    let current = run_git_string(dir, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    if current.trim() == new {
        return Ok(());
    }
    if run_git_string(dir, &["merge-base", "--is-ancestor", new, "HEAD"]).is_ok() {
        return Ok(());
    }
    run_git_string(dir, &["merge-base", "--is-ancestor", "HEAD", new])
        .context("本地与上游分叉，无法快进")?;

    let status = run_git_string(dir, &["status", "--porcelain", "-z", "-uall"])?;
    let stash = if status.is_empty() {
        None
    } else {
        Some(stash_local(dir)?)
    };
    let merged = if cancel.load(Ordering::Acquire) {
        Err(Cancelled.into())
    } else {
        // A local Git update completes even if cancel arrives during the filesystem write.
        run_git_string(dir, &["merge", "--ff-only", "--no-edit", new]).map(|_| ())
    };
    if let Some((oid, marker)) = stash {
        restore_local(dir, &oid, &marker).with_context(|| match &merged {
            Ok(()) => format!("已更新分支，本地变更仍保留在 stash {oid}"),
            Err(err) => format!("拉取未完成（{err}），本地变更仍保留在 stash {oid}"),
        })?;
    }
    merged
}

fn stash_local(dir: &Path) -> Result<(String, String)> {
    let sequence = STASH_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let marker = format!("nemorous-pull-{}-{nanos}-{sequence}", std::process::id());
    run_git_string(
        dir,
        &["stash", "push", "--include-untracked", "-m", &marker],
    )?;
    let stash = run_git_string(dir, &["stash", "list", "--format=%H%x00%gs"])?;
    let oid = stash
        .lines()
        .filter_map(|line| line.split_once('\0'))
        .find(|(_, subject)| subject.ends_with(&marker))
        .map(|(oid, _)| oid)
        .context("已执行贮藏，但无法确认该条目，请检查 stash 列表")?;
    ensure!(oid_ok(oid), "无法确认本地贮藏对象");
    Ok((oid.to_owned(), marker))
}

fn restore_local(dir: &Path, oid: &str, marker: &str) -> Result<()> {
    run_git_string(dir, &["stash", "apply", "--index", oid]).context("自动恢复本地变更失败")?;
    // Keep ownership explicit. If another client added a stash, leave ours as a recovery copy.
    let top = run_git_string(
        dir,
        &["log", "-1", "--format=%H%x00%gs", "-g", "refs/stash"],
    )?;
    if let Some((tip, subject)) = top.trim_end().split_once('\0') {
        if tip == oid && subject.ends_with(marker) {
            run_git_string(dir, &["stash", "drop", "stash@{0}"])?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, writer: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writer.write_str("已取消")
    }
}

impl std::error::Error for Cancelled {}

fn network(dir: &Path, args: &[&str], cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", ssh_command(dir))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().context("无法启动 Git")?;
    let mut stderr = child.stderr.take().context("无法读取 Git 状态")?;
    let sink = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.by_ref().take(4096).read_to_end(&mut bytes);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        bytes
    });
    let start = Instant::now();
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Ok(status);
        }
        let cancelled = cancel.load(Ordering::Acquire);
        if cancelled || start.elapsed() >= DEADLINE {
            stop_child(&mut child);
            break if cancelled {
                Err(Cancelled.into())
            } else {
                Err(anyhow::anyhow!("远端 Git 操作超时"))
            };
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let detail = sink.join().unwrap_or_default();
    let outcome = status?;
    if !outcome.success() {
        let stderr = String::from_utf8_lossy(&detail);
        let category =
            if stderr.contains("Authentication failed") || stderr.contains("Permission denied") {
                "认证失败"
            } else if stderr.contains("non-fast-forward") || stderr.contains("[rejected]") {
                "远端拒绝更新"
            } else if stderr.contains("Could not resolve") || stderr.contains("unable to access") {
                "无法连接远端"
            } else {
                "远端 Git 操作失败"
            };
        // Git output may echo credential-bearing remote URLs.
        bail!("{category}（退出码 {:?}）", outcome.code());
    }
    Ok(())
}

fn ssh_command(dir: &Path) -> OsString {
    std::env::var_os("GIT_SSH_COMMAND")
        .or_else(|| {
            run_git_string(dir, &["config", "--get", "core.sshCommand"])
                .ok()
                .map(|value| value.trim().to_owned().into())
        })
        .unwrap_or_else(|| "ssh -o BatchMode=yes".into())
}

pub(crate) fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        if let Ok(pid) = i32::try_from(child.id()) {
            // Only signal the process group created by this Command.
            unsafe {
                kill(-pid, 9);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("nemorous-remotes-{}-{id}", std::process::id()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn git(&self, dir: &Path, args: &[&str]) -> String {
            run_git_string(dir, args).unwrap().trim_end().to_owned()
        }

        fn identify(&self, dir: &Path) {
            self.git(dir, &["config", "user.name", "Nemorous QA"]);
            self.git(dir, &["config", "user.email", "qa@example.invalid"]);
            self.git(dir, &["config", "commit.gpgsign", "false"]);
            self.git(dir, &["config", "core.autocrlf", "false"]);
        }

        fn repo(&self, dir: &Path) -> RepoInfo {
            let name = dir.file_name().unwrap().to_str().unwrap().to_owned();
            RepoInfo {
                id: name.clone(),
                rel_path: ".".to_owned(),
                name,
                monogram: "R".to_owned(),
                path: dir.to_str().unwrap().to_owned(),
                color_idx: 0,
                is_root: true,
                desc: String::new(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn remotes_preserve_dirty_index_and_push_real_local_refs() {
        let f = Fixture::new();
        let bare = f.0.join("remote.git");
        let producer = f.0.join("producer");
        let consumer = f.0.join("consumer");
        f.git(
            &f.0,
            &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
        );
        f.git(
            &f.0,
            &[
                "clone",
                "--",
                bare.to_str().unwrap(),
                producer.to_str().unwrap(),
            ],
        );
        f.identify(&producer);
        fs::write(producer.join("local.txt"), "base\n").unwrap();
        f.git(&producer, &["add", "local.txt"]);
        f.git(&producer, &["commit", "-m", "base"]);
        f.git(&producer, &["push", "-u", "origin", "main"]);
        f.git(&producer, &["branch", "temporary"]);
        f.git(&producer, &["push", "origin", "temporary"]);
        f.git(
            &f.0,
            &[
                "clone",
                "--",
                bare.to_str().unwrap(),
                consumer.to_str().unwrap(),
            ],
        );
        f.identify(&consumer);
        fs::write(consumer.join("local.txt"), "local staged\n").unwrap();
        f.git(&consumer, &["add", "local.txt"]);
        fs::write(consumer.join("local.txt"), "local unstaged\n").unwrap();
        fs::write(consumer.join("untracked.txt"), "keep unsaved bytes\n").unwrap();

        fs::write(producer.join("remote.txt"), "new remote\n").unwrap();
        f.git(&producer, &["add", "remote.txt"]);
        f.git(&producer, &["commit", "-m", "remote update"]);
        f.git(&producer, &["push", "origin", "main"]);
        f.git(&producer, &["push", "origin", "--delete", "temporary"]);

        let repo = f.repo(&consumer);
        let cancel = AtomicBool::new(false);
        let result = run_batch(std::slice::from_ref(&repo), RemoteAction::Pull, &cancel);
        assert_eq!(result[0].status, RemoteStatus::Done, "{result:?}");
        assert_eq!(
            f.git(&consumer, &["rev-parse", "HEAD"]),
            f.git(&producer, &["rev-parse", "HEAD"])
        );
        assert_eq!(f.git(&consumer, &["show", ":local.txt"]), "local staged");
        assert_eq!(
            fs::read(consumer.join("local.txt")).unwrap(),
            b"local unstaged\n"
        );
        assert_eq!(
            fs::read(consumer.join("untracked.txt")).unwrap(),
            b"keep unsaved bytes\n"
        );
        assert!(f.git(&consumer, &["stash", "list"]).is_empty());
        assert!(!f.git(&consumer, &["branch", "-r"]).contains("temporary"));

        let result = run_batch(std::slice::from_ref(&repo), RemoteAction::Push, &cancel);
        assert_eq!(result[0].status, RemoteStatus::Skipped, "{result:?}");
        f.git(&consumer, &["commit", "-m", "local staged"]);
        let result = run_batch(std::slice::from_ref(&repo), RemoteAction::Push, &cancel);
        assert_eq!(result[0].status, RemoteStatus::Done, "{result:?}");
        assert_eq!(
            f.git(&bare, &["rev-parse", "refs/heads/main"]),
            f.git(&consumer, &["rev-parse", "HEAD"])
        );
        cancel.store(true, Ordering::Release);
        let result = run_batch(&[repo], RemoteAction::Fetch, &cancel);
        assert_eq!(result[0].status, RemoteStatus::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn remote_cancel_terminates_active_git_transport() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new();
        f.git(&f.0, &["init", "-b", "main"]);
        let script = f.0.join("ssh-test");
        fs::write(&script, "#!/bin/sh\ntouch transport-started\nsleep 60\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        f.git(&f.0, &["config", "core.sshCommand", "./ssh-test"]);
        f.git(&f.0, &["config", "ssh.variant", "ssh"]);
        f.git(
            &f.0,
            &["remote", "add", "origin", "ssh://test@localhost/fake"],
        );
        let repo = f.repo(&f.0);
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| run_batch(&[repo], RemoteAction::Fetch, &cancel));
            while !f.0.join("transport-started").exists()
                && started.elapsed() < Duration::from_secs(4)
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            let reached_transport = f.0.join("transport-started").exists();
            cancel.store(true, Ordering::Release);
            let report = worker.join().unwrap();
            assert!(reached_transport);
            assert_eq!(report[0].status, RemoteStatus::Cancelled);
            assert!(started.elapsed() < Duration::from_secs(8));
        });
    }
}
