use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::Value;

pub(crate) fn server(path: &Path) -> Option<(&'static str, &'static str)> {
    Some(match path.extension()?.to_str()? {
        "go" => ("go", "gopls"),
        "rs" => ("rust", "rust-analyzer"),
        "java" => ("java", "jdtls"),
        "py" => ("python", "pyright-langserver --stdio"),
        "js" | "mjs" | "cjs" => ("javascript", "typescript-language-server --stdio"),
        "jsx" => ("javascriptreact", "typescript-language-server --stdio"),
        "ts" => ("typescript", "typescript-language-server --stdio"),
        "tsx" => ("typescriptreact", "typescript-language-server --stdio"),
        "c" | "h" => ("c", "clangd"),
        "cc" | "cpp" | "cxx" | "hpp" => ("cpp", "clangd"),
        "html" => ("html", "vscode-html-language-server --stdio"),
        "css" => ("css", "vscode-css-language-server --stdio"),
        "json" | "jsonc" => ("json", "vscode-json-language-server --stdio"),
        _ => return None,
    })
}

pub(crate) fn file_uri(path: &Path) -> Result<String> {
    ensure!(path.is_absolute(), "LSP 文件路径必须是绝对路径");
    let path = path.to_str().context("LSP 文件路径必须是 UTF-8")?;
    let path = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    };
    let mut url = if cfg!(windows) && !path.starts_with('/') {
        "file:///"
    } else {
        "file://"
    }
    .to_owned();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(&mut url, "%{byte:02X}")?;
        }
    }
    Ok(url)
}

pub(crate) fn uri_path(uri: &str) -> Result<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .context("LSP 返回了非文件链接")?;
    let rest = rest
        .strip_prefix("localhost/")
        .map(|path| format!("/{path}"))
        .unwrap_or_else(|| rest.to_owned());
    ensure!(rest.starts_with('/'), "LSP 文件链接包含远端主机");
    let mut bytes = Vec::with_capacity(rest.len());
    let mut chars = rest.as_bytes().iter().copied();
    while let Some(byte) = chars.next() {
        if byte == b'%' {
            let hex = |ch: Option<u8>| {
                ch.and_then(|byte| char::from(byte).to_digit(16))
                    .map(|value| value as u8)
            };
            let high = hex(chars.next()).context("LSP 文件链接编码无效")?;
            let low = hex(chars.next()).context("LSP 文件链接编码无效")?;
            bytes.push(high * 16 + low);
        } else {
            bytes.push(byte);
        }
    }
    let decoded = String::from_utf8(bytes)?;
    ensure!(!decoded.contains('\0'), "LSP 文件链接包含无效字符");
    let path = if cfg!(windows) && decoded.as_bytes().get(2) == Some(&b':') {
        &decoded[1..]
    } else {
        &decoded
    };
    Ok(PathBuf::from(path))
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Edit {
    pub range: Range,
    #[serde(rename = "newText")]
    pub text: String,
}

pub(crate) fn utf16_to_byte(text: &str, units: u32) -> Result<usize> {
    let mut current = 0;
    for (index, character) in text.char_indices() {
        if current == units {
            return Ok(index);
        }
        current += character.len_utf16() as u32;
        if current > units {
            bail!("LSP 坐标切分了 UTF-16 字符");
        }
    }
    ensure!(current == units, "LSP 坐标超出当前行");
    Ok(text.len())
}

fn offset(text: &str, position: Position) -> Result<usize> {
    let mut lines = text.split('\n');
    let mut offset = 0;
    for _ in 0..position.line {
        offset += lines.next().context("LSP 坐标超出文件")?.len() + 1;
    }
    let raw = lines.next().context("LSP 坐标超出文件")?;
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    Ok(offset + utf16_to_byte(line, position.character)?)
}

pub(crate) fn editor_position(text: &str, position: Position) -> Result<nsh_base::EditorPosition> {
    let line = text
        .split('\n')
        .nth(position.line as usize)
        .context("LSP 坐标超出文件")?;
    let byte = utf16_to_byte(line.trim_end_matches('\r'), position.character)?;
    Ok(nsh_base::EditorPosition::new(
        position.line,
        line[..byte].chars().count() as u32,
    ))
}

pub(crate) fn apply_edits(text: &str, edits: &[Edit]) -> Result<String> {
    let mut ranges = edits
        .iter()
        .map(|edit| {
            let start = offset(text, edit.range.start)?;
            let end = offset(text, edit.range.end)?;
            ensure!(start <= end, "LSP 返回了逆向编辑范围");
            Ok((start, end, edit.text.as_str()))
        })
        .collect::<Result<Vec<_>>>()?;
    ranges.sort_by_key(|range| (range.0, range.1));
    ensure!(
        ranges.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "LSP 返回了重叠编辑范围"
    );
    let mut result = text.to_owned();
    for (start, end, replacement) in ranges.into_iter().rev() {
        result.replace_range(start..end, replacement);
    }
    ensure!(
        result.len() <= crate::git::MAX_REVIEW_BYTES,
        "LSP 结果超过编辑器大小限制"
    );
    Ok(result)
}

pub(crate) fn workspace_edits(value: &Value) -> Result<Vec<(String, Option<i32>, Vec<Edit>)>> {
    let mut changes = Vec::new();
    if let Some(items) = value["changes"].as_object() {
        for (uri, list) in items {
            changes.push((uri.clone(), None, serde_json::from_value(list.clone())?));
        }
    }
    if let Some(items) = value["documentChanges"].as_array() {
        for item in items {
            let uri = item["textDocument"]["uri"]
                .as_str()
                .context("LSP 返回了文件创建、移动或删除请求")?;
            let version = item["textDocument"]["version"]
                .as_i64()
                .and_then(|ver| i32::try_from(ver).ok());
            changes.push((
                uri.to_owned(),
                version,
                serde_json::from_value(item["edits"].clone())?,
            ));
        }
    }
    let mut seen = std::collections::HashSet::new();
    ensure!(
        changes.iter().all(|(uri, _, _)| seen.insert(uri.clone())),
        "LSP 返回了重复的文件修改"
    );
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_validate_utf16_and_preserve_line_endings() {
        let edits: Vec<Edit> = serde_json::from_str(r#"[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"newText":"云"},{"range":{"start":{"line":2,"character":0},"end":{"line":2,"character":0}},"newText":"last"}]"#).unwrap();
        assert_eq!(
            apply_edits("a😀b\r\nline\r\n", &edits).unwrap(),
            "a云b\r\nline\r\nlast"
        );
        assert!(utf16_to_byte("😀", 1).is_err());
        assert!(apply_edits("a😀b\r\nline\r\n", &[edits[0].clone(), edits[0].clone()]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn file_uri_roundtrips_unicode_spaces_and_percent() {
        let path = Path::new("/tmp/中文 #%/src");
        let uri = file_uri(path).unwrap();
        assert!(uri.contains("%20%23%25"));
        assert_eq!(uri_path(&uri).unwrap(), path);
        assert!(uri_path("https://example.invalid/a").is_err());
    }
}
