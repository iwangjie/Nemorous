use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::git::run_git_string;
use crate::git_index::{index_git, literal_file};

#[derive(Debug, Clone)]
pub(crate) enum AdminAction {
    Tag(TagAction),
    Remote(RemoteAction),
    Submodule(SubmoduleAction),
    Sparse(SparseAction),
    Lfs(LfsAction),
    Signing {
        enabled: bool,
        format: Option<String>,
        key: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum TagAction {
    Create {
        name: String,
        rev: Option<String>,
        message: Option<String>,
    },
    Delete {
        name: String,
    },
    Push {
        name: String,
        remote: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum RemoteAction {
    Add { name: String, url: String },
    ChangeUrl { name: String, url: String },
    Delete { name: String },
}

#[derive(Debug, Clone)]
pub(crate) enum SubmoduleAction {
    Add { destination: String, url: String },
    Update,
    Sync,
    Remove { path: String },
}

#[derive(Debug, Clone)]
pub(crate) enum SparseAction {
    Set { patterns: Vec<String> },
    Disable,
}

#[derive(Debug, Clone)]
pub(crate) enum LfsAction {
    InstallLocal,
    Pull,
    Track { patterns: Vec<String> },
}

#[derive(Debug, Clone)]
pub(crate) struct RemoteSummary {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SubmoduleSummary {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SigningSummary {
    pub enabled: bool,
    pub format: Option<String>,
    pub key_set: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct AdminSummary {
    pub tags: Vec<String>,
    pub remotes: Vec<RemoteSummary>,
    pub submodules: Vec<SubmoduleSummary>,
    pub sparse_enabled: bool,
    pub lfs_installed: bool,
    pub signing: SigningSummary,
}

pub(crate) fn run(repo: &Path, action: &AdminAction) -> Result<String> {
    ensure_repo(repo)?;
    match action {
        AdminAction::Tag(a) => tag(repo, a),
        AdminAction::Remote(a) => remote(repo, a),
        AdminAction::Submodule(a) => submodule(repo, a),
        AdminAction::Sparse(a) => sparse(repo, a),
        AdminAction::Lfs(a) => lfs(repo, a),
        AdminAction::Signing {
            enabled,
            format,
            key,
        } => signing(repo, *enabled, format.as_deref(), key.as_deref()),
    }
}

pub(crate) fn inspect(repo: &Path) -> Result<AdminSummary> {
    ensure_repo(repo)?;

    let tags = lines(&run_git_string(repo, &["tag", "--list"])?);

    let mut remotes = Vec::new();
    for name in lines(&run_git_string(repo, &["remote"])?) {
        let url = git_opt(repo, &["remote", "get-url", "--", name.as_str()])?.unwrap_or_default();
        remotes.push(RemoteSummary {
            name,
            url: sanitize_url(&url),
        });
    }

    let gm = repo.join(".gitmodules");
    let modules = if gm.is_file() {
        let gm_arg = gm.to_string_lossy().to_string();
        git_opt(
            repo,
            &[
                "config",
                "--file",
                &gm_arg,
                "--get-regexp",
                r"^submodule\..*\.path$",
            ],
        )?
    } else {
        None
    };

    let mut states: HashMap<String, String> = HashMap::new();
    if let Some(status) = git_opt(repo, &["submodule", "status", "--recursive"])? {
        for line in status.lines() {
            let mut it = line.split_whitespace();
            if let (Some(head), Some(path)) = (it.next(), it.next()) {
                let state = match head.chars().next() {
                    Some('-') => "not initialized",
                    Some('+') => "out of date",
                    Some('U') => "conflict",
                    _ => "initialized",
                };
                states.insert(path.to_string(), state.to_string());
            }
        }
    }

    let mut submodules = Vec::new();
    if let Some(modules) = modules {
        for line in modules.lines() {
            if let Some((_, path)) = line.split_once(' ') {
                let status = states
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                submodules.push(SubmoduleSummary {
                    path: path.to_string(),
                    status,
                });
            }
        }
    }

    let sparse_enabled = matches!(
        git_opt(repo, &["config", "--bool", "core.sparseCheckout"])?.as_deref(),
        Some("true")
    );
    let lfs_installed =
        git_opt(repo, &["config", "--local", "--get", "filter.lfs.clean"])?.is_some();

    let commit_signed = matches!(
        git_opt(repo, &["config", "--local", "--bool", "commit.gpgsign"])?.as_deref(),
        Some("true")
    );
    let tag_signed = matches!(
        git_opt(repo, &["config", "--local", "--bool", "tag.gpgsign"])?.as_deref(),
        Some("true")
    );

    Ok(AdminSummary {
        tags,
        remotes,
        submodules,
        sparse_enabled,
        lfs_installed,
        signing: SigningSummary {
            enabled: commit_signed || tag_signed,
            format: git_opt(repo, &["config", "--local", "--get", "gpg.format"])?,
            key_set: git_opt(repo, &["config", "--local", "--get", "user.signingkey"])?.is_some(),
        },
    })
}

fn tag(repo: &Path, action: &TagAction) -> Result<String> {
    match action {
        TagAction::Create { name, rev, message } => {
            check_tag_name(repo, name)?;
            let mut args = vec!["tag"];
            if let Some(message) = message {
                args.extend(["-a", "-m", message]);
            }
            args.extend(["--", name]);
            if let Some(rev) = rev {
                check_exact_rev(rev)?;
                args.push(rev);
            }
            run_git_string(repo, &args)
        }
        TagAction::Delete { name } => {
            check_tag_name(repo, name)?;
            run_git_string(repo, &["tag", "--delete", "--", name.as_str()])
        }
        TagAction::Push { name, remote } => {
            check_tag_name(repo, name)?;
            check_remote_name(remote)?;
            ensure_remote_exists(repo, remote)?;
            let tag_ref = format!("refs/tags/{name}");
            crate::git_remotes::network(
                repo,
                &["push", "--", remote.as_str(), &tag_ref],
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            Ok("已推送标签".to_owned())
        }
    }
}

fn remote(repo: &Path, action: &RemoteAction) -> Result<String> {
    match action {
        RemoteAction::Add { name, url } => {
            check_remote_name(name)?;
            check_url(url)?;
            let existing = run_git_string(repo, &["remote"])?;
            if existing
                .lines()
                .any(|l| l.trim().eq_ignore_ascii_case(name))
            {
                bail!("remote already exists: {name}; use change-url instead of overwriting");
            }
            run_git_string(repo, &["remote", "add", "--", name.as_str(), url.as_str()])
                .map_err(|_| anyhow::anyhow!("添加远端失败"))
        }
        RemoteAction::ChangeUrl { name, url } => {
            check_remote_name(name)?;
            check_url(url)?;
            ensure_remote_exists(repo, name)?;
            run_git_string(
                repo,
                &["remote", "set-url", "--", name.as_str(), url.as_str()],
            )
            .map_err(|_| anyhow::anyhow!("更新远端地址失败"))
        }
        RemoteAction::Delete { name } => {
            check_remote_name(name)?;
            ensure_remote_exists(repo, name)?;
            run_git_string(repo, &["remote", "remove", "--", name.as_str()])
        }
    }
}

fn submodule(repo: &Path, action: &SubmoduleAction) -> Result<String> {
    match action {
        SubmoduleAction::Add { destination, url } => {
            literal_file(destination)?;
            check_url(url)?;
            run_git_string(
                repo,
                &[
                    "-c",
                    "protocol.ext.allow=never",
                    "submodule",
                    "add",
                    "--",
                    url,
                    destination,
                ],
            )
            .map_err(|_| anyhow::anyhow!("添加子模块失败"))
        }
        SubmoduleAction::Update => run_git_string(
            repo,
            &[
                "-c",
                "protocol.ext.allow=never",
                "submodule",
                "update",
                "--init",
                "--recursive",
            ],
        )
        .map_err(|_| anyhow::anyhow!("更新子模块失败")),
        SubmoduleAction::Sync => run_git_string(repo, &["submodule", "sync", "--recursive"]),
        SubmoduleAction::Remove { path } => {
            literal_file(path)?;
            let mut out = run_git_string(
                repo,
                &["--literal-pathspecs", "submodule", "deinit", "--", path],
            )?;
            out.push_str(&run_git_string(
                repo,
                &["--literal-pathspecs", "rm", "--", path],
            )?);
            Ok(out)
        }
    }
}

fn sparse(repo: &Path, action: &SparseAction) -> Result<String> {
    match action {
        SparseAction::Set { patterns } => {
            if patterns.is_empty() {
                bail!("at least one sparse path is required");
            }
            let mut input = String::new();
            for pattern in patterns {
                ensure!(
                    !pattern.is_empty() && !pattern.contains(['\0', '\r', '\n']),
                    "无效 sparse 模式"
                );
                input.push_str(pattern);
                input.push('\n');
            }
            let out = index_git(
                repo,
                &["sparse-checkout", "set", "--no-cone", "--stdin"],
                Some(input.into_bytes()),
                None,
            )?;
            Ok(String::from_utf8_lossy(&out).into_owned())
        }
        SparseAction::Disable => run_git_string(repo, &["sparse-checkout", "disable"]),
    }
}

fn lfs(repo: &Path, action: &LfsAction) -> Result<String> {
    match action {
        LfsAction::InstallLocal => run_git_string(repo, &["lfs", "install", "--local"]),
        LfsAction::Pull => run_git_string(repo, &["lfs", "pull"])
            .map_err(|_| anyhow::anyhow!("Git LFS 拉取失败，请确认 git-lfs 已安装并检查网络")),
        LfsAction::Track { patterns } => {
            if patterns.is_empty() {
                bail!("at least one LFS pattern is required");
            }
            for pattern in patterns {
                check_pattern(pattern)?;
            }
            let mut args = vec!["lfs", "track", "--"];
            args.extend(patterns.iter().map(String::as_str));
            run_git_string(repo, &args)
        }
    }
}

fn signing(repo: &Path, enabled: bool, format: Option<&str>, key: Option<&str>) -> Result<String> {
    if let Some(format) = format {
        ensure!(
            matches!(format, "openpgp" | "x509" | "ssh"),
            "签名格式应为 openpgp、x509 或 ssh"
        );
    }
    if let Some(key) = key {
        ensure!(
            !key.is_empty() && !key.starts_with('-') && !key.chars().any(char::is_control),
            "无效签名密钥"
        );
    }
    let mut out = String::new();
    if let Some(format) = format {
        out.push_str(&run_git_string(
            repo,
            &["config", "--local", "gpg.format", format],
        )?);
    }
    if let Some(key) = key {
        out.push_str(&run_git_string(
            repo,
            &["config", "--local", "user.signingkey", key],
        )?);
    }
    let value = if enabled { "true" } else { "false" };
    out.push_str(&run_git_string(
        repo,
        &["config", "--local", "commit.gpgsign", value],
    )?);
    out.push_str(&run_git_string(
        repo,
        &["config", "--local", "tag.gpgsign", value],
    )?);
    Ok(out)
}

fn ensure_repo(repo: &Path) -> Result<()> {
    if !repo.is_dir() {
        bail!("repository path is not a directory: {}", repo.display());
    }
    run_git_string(repo, &["rev-parse", "--git-dir"]).context("not a git repository")?;
    Ok(())
}

fn check_tag_name(repo: &Path, name: &str) -> Result<()> {
    if name.is_empty()
        || name.starts_with('-')
        || name.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        bail!("invalid tag name: {name}");
    }
    let full = format!("refs/tags/{name}");
    run_git_string(repo, &["check-ref-format", &full])
        .with_context(|| format!("invalid tag name: {name}"))?;
    Ok(())
}

fn check_remote_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 100
        || name.starts_with('-')
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("invalid remote name: {name}");
    }
    Ok(())
}

fn check_exact_rev(rev: &str) -> Result<()> {
    let full_hex =
        (rev.len() == 40 || rev.len() == 64) && rev.bytes().all(|b| b.is_ascii_hexdigit());
    if full_hex {
        return Ok(());
    }
    let forbidden = |c: char| {
        c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
    };
    if rev.starts_with("refs/")
        && !rev.starts_with('-')
        && !rev.contains("..")
        && !rev.chars().any(forbidden)
    {
        return Ok(());
    }
    bail!("revision must be a full refs/... name or a full hexadecimal object id");
}

fn ensure_remote_exists(repo: &Path, name: &str) -> Result<()> {
    if git_opt(repo, &["remote", "get-url", "--", name])?.is_none() {
        bail!("remote does not exist: {name}");
    }
    Ok(())
}

fn check_url(url: &str) -> Result<()> {
    if url.is_empty() || url.len() > 4096 {
        bail!("invalid URL");
    }
    if url.starts_with('-') || url.chars().any(char::is_control) {
        bail!("invalid URL characters");
    }
    ensure!(
        !url.starts_with("ext::"),
        "远端地址含不支持的协议或查询参数"
    );
    if let Some((scheme, rest)) = url.split_once("://") {
        if matches!(scheme, "https" | "http") {
            ensure!(
                !rest.split('/').next().unwrap_or("").contains('@'),
                "HTTPS 凭据请通过 Git 凭据管理设置"
            );
        }
    }
    Ok(())
}

fn sanitize_url(raw: &str) -> String {
    let url = raw.trim().split(['?', '#']).next().unwrap_or("");
    if let Some((scheme, rest)) = url.split_once("://") {
        let end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..end].rfind('@') {
            return format!("{scheme}://{}", &rest[at + 1..]);
        }
    }
    url.to_owned()
}

fn check_pattern(pattern: &str) -> Result<()> {
    if pattern.is_empty()
        || pattern.len() > 1024
        || pattern.starts_with('-')
        || pattern.chars().any(char::is_control)
        || Path::new(pattern).is_absolute()
        || pattern
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
    {
        bail!("invalid pattern: {pattern}");
    }
    Ok(())
}

fn git_opt(repo: &Path, args: &[&str]) -> Result<Option<String>> {
    match run_git_string(repo, args) {
        Ok(out) => {
            let trimmed = out.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(_) => Ok(None),
    }
}

fn lines(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn admin_tags_sparse_and_signing_apply_to_real_repository() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let repo = std::env::temp_dir().join(format!("nemorous-admin-{}-{id}", std::process::id()));
        fs::create_dir(&repo).unwrap();
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(repo.clone());
        let git = |args: &[&str]| run_git_string(&repo, args).unwrap().trim().to_owned();
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Nemorous QA"]);
        git(&["config", "user.email", "qa@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        for path in ["keep", "omit"] {
            fs::create_dir(repo.join(path)).unwrap();
            fs::write(repo.join(path).join("fixture"), path).unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-m", "baseline"]);
        run(
            &repo,
            &AdminAction::Tag(TagAction::Create {
                name: "v1".into(),
                rev: None,
                message: Some("annotated".into()),
            }),
        )
        .unwrap();
        assert_eq!(git(&["cat-file", "-t", "v1"]), "tag");
        run(
            &repo,
            &AdminAction::Remote(RemoteAction::Add {
                name: "origin".into(),
                url: "https://example.invalid/project".into(),
            }),
        )
        .unwrap();
        run(
            &repo,
            &AdminAction::Remote(RemoteAction::ChangeUrl {
                name: "origin".into(),
                url: "https://example.invalid/updated".into(),
            }),
        )
        .unwrap();
        run(
            &repo,
            &AdminAction::Sparse(SparseAction::Set {
                patterns: vec!["/keep/".into()],
            }),
        )
        .unwrap();
        assert!(repo.join("keep").join("fixture").exists());
        assert!(!repo.join("omit").join("fixture").exists());
        run(
            &repo,
            &AdminAction::Signing {
                enabled: true,
                format: Some("ssh".into()),
                key: Some("test-public-key".into()),
            },
        )
        .unwrap();
        assert_eq!(git(&["config", "--local", "gpg.format"]), "ssh");
        assert_eq!(git(&["config", "--local", "commit.gpgsign"]), "true");
        let summary = inspect(&repo).unwrap();
        assert_eq!(summary.tags, ["v1"]);
        assert_eq!(summary.remotes[0].url, "https://example.invalid/updated");
        assert!(summary.signing.enabled);
        assert!(summary.sparse_enabled);
        run(&repo, &AdminAction::Sparse(SparseAction::Disable)).unwrap();
        assert!(repo.join("omit").join("fixture").exists());
    }
}
