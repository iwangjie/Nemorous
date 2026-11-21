use std::collections::BTreeMap;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub workspace_path: String,
    pub recent_workspaces: Vec<String>,
    pub hidden_repos: Vec<String>,
    pub theme: String,
    pub release_branch: String,
    pub default_base_branch: String,
    pub branch_presets: Vec<BranchPreset>,
    pub changes_flat: bool,
    pub change_diff_preview: bool,
    pub git_log_diff_preview: bool,
    pub editor_font_size: f32,
    pub editor_soft_wrap: bool,
    pub diff_single_column: bool,
    pub diff_inline_mode: String,
    pub diff_whitespace_mode: String,
    pub diff_show_whitespaces: bool,
    pub diff_collapse_unchanged: bool,
    pub commit_left_width: f32,
    pub code_left_width: f32,
    pub git_panel_height: f32,
    pub terminal_panel_height: f32,
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
            theme: "light".to_string(),
            release_branch: "release".to_string(),
            default_base_branch: "main".to_string(),
            branch_presets: Vec::new(),
            changes_flat: false,
            change_diff_preview: false,
            git_log_diff_preview: false,
            editor_font_size: 14.0,
            editor_soft_wrap: true,
            diff_single_column: false,
            diff_inline_mode: "word".to_string(),
            diff_whitespace_mode: "none".to_string(),
            diff_show_whitespaces: false,
            diff_collapse_unchanged: false,
            commit_left_width: 320.0,
            code_left_width: 260.0,
            git_panel_height: 290.0,
            terminal_panel_height: 240.0,
            keymap_overrides: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl Config {
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
        if self.code_left_width < 120.0 || self.code_left_width > 900.0 {
            self.code_left_width = 260.0;
        }
        if self.git_panel_height < 120.0 || self.git_panel_height > 900.0 {
            self.git_panel_height = 290.0;
        }
        self
    }

    /// 将旧版 Go 字段（`diff_side_by_side`、`diff_ignore_mode`）同步到当前字段。
    pub fn sync_from_legacy_extra(&mut self) {
        if let Some(side_by_side) = self.extra.get("diff_side_by_side").and_then(|v| v.as_bool()) {
            self.diff_single_column = !side_by_side;
        }
        if let Some(mode) = self.extra.get("diff_ignore_mode").and_then(|v| v.as_u64()) {
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
    let mut cfg: Config = serde_json::from_str(&data)
        .with_context(|| format!("解析配置文件失败: {}", path.display()))?;
    cfg.sync_from_legacy_extra();
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
