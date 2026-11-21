use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::local_history_legacy;
use sha1::{Digest, Sha1};
use sha2::Sha256;

const MAX_SNAPSHOTS_PER_FILE: usize = 30;
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024; // 1 MiB，与旧版 internal/localhistory/store.go 一致

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotMeta {
    pub id: String,
    pub label: String,
    pub repo_id: String,
    pub rel_path: String,
    pub created_at: String,
    pub sha256: String,
    pub size: usize,
}

#[derive(Debug, Clone)]
pub struct LocalHistoryStore {
    root: PathBuf,
    legacy_root: Option<PathBuf>,
}

impl LocalHistoryStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            legacy_root: None,
        }
    }

    pub fn default_store() -> Self {
        let config = crate::config::config_dir();
        Self {
            root: config.join("local-history"),
            legacy_root: Some(config.join("localhistory")),
        }
    }

    fn legacy_bucket(&self, repo_id: &str, rel_path: &str) -> Option<PathBuf> {
        let root = self.legacy_root.as_ref()?;
        Some(
            root.join(sha1_hex(repo_id.as_bytes()))
                .join(sha1_hex(rel_path.as_bytes())),
        )
    }

    fn bucket_dir(&self, repo_id: &str, rel_path: &str) -> PathBuf {
        self.root
            .join(sha1_hex(repo_id.as_bytes()))
            .join(sha1_hex(rel_path.replace('\\', "/").as_bytes()))
    }

    pub fn record(
        &self,
        repo_id: &str,
        rel_path: &str,
        label: &str,
        content: &[u8],
    ) -> Result<Option<SnapshotMeta>> {
        if content.len() > MAX_SNAPSHOT_BYTES {
            return Ok(None);
        }
        static WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
        let _write = WRITE_LOCK.lock();
        let dir = self.bucket_dir(repo_id, rel_path);
        fs::create_dir_all(&dir)
            .with_context(|| format!("创建本地历史目录失败: {}", dir.display()))?;

        let hash = sha256_hex(content);
        let existing = self.list(repo_id, rel_path)?;
        if let Some(latest) = existing.first() {
            if latest.sha256 == hash {
                return Ok(None);
            }
        }

        let now = Utc::now();
        let id = format!(
            "{}-{}",
            now.format("%Y%m%dT%H%M%S%.3fZ"),
            &hash[..8.min(hash.len())]
        );
        let meta = SnapshotMeta {
            id: id.clone(),
            label: label.to_string(),
            repo_id: repo_id.to_string(),
            rel_path: rel_path.replace('\\', "/"),
            created_at: now.to_rfc3339(),
            sha256: hash,
            size: content.len(),
        };

        let content_path = dir.join(format!("{id}.snap"));
        let meta_path = dir.join(format!("{id}.json"));
        crate::session::write_atomic(&content_path, content)
            .with_context(|| format!("写入本地历史内容失败: {}", content_path.display()))?;
        let meta_json = serde_json::to_string_pretty(&meta)?;
        crate::session::write_atomic(&meta_path, meta_json.as_bytes())
            .with_context(|| format!("写入本地历史元数据失败: {}", meta_path.display()))?;

        self.prune(&dir)?;
        Ok(Some(meta))
    }

    pub fn list(&self, repo_id: &str, rel_path: &str) -> Result<Vec<SnapshotMeta>> {
        let dir = self.bucket_dir(repo_id, rel_path);
        let mut items = Vec::new();
        if dir.is_dir() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(raw) = local_history_legacy::read_bounded_regular(&path, 64 * 1024)
                    && let Ok(meta) = serde_json::from_slice::<SnapshotMeta>(&raw)
                    && path.file_stem().and_then(|stem| stem.to_str()) == Some(meta.id.as_str())
                    && meta.repo_id == repo_id
                    && meta.rel_path == rel_path.replace('\\', "/")
                    && DateTime::parse_from_rfc3339(&meta.created_at).is_ok()
                {
                    items.push(meta);
                }
            }
        }
        if let Some(bucket) = self.legacy_bucket(repo_id, rel_path) {
            items.extend(local_history_legacy::list(&bucket, repo_id, rel_path)?);
        }
        items.sort_by_cached_key(|entry| {
            std::cmp::Reverse((
                DateTime::parse_from_rfc3339(&entry.created_at).ok(),
                entry.id.clone(),
            ))
        });
        Ok(items)
    }

    pub fn read_content(&self, repo_id: &str, rel_path: &str, id: &str) -> Result<Vec<u8>> {
        anyhow::ensure!(
            !id.is_empty()
                && !id.starts_with('.')
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')),
            "本地历史快照编号无效"
        );
        if local_history_legacy::is_legacy_id(id) {
            let bucket = self
                .legacy_bucket(repo_id, rel_path)
                .context("旧版历史目录不可用")?;
            return local_history_legacy::read_content(&bucket, id);
        }
        let path = self
            .bucket_dir(repo_id, rel_path)
            .join(format!("{id}.snap"));
        local_history_legacy::read_bounded_regular(&path, MAX_SNAPSHOT_BYTES as u64)
            .with_context(|| format!("读取本地历史快照失败: {}", path.display()))
    }

    fn prune(&self, dir: &Path) -> Result<()> {
        let mut ids: Vec<String> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    ids.push(stem.to_string());
                }
            }
        }
        ids.sort_by(|a, b| b.cmp(a));
        for old_id in ids.into_iter().skip(MAX_SNAPSHOTS_PER_FILE) {
            let _ = fs::remove_file(dir.join(format!("{old_id}.json")));
            let _ = fs::remove_file(dir.join(format!("{old_id}.snap")));
        }
        Ok(())
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    hex_lower(&out)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let out = hasher.finalize();
    hex_lower(&out)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
