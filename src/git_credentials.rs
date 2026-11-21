use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::git::{BoundedGitOutput, run_git_limited, run_git_string};

pub(crate) fn https(repo: &Path, host: &str, username: &str, token: Option<&str>) -> Result<()> {
    let host = clean_host(host)?;
    ensure!(!username.contains(['\0', '\r', '\n']), "用户名不能含有换行");
    if let Some(token) = token {
        ensure!(
            !token.is_empty() && !token.contains(['\0', '\r', '\n']),
            "请输入有效令牌"
        );
    }
    let config = run_git_string(repo, &["config", "--get-all", "credential.helper"])
        .map_err(|_| anyhow::anyhow!("Git 没有配置持久化凭据助手"))?;
    let helper = config
        .lines()
        .rev()
        .find_map(persistent_helper)
        .context("未找到可用的 Git 持久化凭据助手")?;
    let action = if token.is_some() { "store" } else { "erase" };
    let mut payload = format!("protocol=https\nhost={host}\n");
    if !username.is_empty() {
        payload.push_str(&format!("username={username}\n"));
    }
    if let Some(token) = token {
        payload.push_str(&format!("password={token}\n"));
    }
    payload.push('\n');
    let mut args = vec![helper.0.as_str()];
    if let Some(option) = helper.1.as_deref() {
        args.push(option);
    }
    args.push(action);
    match run_git_limited(repo, &args, 1024, Some(payload.into_bytes())) {
        Ok(BoundedGitOutput::Ok(_)) => Ok(()),
        _ => bail!("凭据助手未能完成操作"),
    }
}

fn clean_host(raw: &str) -> Result<&str> {
    ensure!(!raw.contains(['\0', '\r', '\n']), "主机地址格式无效");
    let raw = raw.trim();
    let raw = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let raw = raw.split(['/', '?', '#']).next().unwrap_or("");
    let host = raw.rsplit_once('@').map_or(raw, |(_, host)| host);
    ensure!(
        !host.is_empty()
            && !host.starts_with('-')
            && host.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':' | b'[' | b']')
            }),
        "主机地址格式无效"
    );
    Ok(host)
}

fn persistent_helper(line: &str) -> Option<(String, Option<String>)> {
    let (name, value) = line
        .trim()
        .split_once(char::is_whitespace)
        .unwrap_or((line.trim(), ""));
    let name = name.strip_prefix("credential-").unwrap_or(name);
    match name {
        "osxkeychain" | "manager" | "manager-core" | "libsecret" | "wincred"
            if value.is_empty() =>
        {
            Some((format!("credential-{name}"), None))
        }
        "store" => {
            if value.trim().is_empty() {
                return Some(("credential-store".to_owned(), None));
            }
            let path = value
                .trim()
                .strip_prefix("--file=")
                .or_else(|| value.trim().strip_prefix("--file "))?
                .trim()
                .trim_matches(['"', '\'']);
            (!path.is_empty() && !path.contains(['\0', '\n', '\r'])).then(|| {
                (
                    "credential-store".to_owned(),
                    Some(format!("--file={path}")),
                )
            })
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SshKey {
    pub path: PathBuf,
    pub fingerprint: String,
    pub public_key: String,
}

pub(crate) fn ssh_keys(directory: &Path) -> Result<Vec<SshKey>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let mut keys = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("pub") || !entry.file_type()?.is_file()
        {
            continue;
        }
        if entry.metadata()?.len() > 64 * 1024 {
            continue;
        }
        let Ok(public_key) = fs::read_to_string(&path) else {
            continue;
        };
        let fingerprint = path
            .to_str()
            .and_then(|path| ssh_command("ssh-keygen", &["-lf", path]).ok())
            .and_then(|(status, line)| status.success().then_some(line))
            .unwrap_or_default();
        keys.push(SshKey {
            path,
            fingerprint,
            public_key,
        });
    }
    keys.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(keys)
}

pub(crate) fn create_key(directory: &Path, name: &str, comment: &str) -> Result<String> {
    let path = Path::new(name);
    let mut components = path.components();
    ensure!(
        !name.starts_with('.')
            && !name.starts_with('-')
            && matches!(components.next(), Some(Component::Normal(value)) if value == name)
            && components.next().is_none()
            && !name.contains(['\0', '\r', '\n', '\\']),
        "输入有效的密钥文件名"
    );
    ensure!(!comment.contains(['\0', '\r', '\n']), "注释不能包含换行");
    fs::create_dir_all(directory)?;
    let key = directory.join(path);
    let mut public = key.as_os_str().to_os_string();
    public.push(".pub");
    let public = PathBuf::from(public);
    ensure!(
        fs::symlink_metadata(&key).is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
            && fs::symlink_metadata(&public)
                .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound),
        "此名称的密钥已经存在"
    );
    let (status, _) = ssh_command(
        "ssh-keygen",
        &[
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            comment,
            "-f",
            key.to_str().context("密钥路径无效")?,
        ],
    )?;
    ensure!(status.success(), "ssh-keygen 未能创建新密钥");
    Ok(fs::read_to_string(public)?)
}

pub(crate) fn agent_keys() -> Result<String> {
    let (status, text) = ssh_command("ssh-add", &["-l"])?;
    if status.code() == Some(1) {
        return Ok("SSH Agent 尚无密钥".to_owned());
    }
    ensure!(status.success(), "无法读取 SSH Agent");
    Ok(text)
}

pub(crate) fn test_ssh(host: &str) -> Result<String> {
    let hostname = match host.rsplit_once('@') {
        Some((user, hostname)) => {
            ensure!(
                !user.is_empty()
                    && user
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
                "无效的 SSH 用户名"
            );
            hostname
        }
        None => host,
    };
    ensure!(
        clean_host(hostname)? == hostname && !hostname.starts_with('-'),
        "请输入 SSH 主机或 user@host"
    );
    let (status, text) = ssh_command(
        "ssh",
        &[
            "-oBatchMode=yes",
            "-oConnectTimeout=8",
            "-oStrictHostKeyChecking=yes",
            "-T",
            "--",
            host,
        ],
    )?;
    if status.success() || status.code() == Some(1) {
        if text.is_empty() {
            return Ok("SSH 连接成功".to_owned());
        }
        return Ok(text);
    }
    bail!("SSH 连接失败（退出码 {:?}）：{text}", status.code())
}

fn ssh_command(program: &str, args: &[&str]) -> Result<(std::process::ExitStatus, String)> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .with_context(|| format!("无法启动 {program}"))?;
    let capture = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.by_ref().take(4096).read_to_end(&mut bytes);
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            bytes
        })
    };
    let out = child
        .stdout
        .take()
        .map(|pipe| capture(Box::new(pipe)))
        .unwrap();
    let err = child
        .stderr
        .take()
        .map(|pipe| capture(Box::new(pipe)))
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            if out.is_finished() && err.is_finished() {
                break status;
            }
        }
        if start.elapsed() > Duration::from_secs(15) {
            crate::git_remotes::stop_child(&mut child);
            bail!("{program} 操作超时");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut bytes = out.join().unwrap_or_default();
    bytes.extend(err.join().unwrap_or_default());
    Ok((status, String::from_utf8_lossy(&bytes).trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_uses_configured_local_store_for_save_and_erase() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nemorous-credential-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(root.clone());
        run_git_string(&root, &["init", "-b", "main"]).unwrap();
        let path = root.join("local synthetic credentials");
        let form = format!("store --file={}", path.display());
        run_git_string(&root, &["config", "--local", "credential.helper", &form]).unwrap();
        https(
            &root,
            "https://example.invalid/project",
            "test-user",
            Some("fixture-only"),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "https://test-user:fixture-only@example.invalid\n"
        );
        assert!(
            https(
                &root,
                "example.invalid",
                "test-user\npassword=bad",
                Some("fixture-only")
            )
            .is_err()
        );
        https(&root, "example.invalid", "test-user", None).unwrap();
        assert!(fs::read_to_string(&path).unwrap_or_default().is_empty());
    }

    use std::time::{SystemTime, UNIX_EPOCH};
}
