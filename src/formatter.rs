use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

use crate::git::{LineEnding, MAX_REVIEW_BYTES};

const TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_DIAGNOSTIC_BYTES: u64 = 8 * 1024;

pub fn format_source(
    repo: &Path,
    rel_path: &str,
    custom_command: &str,
    text: &str,
) -> Result<Option<String>> {
    let file = Path::new(rel_path);
    let extension = file
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let explicit = !custom_command.trim().is_empty();
    let commands: Vec<Vec<String>> = if explicit {
        vec![split_legacy_command(custom_command)?]
    } else {
        default_commands(&extension)
            .iter()
            .map(|words| words.iter().map(|word| (*word).to_string()).collect())
            .collect()
    };
    if commands.is_empty() {
        return Ok(None);
    }
    if text.len() > MAX_REVIEW_BYTES {
        bail!("文件超过格式化上限");
    }
    if !file
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        bail!("格式化要求仓库内的文件路径");
    }
    let name = file.file_name().context("缺少要格式化的文件名")?;
    let repo = repo.canonicalize().context("找不到仓库目录")?;
    let parent = repo
        .join(file.parent().unwrap_or_else(|| Path::new("")))
        .canonicalize()?;
    if !parent.starts_with(&repo) {
        bail!("格式化要求仓库内的文件路径");
    }
    let stage = Stage::new(&parent, name, text)?;

    for command in commands {
        let Some((program, args)) = command
            .split_first()
            .filter(|(program, _)| !program.is_empty())
        else {
            bail!("格式化命令为空");
        };
        let mut argv: Vec<_> = args
            .iter()
            .map(|arg| replace_placeholder(arg, &stage.input))
            .collect();
        if !args.iter().any(|arg| arg.contains("{}")) {
            argv.push(stage.input.as_os_str().to_owned());
        }
        match run_formatter(&repo, program, &argv) {
            Ok(()) => return read_output(&stage.input).map(Some),
            Err(err) if !explicit && err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err).with_context(|| format!("格式化命令 {program}")),
        }
    }
    Ok(None)
}

fn default_commands(extension: &str) -> &'static [&'static [&'static str]] {
    match extension {
        "go" => &[&["gofmt", "-w"]],
        "js" | "jsx" | "ts" | "tsx" | "json" | "css" | "scss" | "md" | "html" | "yaml" | "yml" => {
            &[&["prettier", "--write"]]
        }
        "py" => &[&["ruff", "format"], &["black"]],
        "rs" => &[&["rustfmt", "--config", "skip_children=true"]],
        "java" => &[&["google-java-format", "-i"]],
        _ => &[],
    }
}

fn split_legacy_command(input: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = input.chars();

    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => bail!("unterminated single quote in formatter command"),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some(ch) => current.push(ch),
                        None => bail!("unterminated double quote in formatter command"),
                    }
                }
            }
            _ => {
                in_token = true;
                current.push(ch);
            }
        }
    }

    if in_token {
        tokens.push(current);
    }

    Ok(tokens)
}

fn replace_placeholder(argument: &str, stage: &Path) -> OsString {
    let mut output = OsString::new();
    let mut first = true;

    for piece in argument.split("{}") {
        if !first {
            output.push(stage.as_os_str());
        }
        first = false;
        output.push(piece);
    }

    output
}

struct Stage {
    dir: PathBuf,
    input: PathBuf,
}

impl Stage {
    fn new(parent: &Path, name: &std::ffi::OsStr, text: &str) -> Result<Self> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = parent.join(format!(
            ".nemorous-format-{}-{now}-{sequence}",
            std::process::id()
        ));
        let directory = fs::DirBuilder::new();
        #[cfg(unix)]
        let directory = {
            let mut directory = directory;
            directory.mode(0o700);
            directory
        };
        directory.create(&dir)?;
        let stage = Self {
            input: dir.join(name),
            dir,
        };
        let mut options = OpenOptions::new();
        #[cfg(unix)]
        options.mode(0o600);
        options
            .create_new(true)
            .write(true)
            .open(&stage.input)?
            .write_all(text.as_bytes())?;
        Ok(stage)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.input);
        let _ = fs::remove_dir(&self.dir);
    }
}

fn read_output(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_REVIEW_BYTES as u64 {
        bail!("格式化输出不是普通文本文件或已超过大小上限");
    }
    let mut raw = Vec::new();
    File::open(path)?
        .take(MAX_REVIEW_BYTES as u64 + 1)
        .read_to_end(&mut raw)?;
    if raw.len() > MAX_REVIEW_BYTES || raw.contains(&0) {
        bail!("格式化输出不是有效文本或已超过大小上限");
    }
    Ok(LineEnding::normalize_to_lf(&String::from_utf8(raw)?))
}

fn run_formatter(repo: &Path, executable: &str, args: &[OsString]) -> std::io::Result<()> {
    let mut child = crate::tool_command::command(executable)
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let pipe = child.stderr.take().expect("piped formatter stderr");
    let diagnostics = thread::Builder::new().spawn(move || {
        let mut captured = Vec::new();
        pipe.take(MAX_DIAGNOSTIC_BYTES).read_to_end(&mut captured)?;
        Ok::<_, std::io::Error>(captured)
    });
    let deadline = Instant::now() + TIMEOUT;
    let status = diagnostics
        .as_ref()
        .map_err(|err| std::io::Error::other(err.to_string()))
        .and_then(|_| {
            loop {
                if let Some(status) = child.try_wait()? {
                    break Ok(status);
                }
                if Instant::now() >= deadline {
                    break Err(std::io::Error::new(ErrorKind::TimedOut, "格式化超时"));
                }
                thread::sleep(POLL_INTERVAL);
            }
        });
    if status.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let status = status?;
    if status.success() {
        return Ok(());
    }
    let diagnostics = diagnostics?;
    while !diagnostics.is_finished() && Instant::now() < deadline {
        thread::sleep(POLL_INTERVAL);
    }
    let text = if diagnostics.is_finished() {
        diagnostics
            .join()
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let message = String::from_utf8_lossy(&text);
    let message = message.trim();
    Err(std::io::Error::other(if message.is_empty() {
        status.to_string()
    } else {
        message.to_string()
    }))
}
