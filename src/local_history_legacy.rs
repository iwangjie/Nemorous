//! Read-only adapter for legacy Nemorous (Go) local-history snapshots.
//!
//! Old snapshots live under
//! `localhistory/<lowercase-sha1(repo_id)>/<lowercase-sha1(normalized path)>/`.
//! The caller supplies that already-hashed directory as `bucket`, just like the
//! current Rust store. Old entries are `<20-digit id>.json` plus
//! `<20-digit id>.txt`. This module never writes, migrates, or rewrites them;
//! current writes remain in `local-history/*.snap`.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::local_history::SnapshotMeta;

const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MAX_CONTENT_BYTES: u64 = 1024 * 1024;
const ID_LEN: usize = 20;

#[derive(Deserialize)]
struct LegacyRecord {
    id: String,
    #[serde(rename = "repoId")]
    repo_id: String,
    path: String,
    at: String,
    #[serde(default)]
    reason: Option<String>,
    size: u64,
}

/// Returns whether `id` is exactly 20 ASCII decimal digits, the only shape
/// accepted for Nemorous legacy snapshot ids.
pub fn is_legacy_id(id: &str) -> bool {
    id.len() == ID_LEN && id.as_bytes().iter().all(|byte| byte.is_ascii_digit())
}

/// Lists valid legacy snapshots in `bucket` for the requested repo and path.
///
/// Individual corrupt or mismatched records are skipped. A missing bucket is
/// treated as an empty legacy store.
pub fn list(bucket: &Path, repo_id: &str, rel_path: &str) -> Result<Vec<SnapshotMeta>> {
    let entries = match fs::read_dir(bucket) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("read legacy local-history bucket {}", bucket.display()));
        }
    };

    let wanted_path = normalize_path(rel_path);
    let mut snapshots = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_name = match entry.file_name().into_string() {
            Ok(file_name) => file_name,
            Err(_) => continue,
        };
        let id = match file_name.strip_suffix(".json") {
            Some(id) => id,
            None => continue,
        };
        if !is_legacy_id(id) {
            continue;
        }
        match entry.file_type() {
            Ok(file_type) if file_type.is_file() => {}
            _ => continue,
        }
        if let Ok(snapshot) = load_snapshot(bucket, id, repo_id, &wanted_path) {
            snapshots.push(snapshot);
        }
    }

    snapshots.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(snapshots)
}

/// Reads the old `.txt` payload for a legacy snapshot id.
pub fn read_content(bucket: &Path, id: &str) -> Result<Vec<u8>> {
    if !is_legacy_id(id) {
        bail!("invalid legacy snapshot id: {:?}", id);
    }
    read_bounded_regular(&bucket.join(format!("{id}.txt")), MAX_CONTENT_BYTES)
}

fn load_snapshot(
    bucket: &Path,
    id: &str,
    repo_id: &str,
    wanted_path: &str,
) -> Result<SnapshotMeta> {
    let metadata_path = bucket.join(format!("{id}.json"));
    let raw_metadata = read_bounded_regular(&metadata_path, MAX_METADATA_BYTES)?;
    let record: LegacyRecord = serde_json::from_slice(&raw_metadata)
        .with_context(|| format!("parse legacy metadata {}", metadata_path.display()))?;

    let LegacyRecord {
        id: record_id,
        repo_id: record_repo_id,
        path: record_path_raw,
        at,
        reason,
        size: record_size,
    } = record;

    if record_id.as_str() != id {
        bail!(
            "legacy record id {:?} does not match filename id {:?}",
            record_id,
            id
        );
    }
    if record_repo_id.as_str() != repo_id {
        bail!(
            "legacy record repo id {:?} does not match requested repo id {:?}",
            record_repo_id,
            repo_id
        );
    }
    let record_path = normalize_path(&record_path_raw);
    if record_path.as_str() != wanted_path {
        bail!(
            "legacy record path {:?} does not match requested path {:?}",
            record_path_raw,
            wanted_path
        );
    }
    if chrono::DateTime::parse_from_rfc3339(&at).is_err() {
        bail!("legacy record timestamp is not RFC3339: {:?}", at);
    }
    if record_size > MAX_CONTENT_BYTES {
        bail!(
            "legacy record declares {} bytes, above the {} byte limit",
            record_size,
            MAX_CONTENT_BYTES
        );
    }

    let content_path = bucket.join(format!("{id}.txt"));
    let content = fs::symlink_metadata(&content_path)?;
    if !content.is_file() || content.len() != record_size {
        bail!("legacy snapshot content is missing or inconsistent");
    }

    Ok(SnapshotMeta {
        id: id.to_owned(),
        label: label_from_reason(reason),
        repo_id: repo_id.to_owned(),
        rel_path: wanted_path.to_owned(),
        created_at: at,
        sha256: String::new(),
        size: record_size as usize,
    })
}

/// The old Go record stores a free-form `reason`; it is the closest equivalent
/// of the current store's `label`.
fn label_from_reason(reason: Option<String>) -> String {
    reason.unwrap_or_default()
}

fn normalize_path(path: &str) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        path.to_string()
    } else {
        path.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

pub(crate) fn read_bounded_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let symlink_meta = fs::symlink_metadata(path)
        .with_context(|| format!("stat legacy file {}", path.display()))?;
    if !symlink_meta.file_type().is_file() {
        bail!("legacy path is not a regular file: {}", path.display());
    }
    if symlink_meta.len() > limit {
        bail!("legacy file {} exceeds {} bytes", path.display(), limit);
    }

    let file = File::open(path).with_context(|| format!("open legacy file {}", path.display()))?;
    let opened_meta = file
        .metadata()
        .with_context(|| format!("stat opened legacy file {}", path.display()))?;
    if !opened_meta.is_file() {
        bail!("legacy path is not a regular file: {}", path.display());
    }
    if opened_meta.len() > limit {
        bail!("legacy file {} exceeds {} bytes", path.display(), limit);
    }

    let mut data = Vec::with_capacity(opened_meta.len().min(limit) as usize);
    let mut reader = file.take(limit.saturating_add(1));
    reader
        .read_to_end(&mut data)
        .with_context(|| format!("read legacy file {}", path.display()))?;
    if data.len() as u64 > limit {
        bail!("legacy file {} exceeds {} bytes", path.display(), limit);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "omp-local-history-legacy-{}-{}-{}",
                std::process::id(),
                name,
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    const ID: &str = "12345678901234567890";

    fn write_entry(bucket: &Path, id: &str, json: &str, content: &[u8]) {
        fs::write(bucket.join(format!("{id}.json")), json.as_bytes()).unwrap();
        fs::write(bucket.join(format!("{id}.txt")), content).unwrap();
    }

    #[test]
    fn reads_true_shaped_legacy_snapshot() {
        let dir = TestDir::new("true-shape");
        let bucket = dir.path();

        write_entry(
            bucket,
            ID,
            r#"{"id":"12345678901234567890","repoId":"repo-one","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","reason":"manual","size":5}"#,
            b"hello",
        );

        let snapshots = list(bucket, "repo-one", "src/lib.rs").unwrap();
        assert_eq!(
            snapshots,
            vec![SnapshotMeta {
                id: ID.to_owned(),
                label: "manual".to_owned(),
                repo_id: "repo-one".to_owned(),
                rel_path: "src/lib.rs".to_owned(),
                created_at: "2026-11-20T01:02:03.123456789Z".to_owned(),
                sha256: String::new(),
                size: 5,
            }]
        );

        assert_eq!(read_content(bucket, ID).unwrap(), b"hello");
        assert!(is_legacy_id(ID));
        assert!(!is_legacy_id("123"));
        assert!(!is_legacy_id("1234567890123456789a"));
        assert!(!is_legacy_id("123456789012345678901"));
    }

    #[test]
    fn skips_invalid_names_corrupt_records_and_mismatches() {
        let dir = TestDir::new("invalid");
        let bucket = dir.path();

        write_entry(
            bucket,
            ID,
            r#"{"id":"12345678901234567890","repoId":"repo-one","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","size":5}"#,
            b"hello",
        );

        // Filename is not a 20-digit id.
        write_entry(
            bucket,
            "not-a-valid-id",
            r#"{"id":"not-a-valid-id","repoId":"repo-one","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","size":1}"#,
            b"x",
        );

        // Record id does not match its filename.
        write_entry(
            bucket,
            "00000000000000000001",
            r#"{"id":"00000000000000000002","repoId":"repo-one","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","size":1}"#,
            b"x",
        );

        // Record repo does not match the requested repo.
        write_entry(
            bucket,
            "00000000000000000003",
            r#"{"id":"00000000000000000003","repoId":"other-repo","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","size":1}"#,
            b"x",
        );

        // Record path does not match the requested path.
        write_entry(
            bucket,
            "00000000000000000004",
            r#"{"id":"00000000000000000004","repoId":"repo-one","path":"src/other.rs","at":"2026-11-20T01:02:03.123456789Z","size":1}"#,
            b"x",
        );

        // Unparseable metadata.
        write_entry(
            bucket,
            "00000000000000000005",
            r#"{"id":"00000000000000000005","repoId":"repo-one","path":"src/lib.rs""#,
            b"x",
        );

        // Declared size does not match the content.
        write_entry(
            bucket,
            "00000000000000000006",
            r#"{"id":"00000000000000000006","repoId":"repo-one","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","size":2}"#,
            b"x",
        );

        // Metadata exists, content does not.
        fs::write(
            bucket.join("00000000000000000007.json"),
            br#"{"id":"00000000000000000007","repoId":"repo-one","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","size":1}"#,
        )
        .unwrap();

        let snapshots = list(bucket, "repo-one", "src/lib.rs").unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, ID);

        assert!(read_content(bucket, "not-a-valid-id").is_err());
        assert!(read_content(bucket, "../12345678901234567890").is_err());
        assert!(read_content(bucket, "00000000000000000007").is_err());

        let missing = dir.path().join("missing-bucket");
        assert!(list(&missing, "repo-one", "src/lib.rs").unwrap().is_empty());
    }

    #[test]
    fn rejects_oversized_content() {
        let dir = TestDir::new("oversized");
        let bucket = dir.path();
        let id = "00000000000000000009";
        let content = vec![b'x'; (MAX_CONTENT_BYTES + 1) as usize];
        let json = format!(
            r#"{{"id":"{id}","repoId":"repo-one","path":"src/lib.rs","at":"2026-11-20T01:02:03.123456789Z","size":{}}}"#,
            content.len()
        );

        write_entry(bucket, id, &json, &content);

        assert!(list(bucket, "repo-one", "src/lib.rs").unwrap().is_empty());
        assert!(read_content(bucket, id).is_err());
    }
}
