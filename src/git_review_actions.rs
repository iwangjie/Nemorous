use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};

use crate::git::{self, ChangedFile, LineEnding, ReviewChangeData};
use crate::git_commit;
use crate::git_index::{self, IndexEntry};
use crate::selected_changes::{Selection, apply_selection};

pub(crate) struct ReviewBytes {
    pub old: Vec<u8>,
    pub new: Vec<u8>,
    pub mode: String,
    pub old_exists: bool,
    pub new_exists: bool,
    pub entry: Option<IndexEntry>,
}

pub(crate) fn read_selection_versions(
    file: &ChangedFile,
    view: &ReviewChangeData,
) -> Result<ReviewBytes> {
    ensure!(
        !file.conflict && file.old_path_or_current() == file.path,
        "此文件无法按块操作"
    );
    ensure!(!view.binary && !view.too_large, "此文件无法按块操作");
    git_index::literal_file(&file.path)?;
    let repo = Path::new(&file.repo_path);
    let expected = view
        .index_snapshot
        .clone()
        .map_err(|reason| anyhow!("{reason}"))?;
    let entry = git_index::read_index_entry(repo, &file.path)?;
    ensure!(entry == expected, "暂存区已变化，请刷新差异");
    let index = entry
        .as_ref()
        .map(|entry| git_index::read_index_blob(repo, entry))
        .transpose()?;
    if view.staged_only {
        let head = git_commit::head_oid(repo)?;
        ensure!(head == view.head_snapshot, "HEAD 已变化，请刷新差异");
        let head_entry = head
            .as_deref()
            .map(|oid| git_index::tree_file(repo, oid, &file.path))
            .transpose()?
            .flatten();
        let old = head_entry
            .as_ref()
            .map(|entry| git_index::read_index_blob(repo, entry))
            .transpose()?;
        let mode = entry
            .as_ref()
            .or(head_entry.as_ref())
            .map_or("100644", |entry| entry.mode.as_str())
            .to_owned();
        let old_exists = old.is_some();
        let new_exists = index.is_some();
        return Ok(ReviewBytes {
            old: old.unwrap_or_default(),
            new: index.unwrap_or_default(),
            mode,
            old_exists,
            new_exists,
            entry,
        });
    }

    let (worktree, mode) = git::read_review_worktree_bytes(repo, &file.path)?;
    let raw = if let Some((bytes, version)) = worktree {
        ensure!(version == view.version, "工作区文件已变化，请刷新差异");
        let oid = git_index::hash_text(repo, bytes, Some(&file.path))?;
        let cleaned = git_index::read_index_blob(
            repo,
            &IndexEntry {
                oid,
                mode: mode.to_owned(),
            },
        )?;
        let text = std::str::from_utf8(&cleaned).context("无法读取规范化后的文本")?;
        ensure!(
            LineEnding::normalize_to_lf(text) == view.new_text,
            "Git 内容过滤改变了差异行，请暂存整个文件"
        );
        Some(cleaned)
    } else {
        ensure!(!view.version.exists, "工作区文件已删除，请刷新差异");
        None
    };
    let mode = entry
        .as_ref()
        .map_or(mode, |entry| entry.mode.as_str())
        .to_owned();
    let old_exists = index.is_some();
    let new_exists = raw.is_some();
    Ok(ReviewBytes {
        old: index.unwrap_or_default(),
        new: raw.unwrap_or_default(),
        mode,
        old_exists,
        new_exists,
        entry,
    })
}

pub(crate) fn stage_selection(
    file: &ChangedFile,
    view: &ReviewChangeData,
    selection: &Selection,
) -> Result<()> {
    let bytes = read_selection_versions(file, view)?;
    let result = apply_selection(&bytes.old, &bytes.new, selection, view.staged_only)?;
    let exists = if view.staged_only {
        bytes.old_exists
    } else {
        bytes.new_exists
    };
    let next = (exists || !result.is_empty()).then_some(result);
    git_index::replace_index_text(
        Path::new(&file.repo_path),
        &file.path,
        bytes.entry.as_ref(),
        next,
        &bytes.mode,
    )?;
    Ok(())
}

pub(crate) fn discard_file(
    file: &ChangedFile,
    expected: &git::ReviewVersion,
    expected_head: Option<&str>,
    expected_index: Option<&IndexEntry>,
) -> Result<()> {
    let repo = Path::new(&file.repo_path);
    git_index::literal_file(&file.path)?;
    ensure!(
        !file.conflict && file.old_path_or_current() == file.path,
        "请使用 Git 冲突或重命名操作处理此文件"
    );
    let path = repo.join(&file.path);
    git::validate_path_within_repo(Some(repo), &file.path, &path)?;
    ensure!(
        git_commit::head_oid(repo)?.as_deref() == expected_head,
        "HEAD 已改变，请重新确认"
    );
    ensure!(
        git_index::read_index_entry(repo, &file.path)?.as_ref() == expected_index,
        "暂存区已改变，请重新确认"
    );
    let current = git::read_review_worktree(&path).version;
    ensure!(current == *expected, "工作区文件已改变，请重新确认");
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        ensure!(meta.is_file() && !meta.is_symlink(), "仅支持回滚普通文件");
    }
    if file.untracked {
        ensure!(
            expected_index.is_none() && expected.exists,
            "此路径不再是未跟踪文件"
        );
        std::fs::remove_file(path).context("无法删除未跟踪文件")?;
    } else {
        let head = expected_head.context("工作区没有 HEAD 提交")?;
        let source = format!("--source={head}");
        git_index::index_git(
            repo,
            &[
                "--literal-pathspecs",
                "restore",
                &source,
                "--staged",
                "--worktree",
                "--",
                &file.path,
            ],
            None,
            None,
        )?;
    }
    Ok(())
}
