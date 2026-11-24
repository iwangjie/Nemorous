use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::workbench_process::ChildGroup;

const PR_FIELDS: &str = "number,title,state,url,isDraft,headRefName,baseRefName,author,updatedAt";
const DETAIL_FIELDS: &str = "number,title,state,url,isDraft,headRefName,baseRefName,author,updatedAt,body,comments,reviews,statusCheckRollup";

pub(crate) fn repository(remote: &str) -> Result<String> {
    let remote = remote.trim().trim_end_matches('/');
    let path = if let Some((scheme, rest)) = remote.split_once("://") {
        ensure!(
            matches!(scheme, "http" | "https" | "ssh"),
            "无法读取 GitHub 远端"
        );
        let (authority, path) = rest.split_once('/').context("GitHub 远端缺少仓库")?;
        let host = authority.rsplit('@').next().unwrap_or_default();
        ensure!(
            host == "github.com" || host == "github.com:22",
            "当前远端未指向 GitHub"
        );
        path
    } else {
        let (host, path) = remote.split_once(':').context("GitHub 远端缺少仓库")?;
        ensure!(
            host.rsplit('@').next() == Some("github.com"),
            "当前远端未指向 GitHub"
        );
        path
    };
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/').context("GitHub 远端路径无效")?;
    let valid = |part: &str| {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    ensure!(valid(owner) && valid(name), "GitHub 远端路径无效");
    Ok(path.to_owned())
}

pub(crate) enum Action {
    List(String),
    Detail(u64),
    Comment {
        number: u64,
        body: String,
    },
    Create {
        title: String,
        body: String,
        base: String,
        draft: bool,
    },
}

impl Action {
    pub(crate) fn is_submission(&self) -> bool {
        matches!(self, Self::Comment { .. } | Self::Create { .. })
    }
}

fn command(program: &Path, path: &Path, repository: &str, action: &Action) -> Result<Command> {
    let mut cmd = crate::tool_command::command(program);
    cmd.current_dir(path)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0");
    cmd.arg("pr");
    match action {
        Action::List(state) => {
            ensure!(
                matches!(state.as_str(), "open" | "closed" | "merged" | "all"),
                "PR 状态无效"
            );
            cmd.args([
                "list", "--state", state, "--limit", "50", "--json", PR_FIELDS,
            ]);
        }
        Action::Detail(number) => {
            cmd.args(["view", &number.to_string(), "--json", DETAIL_FIELDS]);
        }
        Action::Comment { number, body } => {
            ensure!(!body.trim().is_empty(), "请输入评论");
            cmd.args(["comment", &number.to_string(), "--body", body]);
        }
        Action::Create {
            title,
            body,
            base,
            draft,
        } => {
            ensure!(
                !title.trim().is_empty() && !base.trim().is_empty(),
                "请输入标题和目标分支"
            );
            cmd.args(["create", "--title", title, "--body", body, "--base", base]);
            if *draft {
                cmd.arg("--draft");
            }
        }
    }
    cmd.args(["--repo", repository]);
    Ok(cmd)
}

pub(crate) fn run(
    repo: &Path,
    expected_repository: &str,
    action: Action,
    cancelled: &AtomicBool,
) -> Result<(String, Value)> {
    ensure!(
        !expected_repository.is_empty() || matches!(&action, Action::List(_)),
        "请先加载当前仓库的 PR 列表"
    );
    ensure!(repo.join(".git").exists(), "当前目录已不再是 Git 仓库");
    let remote = crate::git::run_git_string(repo, &["remote", "get-url", "origin"])
        .map_err(|_| anyhow::anyhow!("当前仓库缺少 origin"))?;
    let target = repository(&remote)?;
    ensure!(
        expected_repository.is_empty() || target.eq_ignore_ascii_case(expected_repository),
        "origin 已更改，请重新打开此仓库"
    );
    let mut cmd = command(Path::new("gh"), repo, &target, &action)?;
    let output = capture(&mut cmd, cancelled, action.is_submission())?;
    let value = match action {
        Action::List(_) | Action::Detail(_) => {
            serde_json::from_str(&output).context("无法读取 gh 返回的数据")?
        }
        _ => Value::String(output.trim().to_owned()),
    };
    Ok((target, value))
}

fn capture(command: &mut Command, cancelled: &AtomicBool, submission: bool) -> Result<String> {
    ensure!(!cancelled.load(Ordering::Acquire), "GitHub 操作已取消");
    let mut process = ChildGroup::spawn(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .context("无法启动 gh，请确认已安装 GitHub CLI")?;
    let collect = |mut stream: Box<dyn Read + Send>, limit: u64| {
        std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
            let mut result = Vec::new();
            stream.by_ref().take(limit + 1).read_to_end(&mut result)?;
            std::io::copy(&mut stream, &mut std::io::sink())?;
            Ok(result)
        })
    };
    let out = collect(
        Box::new(process.child.stdout.take().context("无法读取 gh")?),
        4 * 1024 * 1024,
    );
    let err = collect(
        Box::new(process.child.stderr.take().context("无法读取 gh")?),
        4096,
    );
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            bail!("GitHub 操作已取消");
        }
        if Instant::now() >= deadline {
            if submission {
                bail!("提交结果未确认，请先在 GitHub 核对后再试");
            }
            bail!("GitHub 请求超时");
        }
        if let Some(status) = process.child.try_wait()? {
            if out.is_finished() && err.is_finished() {
                break status;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let read = |thread: std::thread::JoinHandle<std::io::Result<Vec<u8>>>| {
        thread
            .join()
            .map_err(|_| anyhow::anyhow!("读取 gh 输出失败"))?
            .map_err(anyhow::Error::from)
    };
    if !status.success() {
        let error = String::from_utf8_lossy(&read(err)?).to_ascii_lowercase();
        if error.contains("gh auth login")
            || status.code() == Some(4)
            || error.contains("authentication")
        {
            bail!("GitHub CLI 需要认证");
        }
        if submission {
            bail!("提交结果未确认，请先在 GitHub 核对后再试（gh 退出码 {:?}）", status.code());
        }
        bail!("GitHub CLI 请求失败（退出码 {:?}）", status.code());
    }
    let data = read(out)?;
    ensure!(data.len() <= 4 * 1024 * 1024, "GitHub CLI 返回超过大小限制");
    Ok(String::from_utf8(data)?)
}

pub(crate) fn safe_url(repository: &str, url: &str) -> bool {
    url.strip_prefix("https://github.com/")
        .and_then(|path| path.strip_prefix(repository))
        .is_some_and(|tail| tail.starts_with("/pull/") && !url.chars().any(char::is_control))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn gh_argv_and_redacted_origin() {
        assert_eq!(
            repository("git@github.com:demo/native.git").unwrap(),
            "demo/native"
        );
        assert_eq!(
            repository("https://secret@github.com/demo/native").unwrap(),
            "demo/native"
        );
        assert!(repository("git@github.com:demo/../native").is_err());
        assert!(safe_url(
            "demo/native",
            "https://github.com/demo/native/pull/2"
        ));
        assert!(!safe_url(
            "demo/native",
            "https://github.com/demo/native-fork/pull/2"
        ));
        let mut mock = Command::new("/bin/sh");
        mock.args(["-c", "printf '%s' '[{\"number\":2}]'"]);
        let result = capture(&mut mock, &AtomicBool::new(false), false).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result).unwrap()[0]["number"],
            2
        );
        let create = command(
            Path::new("gh"),
            Path::new("."),
            "demo/native",
            &Action::Create {
                title: "Test title".into(),
                body: "- literal body".into(),
                base: "main".into(),
                draft: true,
            },
        )
        .unwrap();
        assert_eq!(
            create.get_args().collect::<Vec<_>>(),
            [
                "pr",
                "create",
                "--title",
                "Test title",
                "--body",
                "- literal body",
                "--base",
                "main",
                "--draft",
                "--repo",
                "demo/native"
            ]
        );
    }
}
