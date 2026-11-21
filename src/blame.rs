use std::path::{Component, Path};

use anyhow::{Result, bail};

use crate::git::{BoundedGitOutput, run_git_limited};

const MAX_OUTPUT: usize = 8 * 1024 * 1024;
pub(crate) const MAX_LINES: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub number: u32,
    pub hash: String,
    pub author: String,
    pub author_unix: i64,
    pub summary: String,
}

pub fn read(
    repo: &Path,
    rel: &str,
    version: &str,
    first: u32,
    last: u32,
    contents: Option<Vec<u8>>,
) -> Result<Vec<Line>> {
    validate_version(version)?;
    validate_path(rel)?;
    if first == 0 || last < first {
        bail!("无效的 blame 行范围 {first}..{last}");
    }
    if last - first + 1 > MAX_LINES {
        bail!("blame 行范围超过 {MAX_LINES} 行");
    }

    let range = format!("{first},{last}");
    let mut args = vec![
        "--literal-pathspecs",
        "blame",
        "--line-porcelain",
        "--root",
        "-L",
        range.as_str(),
    ];
    if !version.is_empty() {
        args.push(version);
    }
    if let Some(text) = contents.as_ref() {
        anyhow::ensure!(
            version.is_empty() && text.len() <= crate::git::MAX_REVIEW_BYTES,
            "超出可归属的缓冲区范围"
        );
        args.extend(["--contents", "-"]);
    }
    args.extend(["--", rel]);

    let bytes = match run_git_limited(repo, &args, MAX_OUTPUT, contents)? {
        BoundedGitOutput::Ok(bytes) => bytes,
        BoundedGitOutput::TooLarge => bail!("blame 输出超过 8 MiB"),
        BoundedGitOutput::Failed { stderr, exit } => {
            if is_untracked(&stderr) {
                return Ok(Vec::new());
            }
            bail!("git blame 失败: {stderr} (exit {exit:?})")
        }
    };

    Ok(parse(&bytes))
}

fn validate_version(version: &str) -> Result<()> {
    if version.is_empty() || version == "HEAD" {
        return Ok(());
    }
    if matches!(version.len(), 40 | 64) && version.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    bail!("version 只允许 HEAD、空字符串或严格十六进制 SHA")
}

fn validate_path(rel: &str) -> Result<()> {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\0') {
        bail!("无效的仓库相对路径");
    }
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) if !part.is_empty() => {}
            _ => bail!("无效的仓库相对路径"),
        }
    }
    Ok(())
}

fn parse(bytes: &[u8]) -> Vec<Line> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    let mut cur: Option<Line> = None;

    for raw in text.lines() {
        if raw.starts_with('\t') {
            out.extend(cur.take());
            continue;
        }
        if let Some((hash, number)) = parse_header(raw) {
            cur = Some(Line {
                number,
                hash: if hash.bytes().all(|b| b == b'0') {
                    String::new()
                } else {
                    hash.to_string()
                },
                author: String::new(),
                author_unix: 0,
                summary: String::new(),
            });
            continue;
        }
        let Some(line) = cur.as_mut() else {
            continue;
        };
        if let Some(v) = raw.strip_prefix("author ") {
            line.author.push_str(v);
        } else if let Some(v) = raw.strip_prefix("author-time ") {
            line.author_unix = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = raw.strip_prefix("summary ") {
            line.summary.push_str(v);
        }
    }
    out
}

fn parse_header(raw: &str) -> Option<(&str, u32)> {
    let mut parts = raw.split(' ');
    let mut hash = parts.next()?;
    if let Some(rest) = hash.strip_prefix('^') {
        hash = rest;
    }
    if !matches!(hash.len(), 40 | 64) || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    parts.next()?.parse::<u32>().ok()?;
    let number = parts.next()?.parse::<u32>().ok()?;
    if number == 0
        || parts
            .next()
            .is_some_and(|field| field.parse::<u32>().is_err())
    {
        return None;
    }
    Some((hash, number))
}

fn is_untracked(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("no such path")
        || s.contains("not tracked")
        || s.contains("has no commits yet")
        || s.contains("has only")
        || s.contains("bad revision 'head'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Blame Tester")
            .env("GIT_AUTHOR_EMAIL", "blame@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_NAME", "Blame Tester")
            .env("GIT_COMMITTER_EMAIL", "blame@example.com")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn reads_real_line_porcelain() {
        let dir = std::env::temp_dir().join(format!(
            "omp-blame-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        fs::write(dir.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "初始提交"]);

        let lines = read(&dir, "a.txt", "HEAD", 2, 2, None).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].number, 2);
        assert_eq!(lines[0].hash.len(), 40);
        assert_eq!(lines[0].author, "Blame Tester");
        assert_eq!(lines[0].author_unix, 1_700_000_000);
        assert_eq!(lines[0].summary, "初始提交");

        fs::write(dir.join("a.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
        let local = read(&dir, "a.txt", "", 4, 4, None).unwrap();
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].number, 4);
        assert!(local[0].hash.is_empty());

        let unsaved = b"inserted
alpha
beta
gamma
delta
"
        .to_vec();
        let attribution = read(&dir, "a.txt", "", 1, 5, Some(unsaved)).unwrap();
        assert!(attribution[0].hash.is_empty());
        assert_eq!(attribution[2].hash, lines[0].hash);
        assert!(attribution[4].hash.is_empty());
        assert_eq!(
            fs::read(dir.join("a.txt")).unwrap(),
            b"alpha
beta
gamma
delta
"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
