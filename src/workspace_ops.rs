use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};

use crate::git::{discover_workspace_repos, run_git_string};

pub(crate) fn workspace_path(path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize().context("无法打开工作区目录")?;
    ensure!(path.is_dir(), "请选择工作区目录");
    ensure!(path.to_str().is_some(), "工作区路径必须是 UTF-8");
    Ok(path)
}

fn target(workspace: &Path, name: &str) -> Result<(PathBuf, PathBuf)> {
    let mut components = Path::new(name).components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(value)) if value == name)
            && components.next().is_none()
            && !name.starts_with('.')
            && !name.contains(['\r', '\n', '\0']),
        "请输入单个有效的仓库目录名"
    );
    let workspace = workspace_path(workspace)?;
    let path = workspace.join(name);
    match fs::symlink_metadata(&path) {
        Ok(_) => bail!("目标目录已经存在"),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).context("无法检查目标目录"),
    }
    Ok((workspace, path))
}

pub(crate) fn clone_repo(workspace: &Path, source: &str, name: &str) -> Result<PathBuf> {
    ensure!(!source.trim().is_empty(), "请输入仓库 URL 或本地路径");
    let (workspace, path) = target(workspace, name)?;
    // The Git client can echo URL userinfo on failure; surface a status without the URL.
    let outcome = crate::tool_command::command("git")
        .args(["-c", "protocol.ext.allow=never", "clone", "--", source])
        .arg(&path)
        .current_dir(&workspace)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("无法启动 Git 克隆")?;
    ensure!(
        outcome.success(),
        "Git 克隆失败（退出码 {:?}）",
        outcome.code()
    );
    Ok(path)
}

pub(crate) fn init_repo(workspace: &Path, name: &str, branch: &str) -> Result<PathBuf> {
    let (workspace, path) = target(workspace, name)?;
    let branch = format!("--initial-branch={branch}");
    run_git_string(
        &workspace,
        &[
            "init",
            &branch,
            "--",
            path.to_str().context("无效仓库路径")?,
        ],
    )?;
    Ok(path)
}

pub(crate) fn import_repo(workspace: &Path, repo: &Path) -> Result<PathBuf> {
    let workspace = workspace_path(workspace)?;
    let repo = workspace_path(repo)?;
    let root = run_git_string(&repo, &["rev-parse", "--show-toplevel"])?;
    ensure!(
        Path::new(root.strip_suffix('\n').unwrap_or(&root)).canonicalize()? == repo,
        "请选择 Git 仓库的根目录"
    );
    let workspace_str = workspace.to_str().context("无效工作区路径")?;
    let existing = discover_workspace_repos(workspace_str, &[]);
    if existing
        .iter()
        .any(|item| Path::new(&item.path).canonicalize().ok().as_ref() == Some(&repo))
    {
        return Ok(repo);
    }
    let rel = repo
        .strip_prefix(&workspace)
        .unwrap_or(&repo)
        .to_str()
        .context("无效仓库路径")?;
    let name = repo
        .file_name()
        .and_then(|part| part.to_str())
        .context("无效仓库名")?;
    ensure!(
        !existing.iter().any(|item| item.name == name),
        "已有同名项目，请改用其他目录名"
    );
    let valid_scalar =
        |value: &str| !value.contains(['\r', '\n', '\0', '"', '\\']) && !value.contains('\'');
    ensure!(
        valid_scalar(name) && valid_scalar(rel),
        "mani 项目路径含无法安全表示的字符"
    );

    let yaml = workspace.join("mani.yaml");
    let yml = workspace.join("mani.yml");
    let file = if !yaml.exists() && yml.exists() {
        yml
    } else {
        yaml
    };
    let original = match fs::symlink_metadata(&file) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.is_symlink() && meta.len() < 2 * 1024 * 1024,
                "无法编辑现有 mani 清单"
            );
            Some(fs::read_to_string(&file)?)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).context("无法检查 mani 清单"),
    };
    let next = append_project(original.as_deref().unwrap_or(""), name, rel)?;
    let tmp = workspace.join(".mani.nemorous.lock");
    let mut write = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .context("另一个 mani 清单写入正在进行")?;
    let installed = (|| -> Result<()> {
        write.write_all(next.as_bytes())?;
        if let Ok(meta) = fs::symlink_metadata(&file) {
            ensure!(meta.is_file() && !meta.is_symlink(), "mani 清单类型已变化");
            write.set_permissions(meta.permissions())?;
        }
        write.sync_all()?;
        drop(write);
        let current = match fs::read_to_string(&file) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        ensure!(current == original, "mani 清单已在外部修改，请刷新后再导入");
        fs::rename(&tmp, &file)?;
        Ok(())
    })();
    if installed.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    installed?;
    Ok(repo)
}

fn append_project(original: &str, name: &str, path: &str) -> Result<String> {
    let eol = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut projects = false;
    let mut encountered = false;
    let mut insertion = original.len();
    let mut segment = None;
    let mut offset = 0;
    for line in original.split_inclusive('\n') {
        let trimmed = line.trim();
        let starts_at_root =
            !line.starts_with([' ', '\t']) && !trimmed.is_empty() && !trimmed.starts_with('#');
        if starts_at_root {
            if projects && insertion == original.len() {
                insertion = offset;
            }
            projects = trimmed.starts_with("projects:");
            if projects {
                ensure!(!encountered, "mani 清单有重复 projects 段");
                encountered = true;
                let value = trimmed
                    .strip_prefix("projects:")
                    .unwrap_or_default()
                    .trim_start();
                ensure!(
                    value.is_empty() || value.starts_with('#') || value == "{}",
                    "mani 的 projects 格式无法安全追加"
                );
                if value == "{}" {
                    let begin = offset + line.find("{}").context("无效 mani 清单")?;
                    segment = Some(begin..begin + 2);
                }
            }
        } else if projects {
            let raw = line.trim_end();
            if raw.starts_with("  ") && !raw.starts_with("   ") && trimmed.ends_with(':') {
                let key = trimmed
                    .trim_end_matches(':')
                    .trim()
                    .trim_matches(['\'', '"']);
                ensure!(key != name, "mani 清单已有同名项目");
            }
        }
        offset += line.len();
    }
    let mut next = String::with_capacity(original.len() + name.len() + path.len() + 30);
    next.push_str(&original[..insertion]);
    if let Some(segment) = segment {
        next.replace_range(segment, "");
    }
    if !next.is_empty() && !next.ends_with('\n') {
        next.push_str(eol);
    }
    if !encountered {
        next.push_str("projects:");
        next.push_str(eol);
    }
    next.push_str(&format!("  \"{name}\":{eol}    path: \"{path}\"{eol}"));
    next.push_str(&original[insertion..]);
    Ok(next)
}

pub(crate) fn launch_window(workspace: &Path) -> Result<()> {
    let workspace = workspace_path(workspace)?;
    let mut instance = Command::new(std::env::current_exe()?)
        .current_dir(&workspace)
        .env("NEMOROUS_WORKSPACE", &workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法打开 Nemorous 新窗口")?;
    std::thread::spawn(move || {
        let _ = instance.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_init_clone_import_preserve_manifest_and_find_nested_root() {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("nemorous-workspaces-{}-{id}", std::process::id()));
        fs::create_dir(&root).unwrap();
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(root.clone());
        let workspace = init_repo(&root, "工作区", "main").unwrap();
        let source = init_repo(&root, "源仓", "main").unwrap();
        fs::create_dir(workspace.join("nested")).unwrap();
        let nested = init_repo(&workspace.join("nested"), "仓", "main").unwrap();
        let cloned = clone_repo(&workspace, source.to_str().unwrap(), "cloned").unwrap();
        assert!(cloned.join(".git").exists());
        let original = "# keep comment\r\nprojects:\r\n  root:\r\n    path: .\r\n    desc: keep me\r\n    url: keep URL\r\ntasks:\r\n  check: git status\r\n";
        let manifest = workspace.join("mani.yml");
        fs::write(&manifest, original).unwrap();
        import_repo(&workspace, &nested).unwrap();
        import_repo(&workspace, &source).unwrap();
        let changed = fs::read_to_string(&manifest).unwrap();
        assert!(changed.starts_with("# keep comment\r\nprojects:\r\n  root:\r\n    path: .\r\n    desc: keep me\r\n    url: keep URL\r\n"));
        assert!(changed.ends_with("tasks:\r\n  check: git status\r\n"));
        assert!(changed.contains("path: \"nested/仓\""));
        let repos = discover_workspace_repos(workspace.to_str().unwrap(), &[]);
        assert_eq!(repos.len(), 4);
        assert!(
            repos
                .iter()
                .any(|repo| repo.is_root && repo.id == "root" && repo.desc == "keep me")
        );
        import_repo(&workspace, &source).unwrap();
        assert_eq!(fs::read_to_string(manifest).unwrap(), changed);
        assert!(init_repo(&workspace, "../outside", "main").is_err());
    }
}
