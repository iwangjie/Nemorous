//! 工作区编辑会话的读写与恢复（与历史 Go 版本的文件格式保持兼容）。

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};

use crate::config;

const MAX_OPEN_FILES: usize = 20;
const MAX_SESSION_BYTES: u64 = 256 * 1024;
const HEX: &[u8; 16] = b"0123456789abcdef";
const MANAGED_KEYS: &[&str] = &[
    "workspace",
    "openFiles",
    "activeFile",
    "activeTab",
    "showGitPanel",
    "changesSplit",
    "codeSplit",
];

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

fn is_zero_f32(value: &f32) -> bool {
    *value == 0.0
}

fn skip_active_file(value: &Option<i64>) -> bool {
    value.is_none_or(|index| index == 0)
}

/// 恢复出来的一行源码标签。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    #[serde(default, rename = "repoId")]
    pub repo_id: String,
    #[serde(default, rename = "path")]
    pub rel_path: String,
    #[serde(default, skip_serializing_if = "is_zero_i64", rename = "line")]
    pub line: i64,
}

/// 与 Go 端 `session.State` 同构的会话快照。
///
/// `extra` 承载旧版本写入、本版本不解释的字段（如 `gitEditorPct`、
/// `showManiPanel`、`pluginPanel`），重写文件时原样保留以便旧版本回滚。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "openFiles")]
    pub open_files: Vec<FileEntry>,
    #[serde(
        default,
        skip_serializing_if = "skip_active_file",
        rename = "activeFile"
    )]
    pub active_file: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "activeTab")]
    pub active_tab: Option<String>,
    #[serde(default, skip_serializing_if = "is_false", rename = "showGitPanel")]
    pub show_git_panel: bool,
    #[serde(default, skip_serializing_if = "is_zero_f32", rename = "changesSplit")]
    pub changes_split: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32", rename = "codeSplit")]
    pub code_split: f32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 会话文件固定放在 NEMOROUS_CONFIG 同级的 `sessions/` 目录下。
pub fn session_path(workspace: &str) -> PathBuf {
    session_path_beside(&config::config_path(), workspace)
}

pub fn session_path_beside(config_path: &Path, workspace: &str) -> PathBuf {
    let dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("sessions")
        .join(format!("{}.json", workspace_key(workspace)))
}

/// Go 的 `filepath.Clean` 语义（仅 `/` 分隔符，测试与 darwin/linux 一致）。
pub fn lexically_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => match parts.last() {
                Some(last) if *last != ".." => {
                    parts.pop();
                }
                _ if absolute => {}
                _ => parts.push(".."),
            },
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// `<lexically_clean(workspace)>` 的 SHA1 前 8 字节（小写 hex）。
pub fn workspace_key(workspace: &str) -> String {
    let cleaned = lexically_clean(workspace);
    let digest = Sha1::digest(cleaned.as_bytes());
    let mut out = String::with_capacity(16);
    for byte in &digest[..8] {
        out.push(char::from(HEX[(byte >> 4) as usize]));
        out.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    out
}

fn read_session_bytes(path: &Path) -> io::Result<Vec<u8>> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "session is not a regular file",
        ));
    }
    let file = fs::File::open(path)?;
    let mut data = Vec::new();
    file.take(MAX_SESSION_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_SESSION_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "session is too large",
        ));
    }
    Ok(data)
}

/// 缺失、损坏或非对象内容一律视为空会话。
pub fn load(path: &Path) -> State {
    let Ok(data) = read_session_bytes(path) else {
        return State::default();
    };
    let mut st: State = match serde_json::from_slice(&data) {
        Ok(value) => value,
        Err(_) => return State::default(),
    };
    st.open_files.truncate(MAX_OPEN_FILES);
    st
}

pub fn save(path: &Path, st: &State) -> io::Result<()> {
    save_if_changed(path, st).map(|_| ())
}

/// 快照与磁盘内容语义一致时不写盘；返回是否真的重写了文件。
pub fn save_if_changed(path: &Path, st: &State) -> io::Result<bool> {
    let mut capped = st.clone();
    capped.open_files.truncate(MAX_OPEN_FILES);
    let mut value = serde_json::to_value(&capped).map_err(to_io)?;
    if let Some(obj) = value.as_object_mut() {
        for key in ["changesSplit", "codeSplit"] {
            if let Some(number) = obj.get(key).and_then(Value::as_f64) {
                if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e15 {
                    obj.insert((*key).to_string(), Value::from(number as i64));
                }
            }
        }
    }
    let mut incoming = value.as_object().cloned().unwrap_or_default();
    let current = match read_session_bytes(path) {
        Ok(data) => serde_json::from_slice::<Value>(&data)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(err) => return Err(err),
    };
    let mut merged = current.clone();
    for key in MANAGED_KEYS {
        match incoming.remove(*key) {
            Some(value) => {
                merged.insert((*key).to_string(), value);
            }
            None => {
                merged.remove(*key);
            }
        }
    }
    for (key, value) in incoming {
        merged.insert(key, value);
    }
    if path.is_file() && current == merged {
        return Ok(false);
    }
    let merged = Value::Object(merged);
    let pretty = serde_json::to_string_pretty(&merged).map_err(to_io)?;
    let mut bytes = pretty.into_bytes();
    bytes.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(path, &bytes)?;
    Ok(true)
}

pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "session.json".to_string());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut attempts = 0;
    loop {
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".{name}.tmp.{}.{nanos}.{seq}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(mut file) => {
                let written = (|| -> io::Result<()> {
                    file.write_all(data)?;
                    file.sync_all()?;
                    Ok(())
                })();
                drop(file);
                if let Err(err) = written {
                    let _ = fs::remove_file(&tmp);
                    return Err(err);
                }
                return match fs::rename(&tmp, path) {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        let _ = fs::remove_file(&tmp);
                        Err(err)
                    }
                };
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempts < 8 => {
                attempts += 1;
                continue;
            }
            Err(err) => return Err(err),
        }
    }
}

fn to_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nemorous-session-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn workspace_key_matches_legacy_derivation() {
        let hex: String = Sha1::digest(b"abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(workspace_key("abc"), "a9993e364706816a");
        assert_eq!(workspace_key("/tmp/../tmp//ws/"), workspace_key("/tmp/ws"));
        assert_eq!(workspace_key(""), workspace_key("."));
        assert_eq!(lexically_clean("/a/../b/./c"), "/b/c");
        assert_eq!(
            session_path_beside(Path::new("/tmp/cfg/config.json"), "/ws"),
            PathBuf::from("/tmp/cfg/sessions").join(format!("{}.json", workspace_key("/ws")))
        );
    }

    #[test]
    fn legacy_fixture_loads_saves_and_preserves_alien_fields() {
        let dir = temp_dir("legacy");
        let big = dir.join("big.json");
        let mut files = Vec::new();
        for index in 0..22 {
            files.push(serde_json::json!({
                "repoId": "repo",
                "path": format!("src/f{index}.rs"),
                "line": if index == 0 { -4 } else { index }
            }));
        }
        let legacy = serde_json::json!({
            "workspace": "/old/ws",
            "openFiles": files,
            "activeFile": 99,
            "activeTab": "code",
            "showGitPanel": true,
            "changesSplit": 333,
            "showManiPanel": true,
            "gitEditorPct": 50,
            "x-legacy": {"nested": [1, 2, 3]}
        });
        fs::write(&big, serde_json::to_string(&legacy).unwrap()).unwrap();
        let loaded = load(&big);
        assert_eq!(loaded.open_files.len(), 20);
        assert_eq!(loaded.open_files[0].line, -4);
        assert_eq!(loaded.workspace.as_deref(), Some("/old/ws"));
        assert!(loaded.extra.contains_key("x-legacy"));
        assert_eq!(load(&dir.join("missing.json")).open_files.len(), 0);

        let small = dir.join("small.json");
        let fixture = serde_json::json!({
            "workspace": "/old/ws",
            "openFiles": [
                {"repoId": "repo", "path": "src/a.rs", "line": 5},
                {"repoId": "repo", "path": "src/b.rs", "line": 2}
            ],
            "activeFile": 1,
            "activeTab": "code",
            "showGitPanel": true,
            "changesSplit": 333,
            "gitEditorPct": 50,
            "x-legacy": {"nested": [1, 2, 3]}
        });
        fs::write(&small, serde_json::to_string(&fixture).unwrap()).unwrap();
        let loaded = load(&small);
        assert!(
            !save_if_changed(&small, &loaded).unwrap(),
            "unchanged snapshot must not rewrite"
        );

        let mut changed = loaded.clone();
        changed.workspace = None;
        changed.show_git_panel = false;
        changed.open_files.truncate(1);
        changed.active_file = None;
        changed.changes_split = 0.0;
        changed.code_split = 0.0;
        assert!(save_if_changed(&small, &changed).unwrap());

        let raw: Value = serde_json::from_slice(&fs::read(&small).unwrap()).unwrap();
        assert_eq!(raw["x-legacy"]["nested"][2], 3);
        assert_eq!(raw["gitEditorPct"], 50);
        assert_eq!(raw["openFiles"].as_array().unwrap().len(), 1);
        assert_eq!(raw["openFiles"][0]["line"], 5);
        assert!(raw.get("workspace").is_none());
        assert!(raw.get("showGitPanel").is_none());
        assert!(raw.get("activeFile").is_none());
        assert!(raw.get("changesSplit").is_none());
        assert!(raw.get("codeSplit").is_none());

        let leftovers: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
