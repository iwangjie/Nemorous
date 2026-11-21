use std::path::{Component, Path};

use anyhow::{Context, Result, bail, ensure};

use crate::git::{BoundedGitOutput, MAX_REVIEW_BYTES, run_git_limited_with_index};
pub(crate) use crate::git_index_transaction::IndexTransaction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexEntry {
    pub oid: String,
    pub mode: String,
}

pub(crate) fn literal_file(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && !path.contains('\0')
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            && !path
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | "..")),
        "无效的仓库相对路径"
    );
    Ok(())
}

pub(crate) fn oid_ok(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|char| char.is_ascii_hexdigit())
}

pub(crate) fn regular_mode(mode: &str) -> bool {
    matches!(mode, "100644" | "100755")
}

pub(crate) fn index_git(
    repo: &Path,
    args: &[&str],
    input: Option<Vec<u8>>,
    index: Option<&Path>,
) -> Result<Vec<u8>> {
    match run_git_limited_with_index(repo, args, MAX_REVIEW_BYTES, input, index)? {
        BoundedGitOutput::Ok(bytes) => Ok(bytes),
        BoundedGitOutput::TooLarge => bail!("Git 输出超过审阅大小上限"),
        BoundedGitOutput::Failed { stderr, exit } => {
            bail!("Git 操作失败（退出码 {exit:?}）：{stderr}")
        }
    }
}

pub(crate) fn read_index_entry(repo: &Path, path: &str) -> Result<Option<IndexEntry>> {
    literal_file(path)?;
    let raw = index_git(
        repo,
        &[
            "--literal-pathspecs",
            "ls-files",
            "--stage",
            "--debug",
            "-z",
            "--",
            path,
        ],
        None,
        None,
    )?;
    if raw.is_empty() {
        return Ok(None);
    }
    let raw = String::from_utf8(raw).context("索引条目含有无效 UTF-8")?;
    let (record, info) = raw.split_once('\0').context("无法读取 Git 索引条目")?;
    let (prefix, rel) = record.split_once('\t').context("索引条目缺少路径")?;
    ensure!(rel == path, "索引路径不是单个文件");
    let mut fields = prefix.split_whitespace();
    let mode = fields.next().context("索引条目缺少文件类型")?;
    let oid = fields.next().context("索引条目缺少对象 ID")?;
    let stage = fields.next().context("索引条目缺少 stage")?;
    ensure!(
        stage == "0" && !info.contains('\0'),
        "文件存在未合并的索引条目"
    );
    ensure!(oid_ok(oid), "索引条目的对象 ID 无效");
    let flags = info
        .lines()
        .find_map(|line| line.rsplit_once("\tflags: "))
        .and_then(|(_, value)| u32::from_str_radix(value.trim(), 16).ok())
        .context("无法读取索引条目标志")?;
    ensure!(
        flags & 0x2000_0000 == 0,
        "仅标记为意向添加的文件无法按块修改"
    );
    Ok(Some(IndexEntry {
        oid: oid.to_owned(),
        mode: mode.to_owned(),
    }))
}

pub(crate) fn read_index_blob(repo: &Path, entry: &IndexEntry) -> Result<Vec<u8>> {
    ensure!(
        oid_ok(&entry.oid) && regular_mode(&entry.mode),
        "仅支持普通文件"
    );
    let body = index_git(repo, &["cat-file", "blob", &entry.oid], None, None)?;
    ensure!(!body.contains(&0), "仅支持文本文件");
    std::str::from_utf8(&body).context("仅支持 UTF-8 文本")?;
    Ok(body)
}

pub(crate) fn hash_text(repo: &Path, raw: Vec<u8>, filter_path: Option<&str>) -> Result<String> {
    ensure!(raw.len() <= MAX_REVIEW_BYTES, "文件超过审阅大小上限");
    ensure!(!raw.contains(&0), "仅支持文本文件");
    std::str::from_utf8(&raw).context("仅支持 UTF-8 文本")?;
    let mut args = vec!["hash-object", "-w", "--stdin"];
    let filter;
    if let Some(path) = filter_path {
        literal_file(path)?;
        filter = format!("--path={path}");
        args.push(&filter);
    }
    let oid = String::from_utf8(index_git(repo, &args, Some(raw), None)?)?
        .trim()
        .to_owned();
    ensure!(oid_ok(&oid), "Git 返回了无效的 blob 对象 ID");
    Ok(oid)
}

pub(crate) fn put_entry(
    repo: &Path,
    index: &Path,
    path: &str,
    entry: Option<&IndexEntry>,
    removed: Option<&IndexEntry>,
) -> Result<()> {
    literal_file(path)?;
    let (mode, oid) = match (entry, removed) {
        (Some(entry), _) => {
            ensure!(
                regular_mode(&entry.mode) && oid_ok(&entry.oid),
                "无效索引条目"
            );
            (entry.mode.as_str(), entry.oid.clone())
        }
        (None, Some(removed)) => ("0", "0".repeat(removed.oid.len())),
        (None, None) => return Ok(()),
    };
    let input = format!("{mode} {oid}\t{path}\0").into_bytes();
    index_git(
        repo,
        &["update-index", "-z", "--index-info"],
        Some(input),
        Some(index),
    )?;
    Ok(())
}

pub(crate) fn replace_index_text(
    repo: &Path,
    path: &str,
    expected: Option<&IndexEntry>,
    bytes: Option<Vec<u8>>,
    mode: &str,
) -> Result<Option<IndexEntry>> {
    let tx = IndexTransaction::begin(repo)?;
    let actual = read_index_entry(repo, path)?;
    ensure!(
        actual.as_ref() == expected,
        "暂存区内容已变化，请刷新差异后重试"
    );
    if let Some(entry) = &actual {
        ensure!(regular_mode(&entry.mode), "仅支持普通文件");
    }
    if bytes.is_none() && actual.is_none() {
        return Ok(None);
    }
    let next = bytes
        .map(|body| -> Result<IndexEntry> {
            ensure!(regular_mode(mode), "仅支持普通文件");
            Ok(IndexEntry {
                oid: hash_text(repo, body, None)?,
                mode: mode.to_owned(),
            })
        })
        .transpose()?;
    if next != actual {
        put_entry(repo, tx.index_path(), path, next.as_ref(), actual.as_ref())?;
        tx.install()?;
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::run_git_string;
    use std::fs;

    #[test]
    fn stage_zero_write_preserves_other_index_entries_and_checks_preconditions() {
        let temp = std::env::temp_dir().join(format!(
            "nemorous-index-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&temp).unwrap();
        struct Clean(std::path::PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(temp.clone());
        let repo = temp.as_path();
        let git = |args: &[&str]| run_git_string(repo, args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "core.autocrlf", "false"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.invalid"]);
        let name = "样例 [*].txt";
        fs::write(repo.join(name), b"base\r\n").unwrap();
        fs::write(repo.join("other"), b"orig\n").unwrap();
        git(&["add", "-A"]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "base"]);
        fs::write(repo.join("other"), b"staged\n").unwrap();
        git(&["add", "other"]);
        fs::write(repo.join("pending"), b"intent\n").unwrap();
        git(&["add", "-N", "pending"]);
        assert!(
            read_index_entry(repo, "pending")
                .unwrap_err()
                .to_string()
                .contains("意向")
        );
        let old = read_index_entry(repo, name).unwrap().unwrap();
        let other = read_index_entry(repo, "other").unwrap();
        assert_eq!(read_index_blob(repo, &old).unwrap(), b"base\r\n");
        let lock = repo.join(".git/index.lock");
        fs::write(&lock, b"owned by another Git process").unwrap();
        assert!(
            replace_index_text(repo, name, Some(&old), Some(b"new\r\n".to_vec()), &old.mode)
                .is_err()
        );
        assert_eq!(fs::read(&lock).unwrap(), b"owned by another Git process");
        fs::remove_file(lock).unwrap();
        let updated =
            replace_index_text(repo, name, Some(&old), Some(b"new\r\n".to_vec()), &old.mode)
                .unwrap()
                .unwrap();
        assert_eq!(read_index_blob(repo, &updated).unwrap(), b"new\r\n");
        assert_eq!(fs::read(repo.join(name)).unwrap(), b"base\r\n");
        assert_eq!(read_index_entry(repo, "other").unwrap(), other);
        assert!(
            replace_index_text(repo, name, Some(&old), Some(b"stale".to_vec()), &old.mode).is_err()
        );
        assert_eq!(read_index_entry(repo, name).unwrap(), Some(updated));
    }
}

pub(crate) fn tree_file(repo: &Path, treeish: &str, path: &str) -> Result<Option<IndexEntry>> {
    literal_file(path)?;
    ensure!(oid_ok(treeish), "无效 Git 对象 ID");
    let bytes = index_git(
        repo,
        &[
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            "--full-tree",
            treeish,
            "--",
            path,
        ],
        None,
        None,
    )?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let data = String::from_utf8(bytes)?;
    let (meta, name) = data.split_once('\t').context("无法读取 Git 树条目")?;
    ensure!(
        name.strip_suffix('\0') == Some(path),
        "Git 树路径没有精确匹配"
    );
    let mut fields = meta.split_whitespace();
    let mode = fields.next().context("缺少 Git 树文件模式")?;
    let kind = fields.next().context("缺少 Git 树对象类型")?;
    let oid = fields.next().context("缺少 Git 树对象 ID")?;
    ensure!(
        matches!(kind, "blob" | "commit") && oid_ok(oid),
        "目标不是单个 Git 文件"
    );
    Ok(Some(IndexEntry {
        mode: mode.to_owned(),
        oid: oid.to_owned(),
    }))
}
