use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BranchPreset {
    pub name: String,
    pub repo_branches: BTreeMap<String, String>,
}

impl Default for BranchPreset {
    fn default() -> Self {
        Self {
            name: String::new(),
            repo_branches: BTreeMap::new(),
        }
    }
}

/// 旧版内置行书签；`line` 为 1 起始行号。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Bookmark {
    #[serde(default)]
    pub repo_id: String,
    #[serde(default)]
    pub rel_path: String,
    #[serde(default)]
    pub line: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChangeList {
    pub name: String,
    #[serde(deserialize_with = "deserialize_change_list_files")]
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
}

fn deserialize_change_list_files<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub workspace_path: String,
    pub recent_workspaces: Vec<String>,
    pub hidden_repos: Vec<String>,
    pub bookmarks: Vec<Bookmark>,
    pub theme: String,
    pub release_branch: String,
    pub default_base_branch: String,
    pub branch_presets: Vec<BranchPreset>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub change_lists: Vec<ChangeList>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub active_change_list: String,
    pub changes_flat: bool,
    pub change_diff_preview: bool,
    pub git_log_diff_preview: bool,
    pub editor_font_size: f32,
    pub editor_soft_wrap: bool,
    pub format_on_save: bool,
    pub format_command: String,
    pub diff_single_column: bool,
    pub diff_inline_mode: String,
    pub diff_whitespace_mode: String,
    pub diff_show_whitespaces: bool,
    pub diff_collapse_unchanged: bool,
    pub commit_left_width: f32,
    pub code_left_width: f32,
    pub git_panel_height: f32,
    pub terminal_panel_height: f32,
    pub project_follow_editor: bool,
    pub keymap_overrides: BTreeMap<String, String>,
    /// 保留旧版或未来批次的其余配置字段，避免读写时丢失数据。
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace_path: String::new(),
            recent_workspaces: Vec::new(),
            hidden_repos: Vec::new(),
            bookmarks: Vec::new(),
            theme: "light".to_string(),
            release_branch: "release".to_string(),
            default_base_branch: "main".to_string(),
            branch_presets: Vec::new(),
            change_lists: Vec::new(),
            active_change_list: String::new(),
            changes_flat: false,
            change_diff_preview: false,
            git_log_diff_preview: false,
            editor_font_size: 14.0,
            editor_soft_wrap: true,
            format_on_save: false,
            format_command: String::new(),
            diff_single_column: false,
            diff_inline_mode: "word".to_string(),
            diff_whitespace_mode: "none".to_string(),
            diff_show_whitespaces: false,
            diff_collapse_unchanged: false,
            commit_left_width: 320.0,
            code_left_width: 260.0,
            git_panel_height: 290.0,
            terminal_panel_height: 240.0,
            project_follow_editor: false,
            keymap_overrides: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl Config {
    pub(crate) fn extra_bool(&self, key: &str, default: bool) -> bool {
        self.extra.get(key).and_then(serde_json::Value::as_bool).unwrap_or(default)
    }

    pub(crate) fn extra_str<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.extra.get(key).and_then(serde_json::Value::as_str).unwrap_or(default)
    }

    pub(crate) fn parallelism(&self) -> usize {
        self.extra.get("concurrency").and_then(serde_json::Value::as_u64).unwrap_or(8).clamp(1,32) as usize
    }

    pub(crate) fn zoom(&self) -> f32 {
        self.extra.get("zoom").and_then(serde_json::Value::as_f64).unwrap_or(1.0).clamp(0.75,2.0) as f32
    }

    pub(crate) fn protected_branch(&self, name: &str) -> bool {
        let name = name.trim().strip_prefix("origin/").unwrap_or(name.trim()).to_ascii_lowercase();
        if matches!(name.as_str(),"main"|"master"|"trunk"|"release"|"develop"|"development")
            || name.starts_with("release/") || name.starts_with("release-") {
            return true;
        }
        if let Some(branches) = self.extra.get("protected_branches").and_then(serde_json::Value::as_array) {
            branches.iter().filter_map(serde_json::Value::as_str)
                .any(|branch| branch.trim().eq_ignore_ascii_case(&name))
        } else {
            matches!(name.as_str(),"test"|"dev"|"prod"|"production")
        }
    }

    pub fn normalized(mut self) -> Self {
        if self.theme.is_empty() {
            self.theme = "light".to_string();
        }
        if self.editor_font_size < 10.0 || self.editor_font_size > 24.0 {
            self.editor_font_size = 14.0;
        }
        if self.diff_inline_mode.is_empty() {
            self.diff_inline_mode = "word".to_string();
        }
        if self.diff_whitespace_mode.is_empty() {
            self.diff_whitespace_mode = "none".to_string();
        }
        if self.commit_left_width < 80.0 || self.commit_left_width > 900.0 {
            self.commit_left_width = 320.0;
        }
        if self.code_left_width < 90.0 || self.code_left_width > 900.0 {
            self.code_left_width = 260.0;
        }
        if self.git_panel_height < 120.0 || self.git_panel_height > 900.0 {
            self.git_panel_height = 290.0;
        }
        self
    }

    /// 将旧版 Go 字段（`diff_side_by_side`、`diff_ignore_mode`）同步到当前字段。
    fn sync_from_legacy_extra(&mut self, modern_column: bool, modern_whitespace: bool) {
        if !modern_column && let Some(side_by_side) = self.extra.get("diff_side_by_side").and_then(|v| v.as_bool()) {
            self.diff_single_column = !side_by_side;
        }
        if !modern_whitespace && let Some(mode) = self.extra.get("diff_ignore_mode").and_then(|v| v.as_u64()).filter(|m| *m <= 3) {
            self.diff_whitespace_mode = match mode {
                1 => "trim".to_string(),
                2 => "all".to_string(),
                3 => "all_and_empty".to_string(),
                _ => "none".to_string(),
            };
        } else {
            // 兼容别名规范化
            self.diff_whitespace_mode = match self.diff_whitespace_mode.as_str() {
                "trim" | "trim_whitespace" => "trim".to_string(),
                "all" | "whitespace" | "all_whitespace" => "all".to_string(),
                "all_and_empty" | "whitespace_and_blank" | "blank_lines" => {
                    "all_and_empty".to_string()
                }
                _ => "none".to_string(),
            };
        }
    }

    /// 写盘前把当前字段回写到旧版 Go 兼容键，保证旧版仍可无缝读取该副本。
    pub fn sync_to_legacy_extra(&mut self) {
        self.extra.insert(
            "diff_side_by_side".to_string(),
            serde_json::Value::Bool(!self.diff_single_column),
        );
        let ignore_code: u64 = match self.diff_whitespace_mode.as_str() {
            "trim" | "trim_whitespace" => 1,
            "all" | "whitespace" | "all_whitespace" => 2,
            "all_and_empty" | "whitespace_and_blank" | "blank_lines" => 3,
            _ => 0,
        };
        self.extra.insert(
            "diff_ignore_mode".to_string(),
            serde_json::Value::Number(ignore_code.into()),
        );
    }

    pub fn matches_change_list(&self, key: &str) -> bool {
        match self.active_change_list.as_str() {
            "" | "__all__" => true,
            "__default__" => self.change_list_for(key).is_empty(),
            name => self.change_list_for(key) == name,
        }
    }

    pub fn change_list_for(&self, key: &str) -> &str {
        self.change_lists
            .iter()
            .find(|list| list.files.iter().any(|file| file == key))
            .map_or("", |list| list.name.as_str())
    }

    pub fn create_change_list(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        self.validate_new_change_list_name(name)?;
        self.change_lists.push(ChangeList {
            name: name.to_owned(),
            updated_at: chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).to_rfc3339(),
            ..ChangeList::default()
        });
        Ok(())
    }

    pub fn rename_change_list(&mut self, from: &str, to: &str) -> Result<()> {
        let to = to.trim();
        if from == to {
            return Ok(());
        }
        self.validate_new_change_list_name(to)?;
        let list = self.change_lists.iter_mut().find(|list| list.name == from)
            .context("变更列表已不存在")?;
        list.name = to.to_owned();
        list.updated_at = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).to_rfc3339();
        if self.active_change_list == from {
            self.active_change_list = to.to_owned();
        }
        Ok(())
    }

    fn validate_new_change_list_name(&self, name: &str) -> Result<()> {
        anyhow::ensure!(!name.is_empty() && !matches!(name, "__all__" | "__default__"), "请输入有效的变更列表名");
        anyhow::ensure!(
            !self.change_lists.iter().any(|list| list.name == name),
            "变更列表名已存在"
        );
        Ok(())
    }

    pub fn delete_change_list(&mut self, name: &str) {
        self.change_lists.retain(|list| list.name != name);
        if self.active_change_list == name {
            self.active_change_list.clear();
        }
    }

    pub fn assign_change_list(&mut self, name: &str, keys: &[String]) {
        if !name.is_empty() && !self.change_lists.iter().any(|list| list.name == name) {
            return;
        }
        let keys: HashSet<_> = keys.iter().cloned().collect();
        let updated_at = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).to_rfc3339();
        for list in &mut self.change_lists {
            if list.name == name {
                list.files.extend(keys.iter().cloned());
                list.files.sort();
                list.files.dedup();
                list.updated_at.clone_from(&updated_at);
            } else {
                let before = list.files.len();
                list.files.retain(|file| !keys.contains(file));
                if list.files.len() != before {
                    list.updated_at.clone_from(&updated_at);
                }
            }
        }
    }

    pub fn is_repo_hidden(&self, repo_id: &str) -> bool {
        self.hidden_repos.iter().any(|id| id == repo_id)
    }

    pub fn add_recent_workspace(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return;
        }
        self.recent_workspaces.retain(|p| p != trimmed);
        self.recent_workspaces.insert(0, trimmed.to_string());
        if self.recent_workspaces.len() > 16 {
            self.recent_workspaces.truncate(16);
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("NEMOROUS_CONFIG") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("APPDATA").filter(|path| !path.is_empty()) {
        return PathBuf::from(path).join("nemorous").join("config.json");
    }
    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|path| Path::new(path).is_absolute()) {
        return PathBuf::from(path).join("nemorous").join("config.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            #[cfg(target_os = "macos")]
            {
                return PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("nemorous")
                    .join("config.json");
            }
            #[cfg(not(target_os = "macos"))]
            {
                return PathBuf::from(home)
                    .join(".config")
                    .join("nemorous")
                    .join("config.json");
            }
        }
    }
    PathBuf::from(".nemorous-config.json")
}

pub fn config_dir() -> PathBuf {
    let path = config_path();
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn load_from(path: &Path) -> Result<Config> {
    if !path.exists() {
        let mut cfg = Config::default();
        if let Ok(ws) = std::env::var("NEMOROUS_WORKSPACE") {
            let trimmed = ws.trim();
            if !trimmed.is_empty() {
                cfg.workspace_path = trimmed.to_string();
                cfg.add_recent_workspace(trimmed);
            }
        }
        return Ok(cfg);
    }
    let data = fs::read_to_string(path)
        .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
    let raw: serde_json::Value = serde_json::from_str(&data)
        .with_context(|| format!("解析配置文件失败: {}", path.display()))?;
    let modern_column = raw.get("diff_single_column").is_some();
    let modern_whitespace = raw.get("diff_whitespace_mode").is_some();
    let mut cfg: Config = serde_json::from_value(raw)
        .with_context(|| format!("解析配置字段失败: {}", path.display()))?;
    cfg.sync_from_legacy_extra(modern_column, modern_whitespace);
    if let Ok(ws) = std::env::var("NEMOROUS_WORKSPACE") {
        let trimmed = ws.trim();
        if !trimmed.is_empty() {
            cfg.workspace_path = trimmed.to_string();
            cfg.add_recent_workspace(trimmed);
        }
    }
    Ok(cfg.normalized())
}

pub fn load() -> Config {
    let path = config_path();
    match load_from(&path) {
        Ok(cfg) => cfg,
        Err(err) => {
            log::warn!("加载配置回退默认值 ({}): {err}", path.display());
            let mut cfg = Config::default();
            if let Ok(ws) = std::env::var("NEMOROUS_WORKSPACE") {
                let trimmed = ws.trim();
                if !trimmed.is_empty() {
                    cfg.workspace_path = trimmed.to_string();
                    cfg.add_recent_workspace(trimmed);
                }
            }
            cfg
        }
    }
}

pub fn save_to(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }
    let mut out = cfg.clone();
    out.sync_to_legacy_extra();
    let json = serde_json::to_string_pretty(&out).context("序列化配置失败")?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp, json.as_bytes())
        .with_context(|| format!("写入临时配置失败: {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("替换配置文件失败: {}", path.display()))?;
    Ok(())
}

pub fn save(cfg: &Config) -> Result<()> {
    save_to(&config_path(), cfg)
}

#[cfg(test)]
mod change_list_tests {
    use super::*;

    #[test]
    fn change_lists_roundtrip_legacy_null_files_and_move_without_duplicates() {
        let mut cfg: Config = serde_json::from_str(r#"{
            "change_lists": [
                {"name":"登录","files":["a::one","a::two"],"updated_at":"2026-11-20T01:00:00Z"},
                {"name":"支付","files":null}
            ],
            "active_change_list":"登录",
            "external_setting":{"preserve":true}
        }"#).unwrap();
        cfg.assign_change_list("支付", &["a::one".into(), "a::one".into()]);
        assert_eq!(cfg.change_list_for("a::one"), "支付");
        assert_eq!(cfg.change_list_for("a::two"), "登录");
        assert_eq!(cfg.change_lists[1].files, ["a::one"]);
        cfg.rename_change_list("登录", "认证").unwrap();
        assert_eq!(cfg.active_change_list, "认证");
        assert!(cfg.rename_change_list("认证", "支付").is_err());
        cfg.delete_change_list("认证");
        assert_eq!(cfg.active_change_list, "");
        assert_eq!(cfg.change_list_for("a::two"), "");
        cfg.create_change_list("新条目").unwrap();
        let data = serde_json::to_string(&cfg).unwrap();
        let loaded: Config = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded.change_list_for("a::one"), "支付");
        assert!(loaded.change_lists[1].files.is_empty());
        assert_eq!(loaded.extra["external_setting"]["preserve"], true);
    }
}
