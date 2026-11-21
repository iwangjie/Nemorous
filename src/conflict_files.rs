use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

use crate::conflict_text;
use crate::git::{self, LineEnding, MAX_REVIEW_BYTES, ReviewVersion};
use crate::git_index::{self, IndexEntry, IndexTransaction};

#[derive(Debug, Clone)]
pub(crate) struct ConflictFile {
    pub path: String,
    pub stages: [Option<IndexEntry>; 3],
    pub versions: [Option<Vec<u8>>; 3],
    pub working: Option<Vec<u8>>,
    pub ending: LineEnding,
    pub binary: bool,
    disk: ReviewVersion,
    permissions: Option<Permissions>,
}

#[derive(Debug, Clone)]
pub(crate) enum ResolveWith {
    Edited(Vec<u8>),
    Stage(usize),
    Delete,
}

pub(crate) fn stage_entries(repo: &Path, path: &str) -> Result<[Option<IndexEntry>; 3]> {
    git_index::literal_file(path)?;
    let raw = git_index::index_git(
        repo,
        &[
            "--literal-pathspecs",
            "ls-files",
            "--stage",
            "-z",
            "--",
            path,
        ],
        None,
        None,
    )?;
    let mut stages = [None, None, None];
    for record in String::from_utf8(raw)?
        .split('\0')
        .filter(|row| !row.is_empty())
    {
        let (fields, name) = record.split_once('\t').context("无效冲突索引条目")?;
        ensure!(name == path, "冲突条目没有精确匹配");
        let parts: Vec<_> = fields.split_whitespace().collect();
        ensure!(
            parts.len() == 3 && git_index::oid_ok(parts[1]),
            "无效冲突索引对象"
        );
        let stage = parts[2].parse::<usize>()?;
        ensure!((1..=3).contains(&stage), "文件没有未解决的冲突");
        ensure!(stages[stage - 1].is_none(), "冲突索引条目重复");
        stages[stage - 1] = Some(IndexEntry {
            mode: parts[0].to_owned(),
            oid: parts[1].to_owned(),
        });
    }
    Ok(stages)
}

type DiskSnapshot = (Option<Vec<u8>>, ReviewVersion, Option<Permissions>);

fn read_disk(repo: &Path, rel: &str) -> Result<DiskSnapshot> {
    let path = repo.join(rel);
    git::validate_path_within_repo(Some(repo), rel, &path)?;
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok((None, ReviewVersion::default(), None));
        }
        Err(err) => return Err(err.into()),
    };
    ensure!(
        meta.is_file() && !meta.is_symlink(),
        "冲突编辑仅支持常规文件"
    );
    ensure!(
        meta.len() <= MAX_REVIEW_BYTES as u64,
        "冲突文件超过审阅大小限制"
    );
    let file = File::open(&path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && meta.modified()? == opened.modified()? && meta.len() == opened.len(),
        "打开期间工作区版本已变化"
    );
    let mut bytes = Vec::new();
    file.take(MAX_REVIEW_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_REVIEW_BYTES, "冲突文件超过审阅大小限制");
    let version = git::metadata_to_version(&opened, &bytes);
    Ok((Some(bytes), version, Some(opened.permissions())))
}

fn binary(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

pub(crate) fn read(repo: &Path, path: &str) -> Result<ConflictFile> {
    let stages = stage_entries(repo, path)?;
    ensure!(stages.iter().any(Option::is_some), "文件没有未解决的冲突");
    let mut versions = [None, None, None];
    for (index, entry) in stages.iter().enumerate() {
        if let Some(entry) = entry {
            ensure!(
                git_index::regular_mode(&entry.mode),
                "冲突编辑仅支持常规文件"
            );
            versions[index] = Some(git_index::index_git(
                repo,
                &["cat-file", "blob", &entry.oid],
                None,
                None,
            )?);
        }
    }
    let (working, disk, permissions) = read_disk(repo, path)?;
    let ending = working
        .as_deref()
        .or(versions[1].as_deref())
        .or(versions[2].as_deref())
        .map_or(LineEnding::Lf, LineEnding::detect);
    let binary = versions
        .iter()
        .filter_map(Option::as_deref)
        .chain(working.as_deref())
        .any(binary);
    Ok(ConflictFile {
        path: path.to_owned(),
        stages,
        versions,
        working,
        ending,
        binary,
        disk,
        permissions,
    })
}

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_name(parent: &Path, stem: &str) -> PathBuf {
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    parent.join(format!(
        ".nemorous-{stem}-{}-{nanos}-{seq}",
        std::process::id()
    ))
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn write_disk(
    repo: &Path,
    rel: &str,
    expected: &ReviewVersion,
    body: Option<&[u8]>,
    permissions: Option<Permissions>,
    git_mode: Option<&str>,
) -> Result<ReviewVersion> {
    git_index::literal_file(rel)?;
    let path = repo.join(rel);
    git::validate_path_within_repo(Some(repo), rel, &path)?;
    if body.is_none() {
        let current = read_disk(repo, rel)?.1;
        ensure!(current == *expected, "冲突文件已被外部修改");
        if current.exists {
            fs::remove_file(path)?;
        }
        return Ok(ReviewVersion::default());
    }
    let body = body.unwrap_or_default();
    let parent = path.parent().context("无法定位冲突文件的父目录")?;
    let temporary = temp_name(parent, "conflict");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let temporary = TempFile(temporary);
    file.write_all(body)?;
    if let Some(permissions) = permissions {
        file.set_permissions(permissions)?;
    }
    #[cfg(unix)]
    if let Some(mode) = git_mode {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        let bits = permissions.mode();
        permissions.set_mode(if mode == "100755" {
            bits | 0o111
        } else {
            bits & !0o111
        });
        file.set_permissions(permissions)?;
    }
    #[cfg(not(unix))]
    let _ = git_mode;
    file.sync_all()?;
    drop(file);
    ensure!(read_disk(repo, rel)?.1 == *expected, "冲突文件已被外部修改");
    if expected.exists {
        fs::rename(&temporary.0, &path)?;
    } else {
        fs::hard_link(&temporary.0, &path).context("目标文件已被外部创建或无法写入")?;
    }
    Ok(git::metadata_to_version(
        &fs::symlink_metadata(&path)?,
        body,
    ))
}

pub(crate) fn stage_resolution(
    repo: &Path,
    snapshot: &ConflictFile,
    choice: ResolveWith,
) -> Result<()> {
    let tx = IndexTransaction::begin(repo)?;
    let path = &snapshot.path;
    ensure!(
        stage_entries(repo, path)? == snapshot.stages,
        "冲突索引已变化，请重新加载"
    );
    let first = snapshot.stages[1]
        .as_ref()
        .or(snapshot.stages[2].as_ref())
        .or(snapshot.stages[0].as_ref())
        .context("文件没有可解决的冲突")?;
    let (body, entry) = match choice {
        ResolveWith::Delete => (None, None),
        ResolveWith::Stage(side) => {
            let entry = snapshot.stages.get(side).context("无效冲突侧")?.clone();
            (snapshot.versions[side].clone(), entry)
        }
        ResolveWith::Edited(body) => {
            ensure!(
                !snapshot.binary && !binary(&body),
                "二进制或非 UTF-8 冲突只能选择整份版本"
            );
            ensure!(
                conflict_text::parse(&body)?.is_empty(),
                "文件还包含未处理的冲突块"
            );
            let oid = git_index::hash_text(repo, body.clone(), Some(path))?;
            (
                Some(body),
                Some(IndexEntry {
                    mode: first.mode.clone(),
                    oid,
                }),
            )
        }
    };
    git_index::put_entry(repo, tx.index_path(), path, entry.as_ref(), Some(first))?;
    let changed = write_disk(
        repo,
        path,
        &snapshot.disk,
        body.as_deref(),
        snapshot.permissions.clone(),
        entry.as_ref().map(|entry| entry.mode.as_str()),
    )?;
    if let Err(err) = tx.install() {
        let restored = write_disk(
            repo,
            path,
            &changed,
            snapshot.working.as_deref(),
            snapshot.permissions.clone(),
            None,
        );
        if restored.is_err() {
            bail!("索引安装失败，磁盘已有新的文件内容，请重新检查冲突：{err}");
        }
        return Err(err).context("索引安装失败，工作区文件已恢复");
    }
    Ok(())
}

fn draft_file(root: &Path, repo: &Path, file: &ConflictFile) -> PathBuf {
    let mut key = Sha256::new();
    key.update(repo.as_os_str().as_encoded_bytes());
    key.update([0]);
    key.update(file.path.as_bytes());
    for entry in &file.stages {
        key.update([0]);
        if let Some(entry) = entry {
            key.update(entry.oid.as_bytes());
        }
    }
    key.update(file.disk.sha256.as_bytes());
    root.join(format!("{:x}.draft", key.finalize()))
}

pub(crate) fn read_draft(root: &Path, repo: &Path, file: &ConflictFile) -> Option<String> {
    let path = draft_file(root, repo, file);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_REVIEW_BYTES as u64 {
        return None;
    }
    fs::read_to_string(path).ok()
}

pub(crate) fn save_draft(root: &Path, repo: &Path, file: &ConflictFile, text: &str) -> Result<()> {
    ensure!(text.len() <= MAX_REVIEW_BYTES, "冲突草稿超过大小限制");
    fs::create_dir_all(root)?;
    let path = draft_file(root, repo, file);
    let tmp = temp_name(root, "draft");
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut out = options.open(&tmp)?;
    let tmp = TempFile(tmp);
    out.write_all(text.as_bytes())?;
    out.sync_all()?;
    drop(out);
    fs::rename(&tmp.0, &path)?;
    Ok(())
}

pub(crate) fn remove_draft(root: &Path, repo: &Path, file: &ConflictFile) {
    let _ = fs::remove_file(draft_file(root, repo, file));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::run_git_string;

    #[test]
    fn conflict_stage_write_checks_snapshot_and_preserves_crlf_and_other_entries() {
        let root = std::env::temp_dir().join(format!(
            "nemorous-conflict-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(root.clone());
        let git = |args: &[&str]| run_git_string(&root, args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Nemorous QA"]);
        git(&["config", "user.email", "qa@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        git(&["config", "core.autocrlf", "false"]);
        let name = "冲突 [*.txt]";
        fs::write(root.join(name), b"base\r\n").unwrap();
        fs::write(root.join("binary"), b"\0base").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-m", "base"]);
        git(&["switch", "-c", "theme"]);
        fs::write(root.join(name), b"source\r\n").unwrap();
        fs::write(root.join("binary"), b"\0source").unwrap();
        git(&["commit", "-am", "source"]);
        git(&["switch", "main"]);
        fs::write(root.join(name), b"target\r\n").unwrap();
        fs::write(root.join("binary"), b"\0target").unwrap();
        git(&["commit", "-am", "target"]);
        assert!(
            run_git_string(
                &root,
                &[
                    "-c",
                    "merge.conflictStyle=diff3",
                    "merge",
                    "--no-edit",
                    "theme"
                ]
            )
            .is_err()
        );
        let file = read(&root, name).unwrap();
        assert_eq!(file.versions[1].as_deref(), Some(&b"target\r\n"[..]));
        assert_eq!(file.versions[2].as_deref(), Some(&b"source\r\n"[..]));
        let draft_root = root.join(".git").join("drafts");
        save_draft(&draft_root, &root, &file, "planned change").unwrap();
        assert_eq!(
            read_draft(&draft_root, &root, &read(&root, name).unwrap()).as_deref(),
            Some("planned change")
        );
        let original_conflicts = stage_entries(&root, "binary").unwrap();
        fs::write(root.join(name), b"external update\r\n").unwrap();
        let result = b"target\r\nsource\r\n".to_vec();
        assert!(stage_resolution(&root, &file, ResolveWith::Edited(result.clone())).is_err());
        assert_eq!(fs::read(root.join(name)).unwrap(), b"external update\r\n");
        assert_eq!(stage_entries(&root, name).unwrap(), file.stages);
        let updated = read(&root, name).unwrap();
        stage_resolution(&root, &updated, ResolveWith::Edited(result.clone())).unwrap();
        assert_eq!(fs::read(root.join(name)).unwrap(), result);
        assert_eq!(git(&["show", &format!(":{name}")]).as_bytes(), result);
        assert_eq!(stage_entries(&root, "binary").unwrap(), original_conflicts);
        let binary = read(&root, "binary").unwrap();
        assert!(binary.binary);
        stage_resolution(&root, &binary, ResolveWith::Stage(2)).unwrap();
        assert_eq!(fs::read(root.join("binary")).unwrap(), b"\0source");
        assert!(crate::merge_tasks::unresolved(&root).unwrap().is_empty());
    }
}
