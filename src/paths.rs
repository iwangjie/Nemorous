use std::path::{Path, PathBuf};

/// 返回适合用户阅读与复制的显示路径：
/// 位于用户主目录下时折叠为 `~` 或 `~/...`，主目录外保留绝对路径。
pub fn display_path(path: &str) -> String {
    display_path_with_home(path, std::env::var("HOME").ok().as_deref())
}

pub fn display_path_with_home(path: &str, home: Option<&str>) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let clean = normalize_slashes(trimmed);
    let Some(home) = home.map(str::trim).filter(|h| !h.is_empty()) else {
        return clean;
    };
    let clean_home = normalize_slashes(home).trim_end_matches('/').to_string();
    if clean_home.is_empty() {
        return clean;
    }
    if clean == clean_home {
        return "~".to_string();
    }
    if let Some(rest) = clean.strip_prefix(&clean_home) {
        if rest.starts_with('/') {
            return format!("~{rest}");
        }
    }
    clean
}

/// 将 `~/...` 展开为用户主目录路径（如果 `HOME` 可用）。
pub fn expand_tilde(path: &str) -> String {
    expand_tilde_with_home(path, std::env::var("HOME").ok().as_deref())
}

pub fn expand_tilde_with_home(path: &str, home: Option<&str>) -> String {
    let trimmed = path.trim();
    let Some(home) = home.map(str::trim).filter(|h| !h.is_empty()) else {
        return trimmed.to_string();
    };
    let clean_home = home.trim_end_matches('/');
    if trimmed == "~" {
        return clean_home.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return format!("{clean_home}/{rest}");
    }
    trimmed.to_string()
}

pub fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn basename(path: &str) -> String {
    let norm = normalize_slashes(path);
    let trimmed = norm.trim_end_matches('/');
    if trimmed.is_empty() {
        return norm;
    }
    trimmed
        .rsplit('/')
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

pub fn parent_dir_display(abs_path: &str) -> String {
    let p = Path::new(abs_path);
    match p.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            normalize_slashes(&parent.to_string_lossy())
        }
        _ => normalize_slashes(abs_path),
    }
}

/// 计算 `abs_path` 相对于 `base_dir` 的正斜杠相对路径；若不在其下则回退 `fallback`。
pub fn relative_slash_path(base_dir: &str, abs_path: &str, fallback: &str) -> String {
    let base = Path::new(base_dir);
    let target = Path::new(abs_path);
    if let Ok(rel) = target.strip_prefix(base) {
        let s = normalize_slashes(&rel.to_string_lossy());
        if !s.is_empty() {
            return s;
        }
    }
    if !fallback.is_empty() {
        normalize_slashes(fallback)
    } else {
        normalize_slashes(abs_path)
    }
}

/// 复制“文件与行号”的引用格式：`workspace_rel:line`（line 为 1-based）。
pub fn path_with_line_reference(workspace_dir: &str, abs_path: &str, rel_fallback: &str, line_1based: u32) -> String {
    let rel = relative_slash_path(workspace_dir, abs_path, rel_fallback);
    format!("{rel}:{}", line_1based.max(1))
}

/// 向上查找最近的已存在路径，用于缺失文件时“在文件管理器中显示”。
pub fn nearest_existing_path(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(p) = current {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        current = p.parent();
    }
    None
}

/// 解析转到文件输入框中的路径与行列号，支持：
/// - `path:line`
/// - `path:line:col`
/// - `~/path:line:col`
/// - `path#L12`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFileLocation {
    pub path: String,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

pub fn parse_file_location(input: &str) -> ParsedFileLocation {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ParsedFileLocation {
            path: String::new(),
            line: None,
            col: None,
        };
    }

    // 处理 `#L12` 或 `#L12C3`
    if let Some((left, right)) = trimmed.rsplit_once("#L") {
        if let Some((line, col)) = parse_line_col_suffix(right) {
            return ParsedFileLocation {
                path: left.trim().to_string(),
                line: Some(line),
                col,
            };
        }
    }

    // 处理 `:line:col` 或 `:line`
    let parts: Vec<&str> = trimmed.rsplitn(3, ':').collect();
    if parts.len() == 3 {
        if let (Ok(col), Ok(line)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            if line >= 1 && col >= 1 && !parts[2].is_empty() {
                return ParsedFileLocation {
                    path: parts[2].trim().to_string(),
                    line: Some(line),
                    col: Some(col),
                };
            }
        }
    }
    if parts.len() >= 2 {
        if let Ok(line) = parts[0].parse::<u32>() {
            let prefix = &trimmed[..trimmed.len() - parts[0].len() - 1];
            if line >= 1 && !prefix.is_empty() {
                return ParsedFileLocation {
                    path: prefix.trim().to_string(),
                    line: Some(line),
                    col: None,
                };
            }
        }
    }

    ParsedFileLocation {
        path: trimmed.to_string(),
        line: None,
        col: None,
    }
}

fn parse_line_col_suffix(s: &str) -> Option<(u32, Option<u32>)> {
    let upper = s.to_ascii_uppercase();
    if let Some((l, c)) = upper.split_once('C') {
        let line = l.parse::<u32>().ok()?;
        let col = c.parse::<u32>().ok()?;
        if line >= 1 && col >= 1 {
            return Some((line, Some(col)));
        }
        return None;
    }
    let line = upper.parse::<u32>().ok()?;
    if line >= 1 {
        Some((line, None))
    } else {
        None
    }
}

/// 将单行文本内的 UTF-8 字节偏移转为 UTF-16 code unit 列号（与 NSH Editor / LSP 对齐）。
pub fn byte_offset_to_utf16_col(line: &str, byte_offset: usize) -> u32 {
    let clamped = byte_offset.min(line.len());
    let mut boundary = clamped;
    while boundary > 0 && !line.is_char_boundary(boundary) {
        boundary -= 1;
    }
    line[..boundary].encode_utf16().count() as u32
}

/// 将单行文本内的 UTF-16 code unit 列号转为 UTF-8 字节偏移。
pub fn utf16_col_to_byte_offset(line: &str, utf16_col: u32) -> usize {
    let mut units = 0u32;
    for (idx, ch) in line.char_indices() {
        if units >= utf16_col {
            return idx;
        }
        units += ch.len_utf16() as u32;
    }
    line.len()
}

/// 解析 `line` 或 `line:col` 输入（1-based，返回 0-based 的 `(line, col)`）。
pub fn parse_goto_line_col(input: &str) -> Option<(u32, u32)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((l, c)) = trimmed.split_once(':') {
        let line = l.trim().parse::<u32>().ok()?;
        let col = c.trim().parse::<u32>().unwrap_or(1);
        if line == 0 {
            return None;
        }
        return Some((line - 1, col.saturating_sub(1)));
    }
    let line = trimmed.parse::<u32>().ok()?;
    if line == 0 {
        return None;
    }
    Some((line - 1, 0))
}
