use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

use crate::git::RepoInfo;

const YAML_LIMIT: u64 = 1024 * 1024;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct Manifest {
    pub tasks: BTreeMap<String, Task>,
    projects: BTreeMap<String, Project>,
}

#[derive(Default, Deserialize, Clone)]
#[serde(default)]
struct Project {
    path: String,
}

#[derive(Default, Deserialize, Clone)]
#[serde(default)]
pub(crate) struct Task {
    pub desc: String,
    cmd: String,
    env: Value,
    target: Value,
}

pub(crate) fn load(root: &Path) -> Result<Manifest> {
    let path = ["mani.yaml", "mani.yml"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file());
    let Some(path) = path else {
        return Ok(Manifest::default());
    };
    let mut data = Vec::new();
    File::open(path)?
        .take(YAML_LIMIT + 1)
        .read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= YAML_LIMIT, "Mani 配置超过大小限制");
    Ok(serde_yaml_ng::from_slice::<Option<Manifest>>(&data)?.unwrap_or_default())
}

fn env_pairs(value: &Value) -> Result<BTreeMap<String, String>> {
    let result = match value {
        Value::Null => BTreeMap::new(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                )
            })
            .collect(),
        Value::Array(items) => {
            let mut result = BTreeMap::new();
            for item in items {
                let (key, val) = item
                    .as_str()
                    .and_then(|s| s.split_once('='))
                    .context("Mani 环境变量需要 K=V")?;
                result.insert(key.to_owned(), val.to_owned());
            }
            result
        }
        _ => anyhow::bail!("Mani 环境变量需要映射或列表"),
    };
    ensure!(
        result
            .iter()
            .all(|(key, val)| !key.is_empty() && !key.contains(['=', '\0']) && !val.contains('\0')),
        "Mani 环境变量无效"
    );
    Ok(result)
}

pub(crate) fn plan(
    root: &Path,
    manifest: &Manifest,
    name: &str,
    repos: &[RepoInfo],
    scope: Option<&Path>,
) -> Result<Vec<(String, Command)>> {
    let task = manifest.tasks.get(name).context("Mani 任务已经变化")?;
    ensure!(
        !task.cmd.trim().is_empty() && !task.cmd.contains('\0'),
        "Mani 命令为空或无效"
    );
    let env = env_pairs(&task.env)?;
    let root = fs::canonicalize(root)?;
    let cwd = task.target == "cwd" || task.target.get("cwd").and_then(Value::as_bool) == Some(true);
    let targets: Vec<(String, PathBuf)> = if let Some(scope) = scope {
        let dir = fs::canonicalize(scope)?;
        let name = repos
            .iter()
            .find(|repo| fs::canonicalize(&repo.path).ok().as_ref() == Some(&dir))
            .map(|repo| repo.name.clone())
            .context("Mani 项目已变化")?;
        vec![(name, dir)]
    } else if cwd {
        vec![("workspace (cwd)".into(), root.clone())]
    } else {
        let listed = manifest
            .projects
            .iter()
            .filter_map(|(name, project)| {
                let dir = root.join(if project.path.is_empty() {
                    name
                } else {
                    &project.path
                });
                let canonical = fs::canonicalize(dir).ok()?;
                repos
                    .iter()
                    .any(|repo| fs::canonicalize(&repo.path).ok().as_ref() == Some(&canonical))
                    .then(|| (name.clone(), canonical))
            })
            .collect::<Vec<_>>();
        if listed.is_empty() {
            repos
                .iter()
                .map(|repo| (repo.name.clone(), PathBuf::from(&repo.path)))
                .collect()
        } else {
            listed
        }
    };
    targets
        .into_iter()
        .map(|(name, dir)| {
            let dir = fs::canonicalize(dir)?;
            ensure!(
                dir.is_dir() && dir.starts_with(&root),
                "Mani 项目不在当前工作区"
            );
            let mut cmd = Command::new("bash");
            cmd.args(["-c", &task.cmd])
                .current_dir(&dir)
                .env("MANI_PROJECT_NAME", &name)
                .env("MANI_PROJECT_PATH", dir)
                .envs(&env);
            Ok((name, cmd))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_preserves_block_commands_and_root_task_environment() {
        let root = std::env::temp_dir().join(format!("nemorous-mani-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("mani.yml"), "tasks:\n  inspect:\n    target: {cwd: true}\n    env: {A: 'x y'}\n    cmd: |\n      printf '%s' \"$A\"\n      pwd\n").unwrap();
        let manifest = load(&root).unwrap();
        let target = plan(&root, &manifest, "inspect", &[], None).unwrap();
        assert_eq!(target.len(), 1);
        assert_eq!(target[0].0, "workspace (cwd)");
        assert_eq!(
            target[0].1.get_args().last().unwrap(),
            "printf '%s' \"$A\"\npwd\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
