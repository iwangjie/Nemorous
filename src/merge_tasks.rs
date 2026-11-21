use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::git::run_git_string;
use crate::git_index::{index_git, oid_ok};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MergeStrategy {
    Default,
    NoFf,
    FastForward,
    Squash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MergeTask {
    pub id: String,
    pub repo: PathBuf,
    pub source: String,
    pub target: String,
    pub source_oid: String,
    pub target_oid: String,
    pub strategy: MergeStrategy,
    pub original_conflicts: Vec<String>,
    #[serde(default)]
    protected_entries: Option<String>,
    #[serde(default)]
    expected_tree: Option<String>,
}

#[derive(Debug)]
pub(crate) enum BeginMerge {
    Waiting(MergeTask),
    Complete(String),
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(run_git_string(repo, args)?
        .trim_end_matches('\n')
        .to_owned())
}

fn branch_ref(repo: &Path, name: &str) -> Result<String> {
    ensure!(!name.is_empty() && !name.starts_with('-'), "无效分支名");
    let reference = format!("refs/heads/{name}");
    git(repo, &["check-ref-format", &reference])?;
    Ok(reference)
}

fn resolve(repo: &Path, spec: &str) -> Result<String> {
    let hash = git(repo, &["rev-parse", "--verify", "--end-of-options", spec])?;
    ensure!(oid_ok(&hash), "无法读取 Git 提交 {spec}");
    Ok(hash)
}

fn active_branch(repo: &Path) -> Result<String> {
    git(repo, &["rev-parse", "--symbolic-full-name", "HEAD"])
}

fn unique_id() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("merge-{}-{time}", std::process::id())
}

fn task_id_ok(id: &str) -> bool {
    id.starts_with("merge-")
        && id
            .bytes()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, b'-' | b'm' | b'e' | b'r' | b'g'))
}

impl MergeTask {
    pub(crate) fn worktree(&self, root: &Path) -> PathBuf {
        root.join(&self.id).join("worktree")
    }

    fn manifest(&self, root: &Path) -> PathBuf {
        root.join(&self.id).join("task.json")
    }
}

struct TaskLock {
    _file: File,
}

impl TaskLock {
    fn acquire(root: &Path, task: &MergeTask) -> Result<Self> {
        ensure!(task_id_ok(&task.id), "合并任务 ID 无效");
        let lock = root.join(&task.id).join("operation.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)?;
        file.try_lock().context("合并任务正在由另一个窗口处理")?;
        Ok(Self { _file: file })
    }
}

fn remove_task_directory(root: &Path, id: &str) {
    let directory = root.join(id);
    let _ = fs::remove_file(directory.join("operation.lock"));
    let _ = fs::remove_dir(directory);
}

fn persist(root: &Path, task: &MergeTask) -> Result<()> {
    let manifest = task.manifest(root);
    let next = manifest.with_extension(format!("{}-pending", unique_id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&next)?;
    let result = (|| -> Result<()> {
        file.write_all(&serde_json::to_vec_pretty(task)?)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&next, &manifest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(next);
    }
    result
}

pub(crate) fn list(root: &Path, repos: &[PathBuf]) -> Vec<MergeTask> {
    let repos = repos
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .collect::<Vec<_>>();
    let Ok(contents) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut tasks = contents
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !task_id_ok(name) || !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let manifest = entry.path().join("task.json");
            if fs::symlink_metadata(&manifest).ok()?.len() > 1_048_576 {
                return None;
            }
            let task: MergeTask = serde_json::from_slice(&fs::read(manifest).ok()?).ok()?;
            (task.id == name && repos.contains(&task.repo) && task.worktree(root).is_dir())
                .then_some(task)
        })
        .collect::<Vec<_>>();
    tasks.sort_by(|a, b| a.repo.cmp(&b.repo).then_with(|| a.id.cmp(&b.id)));
    tasks
}

pub(crate) fn unresolved(repo: &Path) -> Result<Vec<String>> {
    let raw = index_git(repo, &["ls-files", "--unmerged", "-z"], None, None)?;
    let mut paths = std::collections::BTreeSet::new();
    for record in String::from_utf8(raw)?
        .split('\0')
        .filter(|row| !row.is_empty())
    {
        let (_, path) = record.split_once('\t').context("无效的 Git 冲突条目")?;
        paths.insert(path.to_owned());
    }
    Ok(paths.into_iter().collect())
}

pub(crate) fn start(
    root: &Path,
    repo: &Path,
    source: &str,
    target: &str,
    strategy: MergeStrategy,
) -> Result<BeginMerge> {
    let repo = repo.canonicalize()?;
    let source_ref = branch_ref(&repo, source)?;
    let target_ref = branch_ref(&repo, target)?;
    ensure!(target_ref != source_ref, "请选择两个不同的分支");
    let source_oid = resolve(&repo, &source_ref)?;
    let target_oid = resolve(&repo, &target_ref)?;
    if source_oid == target_oid
        || git(
            &repo,
            &["merge-base", "--is-ancestor", &source_oid, &target_oid],
        )
        .is_ok()
    {
        return Ok(BeginMerge::Complete(target_oid));
    }

    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let root = root.as_path();
    let mut task = MergeTask {
        id: unique_id(),
        repo,
        source: source.to_owned(),
        target: target.to_owned(),
        source_oid,
        target_oid,
        strategy,
        original_conflicts: Vec::new(),
        protected_entries: None,
        expected_tree: None,
    };
    fs::create_dir(root.join(&task.id))?;
    let lock = TaskLock::acquire(root, &task)?;
    persist(root, &task)?;
    let path = task.worktree(root);
    let path_arg = path.to_str().context("工作树目录必须是 UTF-8")?;
    if let Err(err) = git(
        &task.repo,
        &[
            "worktree",
            "add",
            "--detach",
            "--",
            path_arg,
            &task.target_oid,
        ],
    ) {
        let _ = fs::remove_file(task.manifest(root));
        drop(lock);
        remove_task_directory(root, &task.id);
        return Err(err);
    }

    let mut args = vec!["-c", "merge.conflictStyle=diff3", "merge", "--no-edit"];
    match strategy {
        MergeStrategy::Default => {}
        MergeStrategy::NoFf => args.push("--no-ff"),
        MergeStrategy::FastForward => args.push("--ff-only"),
        MergeStrategy::Squash => args.push("--squash"),
    }
    args.extend(["--", &task.source_oid]);
    let merged = git(&path, &args);
    task.original_conflicts = unresolved(&path)?;
    if !task.original_conflicts.is_empty() {
        task.protected_entries = Some(protected_fingerprint(&path, &task.original_conflicts)?);
        persist(root, &task)?;
        return Ok(BeginMerge::Waiting(task));
    }
    if let Err(err) = merged {
        if git(&path, &["status", "--porcelain", "-z", "-uall"])?.is_empty() {
            remove_worktree(root, &task, false)?;
            drop(lock);
            remove_task_directory(root, &task.id);
        }
        return Err(err).context("隔离合并失败");
    }
    drop(lock);
    finish(root, &task).map(BeginMerge::Complete)
}

fn verify_worktree(root: &Path, task: &MergeTask) -> Result<PathBuf> {
    ensure!(
        task_id_ok(&task.id) && oid_ok(&task.target_oid) && oid_ok(&task.source_oid),
        "合并任务记录无效"
    );
    let tree = task.worktree(root).canonicalize()?;
    let owner = root.join(&task.id).canonicalize()?;
    ensure!(
        tree.parent() == Some(owner.as_path()),
        "合并工作树路径已变化"
    );
    ensure!(
        Path::new(&git(&tree, &["rev-parse", "--show-toplevel"])?).canonicalize()? == tree,
        "合并工作树根路径已变化"
    );
    let common = |repo: &Path| -> Result<PathBuf> {
        let path = git(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        Ok(Path::new(&path).canonicalize()?)
    };
    ensure!(
        common(&tree)? == common(&task.repo)?,
        "合并工作树属于其他仓库"
    );
    Ok(tree)
}

fn verify_no_markers(tree: &Path, paths: &[String]) -> Result<()> {
    for path in paths {
        crate::git_index::literal_file(path)?;
        let full = tree.join(path);
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() && meta.len() <= crate::git::MAX_REVIEW_BYTES as u64 => meta,
            Ok(_) => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        if meta.is_file() {
            let bytes = fs::read(full)?;
            if bytes.contains(&0) {
                continue;
            }
            if std::str::from_utf8(&bytes).is_ok()
                && !crate::conflict_text::parse(&bytes)?.is_empty()
            {
                bail!("文件仍含有冲突标记：{path}");
            }
        }
    }
    Ok(())
}

fn verify_parents(tree: &Path, task: &MergeTask, result: &str) -> Result<()> {
    if result == task.target_oid {
        ensure!(
            git(
                tree,
                &["merge-base", "--is-ancestor", &task.source_oid, result]
            )
            .is_ok(),
            "合并结果没有包含来源分支"
        );
        return Ok(());
    }
    if result == task.source_oid
        && matches!(
            task.strategy,
            MergeStrategy::Default | MergeStrategy::FastForward
        )
    {
        git(
            tree,
            &["merge-base", "--is-ancestor", &task.target_oid, result],
        )
        .context("快进结果不包含目标分支基线")?;
        return Ok(());
    }
    let parents = git(tree, &["show", "-s", "--format=%P", result])?;
    let expected = match task.strategy {
        MergeStrategy::Squash => task.target_oid.clone(),
        MergeStrategy::FastForward => bail!("快进产生了意外的提交"),
        _ => format!("{} {}", task.target_oid, task.source_oid),
    };
    ensure!(parents == expected, "合并结果的父提交与任务不符");
    Ok(())
}

fn protected_fingerprint(repo: &Path, allowed: &[String]) -> Result<String> {
    let bytes = crate::git::run_git_bytes(repo, &["ls-files", "--stage", "-z"])?;
    let mut hash = Sha256::new();
    for entry in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let tab = entry
            .iter()
            .position(|byte| *byte == b'\t')
            .context("索引条目缺少路径")?;
        let path = &entry[tab + 1..];
        if !allowed.iter().any(|allowed| path == allowed.as_bytes()) {
            hash.update(entry);
            hash.update([0]);
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn finish(root: &Path, task: &MergeTask) -> Result<String> {
    let lock = TaskLock::acquire(root, task)?;
    let saved: MergeTask = serde_json::from_slice(&fs::read(task.manifest(root))?)?;
    ensure!(
        saved.id == task.id
            && saved.repo == task.repo
            && saved.source_oid == task.source_oid
            && saved.target_oid == task.target_oid,
        "合并任务已变化，请重新打开"
    );
    let task = &saved;
    let tree = verify_worktree(root, task)?;
    ensure!(unresolved(&tree)?.is_empty(), "请先处理所有冲突文件");
    verify_no_markers(&tree, &task.original_conflicts)?;
    if let Some(expected) = &task.protected_entries {
        ensure!(
            protected_fingerprint(&tree, &task.original_conflicts)? == *expected,
            "隔离工作树的非冲突索引条目已变化"
        );
    }
    let head = resolve(&tree, "HEAD")?;
    let mut persisted = task.clone();
    if head == task.target_oid {
        let merge_head = resolve(&tree, "MERGE_HEAD").ok();
        if task.strategy == MergeStrategy::Squash || merge_head.is_some() {
            ensure!(
                task.strategy == MergeStrategy::Squash
                    || merge_head.as_deref() == Some(&task.source_oid),
                "工作树正在合并其他提交"
            );
            ensure!(
                git(&tree, &["diff", "--name-only", "-z"])?.is_empty(),
                "隔离工作树含有未暂存改动"
            );
            persisted.expected_tree = Some(git(&tree, &["write-tree"])?);
            persist(root, &persisted)?;
            let message = format!("Merge {} into {}", task.source, task.target);
            git(&tree, &["commit", "--no-edit", "-m", &message])?;
        }
    }
    let result = resolve(&tree, "HEAD")?;
    verify_parents(&tree, task, &result)?;
    if let Some(expected) = &persisted.expected_tree {
        ensure!(
            resolve(&tree, &format!("{result}^{{tree}}"))? == *expected,
            "合并提交树与已审阅的暂存版本不同"
        );
    }
    ensure!(
        git(&tree, &["status", "--porcelain", "-z", "-uall"])?.is_empty(),
        "隔离工作树含有未保存到合并提交的文件"
    );

    let target_ref = branch_ref(&task.repo, &task.target)?;
    let current = resolve(&task.repo, &target_ref)?;
    if current != result {
        ensure!(
            current == task.target_oid,
            "目标分支在合并期间已变化，隔离工作树已保留"
        );
        promote(task, &target_ref, &result)?;
    }
    remove_worktree(root, task, false).context("目标分支已更新，隔离工作树清理失败")?;
    drop(lock);
    remove_task_directory(root, &task.id);
    Ok(result)
}

fn promote(task: &MergeTask, reference: &str, result: &str) -> Result<()> {
    let worktrees = git(&task.repo, &["worktree", "list", "--porcelain", "-z"])?;
    let mut located = None;
    for item in worktrees.split('\0') {
        if let Some(path) = item.strip_prefix("worktree ") {
            located = Some(Path::new(path));
        } else if item.strip_prefix("branch ") == Some(reference) {
            let checked = located.context("无法查找目标分支工作树")?.canonicalize()?;
            ensure!(checked == task.repo, "目标分支已在其他工作树检出");
            ensure!(
                active_branch(&task.repo)? == reference
                    && git(&task.repo, &["status", "--porcelain", "-z", "-uall"])?.is_empty(),
                "目标分支的主工作树含有改动"
            );
            git(&task.repo, &["merge", "--ff-only", "--no-edit", result])?;
            return Ok(());
        }
    }
    git(
        &task.repo,
        &[
            "update-ref",
            "-m",
            "Nemorous: isolated merge",
            reference,
            result,
            &task.target_oid,
        ],
    )?;
    Ok(())
}

fn remove_worktree(root: &Path, task: &MergeTask, force: bool) -> Result<()> {
    let tree = verify_worktree(root, task)?;
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", tree.to_str().context("无效的工作树路径")?]);
    git(&task.repo, &args)?;
    fs::remove_file(task.manifest(root))?;
    Ok(())
}

pub(crate) fn abort(root: &Path, task: &MergeTask) -> Result<()> {
    let lock = TaskLock::acquire(root, task)?;
    let result = remove_worktree(root, task, true);
    drop(lock);
    if result.is_ok() {
        remove_task_directory(root, &task.id);
    }
    result
}

pub(crate) fn push_target(
    task: &MergeTask,
    result: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    let local = branch_ref(&task.repo, &task.target)?;
    ensure!(resolve(&task.repo, &local)? == result, "目标分支已变化");
    let remote_key = format!("branch.{}.remote", task.target);
    let merge_key = format!("branch.{}.merge", task.target);
    let remote =
        git(&task.repo, &["config", "--get", &remote_key]).unwrap_or_else(|_| "origin".to_owned());
    let target =
        git(&task.repo, &["config", "--get", &merge_key]).unwrap_or_else(|_| local.clone());
    ensure!(
        !remote.starts_with('-') && target.starts_with("refs/heads/"),
        "目标分支的上游配置无效"
    );
    git(&task.repo, &["check-ref-format", &target])?;
    let spec = format!("{result}:{target}");
    crate::git_remotes::network(&task.repo, &["push", "--", &remote, &spec], cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(unique_id());
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn repo(&self) -> PathBuf {
            let repo = self.0.join("repo");
            git(&self.0, &["init", "-b", "main", repo.to_str().unwrap()]).unwrap();
            git(&repo, &["config", "user.name", "Nemorous QA"]).unwrap();
            git(&repo, &["config", "user.email", "qa@example.invalid"]).unwrap();
            git(&repo, &["config", "commit.gpgsign", "false"]).unwrap();
            fs::write(repo.join("ticket.txt"), b"base\n").unwrap();
            git(&repo, &["add", "."]).unwrap();
            git(&repo, &["commit", "-m", "base"]).unwrap();
            repo
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn isolated_merge_recovers_conflict_and_promotes_only_verified_parents() {
        let f = Fixture::new();
        let repo = f.repo();
        let tasks = f.0.join("tasks");
        git(&repo, &["switch", "-c", "release"]).unwrap();
        fs::write(repo.join("ticket.txt"), b"release\n").unwrap();
        git(&repo, &["commit", "-am", "release"]).unwrap();
        let release = resolve(&repo, "HEAD").unwrap();
        git(&repo, &["switch", "main"]).unwrap();
        fs::write(repo.join("ticket.txt"), b"feature\n").unwrap();
        git(&repo, &["commit", "-am", "feature"]).unwrap();
        let source = resolve(&repo, "HEAD").unwrap();
        let BeginMerge::Waiting(task) =
            start(&tasks, &repo, "main", "release", MergeStrategy::Default).unwrap()
        else {
            panic!("divergent changes should produce a conflict");
        };
        assert_eq!(task.original_conflicts, vec!["ticket.txt"]);
        assert_eq!(resolve(&repo, "refs/heads/release").unwrap(), release);
        assert_eq!(fs::read(repo.join("ticket.txt")).unwrap(), b"feature\n");

        let restored = list(&tasks, std::slice::from_ref(&repo)).pop().unwrap();
        let copy = restored.worktree(&tasks);
        let lock = TaskLock::acquire(&tasks, &restored).unwrap();
        assert!(TaskLock::acquire(&tasks, &restored).is_err());
        drop(lock);
        assert!(TaskLock::acquire(&tasks, &restored).is_ok());
        assert!(finish(&tasks, &restored).is_err());
        fs::write(copy.join("ticket.txt"), b"combined\n").unwrap();
        git(&copy, &["add", "ticket.txt"]).unwrap();
        fs::write(copy.join("unexpected"), "outside selected conflict").unwrap();
        git(&copy, &["add", "unexpected"]).unwrap();
        assert!(
            finish(&tasks, &restored)
                .unwrap_err()
                .to_string()
                .contains("非冲突")
        );
        assert_eq!(resolve(&repo, "refs/heads/release").unwrap(), release);
        git(&copy, &["rm", "-f", "--", "unexpected"]).unwrap();
        let merged = finish(&tasks, &restored).unwrap();
        assert_eq!(
            git(&repo, &["show", "-s", "--format=%P", &merged]).unwrap(),
            format!("{release} {source}")
        );
        assert_eq!(
            git(&repo, &["show", "release:ticket.txt"]).unwrap(),
            "combined"
        );
        assert_eq!(resolve(&repo, "HEAD").unwrap(), source);
        assert_eq!(fs::read(repo.join("ticket.txt")).unwrap(), b"feature\n");
        assert!(list(&tasks, &[repo]).is_empty());
    }
}
