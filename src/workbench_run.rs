use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};

fn null_default<'de, D>(input: D) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<BTreeMap<String, String>>::deserialize(input)?.unwrap_or_default())
}

/// Upper bounds so a hostile/oversized config cannot allocate without limit.
const MAX_ARGS: usize = 4096;
const MAX_ARG_BYTES: usize = 32 * 1024;
const MAX_ENV_ENTRIES: usize = 4096;
const MAX_ENV_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Run,
    Test,
    Debug,
}

/// Persisted workbench run model. All legacy keys are lowercase and every
/// field is optional on the wire.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RunConfig {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Executable plus optional leading arguments, shell-free; may be a quoted
    /// path such as `"C:\Program Files\App\app.exe" --flag`.
    pub command: String,
    /// Space separated argument text using the same quoting rules as `command`.
    pub args: String,
    /// Empty => workspace, relative => joined to workspace, absolute => kept.
    pub cwd: String,
    #[serde(deserialize_with = "null_default")]
    pub env: BTreeMap<String, String>,
    pub adapter: String,
    pub adapter_args: String,
    pub network: String,
    pub address: String,
    pub program: String,
    pub program_args: String,
    pub launch: String,
}

/// Parses shell-free argument text.
///
/// Rules:
/// * ASCII whitespace separates arguments; `""` yields an explicit empty arg.
/// * `'...'` is literal (no escapes inside single quotes).
/// * `"..."` is literal except `\"` and `\\`, which yield `"` and `\`.
/// * Outside quotes `\` escapes whitespace, `"`, `'` and `\`; any other
///   backslash is literal so Windows paths such as `C:\tools\x` survive.
/// * Adjacent segments concatenate: `a"b c"d` => `ab cd`.
pub fn split_args(input: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut it = input.chars().peekable();

    while let Some(ch) = it.next() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                if started {
                    push_arg(&mut out, std::mem::take(&mut cur))?;
                    started = false;
                }
            }
            '\'' => {
                started = true;
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(c) => push_char(&mut cur, c)?,
                        None => bail!("unterminated single quote in argument text"),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => match it.peek().copied() {
                            Some(n @ ('"' | '\\')) => {
                                it.next();
                                push_char(&mut cur, n)?;
                            }
                            _ => push_char(&mut cur, '\\')?,
                        },
                        Some(c) => push_char(&mut cur, c)?,
                        None => bail!("unterminated double quote in argument text"),
                    }
                }
            }
            '\\' => {
                started = true;
                match it.peek().copied() {
                    Some(n) if matches!(n, '"' | '\'' | '\\' | ' ' | '\t' | '\r' | '\n') => {
                        it.next();
                        push_char(&mut cur, n)?;
                    }
                    _ => push_char(&mut cur, '\\')?,
                }
            }
            c => {
                started = true;
                push_char(&mut cur, c)?;
            }
        }
    }

    if started {
        push_arg(&mut out, cur)?;
    }
    Ok(out)
}

fn push_char(cur: &mut String, ch: char) -> Result<()> {
    if cur.len() + ch.len_utf8() > MAX_ARG_BYTES {
        bail!("argument exceeds {MAX_ARG_BYTES} bytes");
    }
    cur.push(ch);
    Ok(())
}

fn push_arg(out: &mut Vec<String>, arg: String) -> Result<()> {
    if out.len() >= MAX_ARGS {
        bail!("too many arguments (limit {MAX_ARGS})");
    }
    out.push(arg);
    Ok(())
}

/// Decodes `KEY=VALUE` lines into an ordered environment map.
pub fn env_pairs(input: &str) -> Result<BTreeMap<String, String>> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (index, raw) in input.lines().enumerate() {
        let line_no = index + 1;
        if raw.len() > MAX_ENV_LINE_BYTES {
            bail!("env line {line_no} exceeds {MAX_ENV_LINE_BYTES} bytes");
        }
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow!("env line {line_no}: expected KEY=VALUE"))?;
        let key = key.trim_end();
        if key.is_empty() {
            bail!("env line {line_no}: empty key");
        }
        if key
            .chars()
            .any(|c| c == '=' || c.is_whitespace() || c.is_control())
        {
            bail!("env line {line_no}: invalid key {key:?}");
        }
        if value.contains('\0') {
            bail!("env line {line_no}: value contains NUL");
        }
        if !map.contains_key(key) && map.len() >= MAX_ENV_ENTRIES {
            bail!("too many environment entries (limit {MAX_ENV_ENTRIES})");
        }
        if map.insert(key.to_owned(), value.to_owned()).is_some() {
            bail!("env line {line_no}: duplicate key {key:?}");
        }
    }
    Ok(map)
}

pub fn run_command(cfg: &RunConfig, workspace: &Path) -> Result<std::process::Command> {
    ensure!(cfg.kind != Kind::Debug, "调试配置需要 DAP 适配器");
    let cwd = workspace.join(cfg.cwd.trim());
    let direct = cfg.command.trim();
    ensure!(
        !direct.is_empty() && !direct.contains('\0'),
        "请输入可执行命令"
    );
    let mut command = if cwd.join(direct).is_file() || Path::new(direct).is_file() {
        let path = cwd.join(direct);
        std::process::Command::new(if path.is_file() {
            path
        } else {
            Path::new(direct).to_path_buf()
        })
    } else {
        let mut parts = split_args(direct)?.into_iter();
        let program = parts
            .next()
            .filter(|s| !s.is_empty())
            .context("请输入可执行命令")?;
        let mut command = std::process::Command::new(program);
        command.args(parts);
        command
    };
    // On Windows std::process applies its checked batch-file argument handling.
    command
        .args(split_args(&cfg.args)?)
        .current_dir(cwd)
        .envs(&cfg.env);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_environment_and_quoted_arguments() {
        let cfg: RunConfig = serde_json::from_value(serde_json::json!({
            "id": "qa", "kind": "test", "name": "Run fixture", "command": "go",
            "args": "-run \"Test A\"", "env": {"NEMOROUS_TEST": "space value"}
        }))
        .unwrap();
        let command = run_command(&cfg, Path::new(".")).unwrap();
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["-run", "Test A"]);
        assert_eq!(cfg.env["NEMOROUS_TEST"], "space value");
        assert!(serde_json::to_value(cfg).unwrap()["env"].is_object());
        assert!(
            serde_json::from_str::<RunConfig>(r#"{"env":null}"#)
                .unwrap()
                .env
                .is_empty()
        );
        assert_eq!(
            split_args(r#"C:\work\p "" '两 行'"#).unwrap(),
            [r"C:\work\p", "", "两 行"]
        );
        assert!(split_args("a 'unclosed").is_err());
        assert_eq!(env_pairs("A= value ").unwrap()["A"], " value ");
    }
}
