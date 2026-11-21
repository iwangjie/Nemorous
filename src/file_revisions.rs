use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::git::run_git_bytes;

const MAX_REVISIONS: usize = 100;

const LOG_FORMAT: &str = "--format=%x00%H%x00%h%x00%an%x00%aI%x00%s%x00";

/// One commit's view of a followed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    /// Repo-relative path of the file at this revision.
    ///
    /// For a rename revision this is the rename target. Older revisions use
    /// the rename source path.
    pub path: String,
    /// Rename source path when this revision is a rename; empty otherwise.
    pub orig_path: String,
    /// Exact `--name-status` code, for example `M`, `A`, `D`, or `R100`.
    pub status: String,
    pub exists: bool,
}

pub fn list(repo: &Path, rev: &str, current_path: &str, limit: usize) -> Result<Vec<Revision>> {
    let limit = limit.min(MAX_REVISIONS);
    if limit == 0 {
        return Ok(Vec::new());
    }
    let rev_or_head = if rev.trim().is_empty() { "HEAD" } else { rev };
    let count_arg = format!("-n{limit}");

    let args = [
        "--literal-pathspecs",
        "log",
        "--no-color",
        "--follow",
        "--name-status",
        "-z",
        LOG_FORMAT,
        count_arg.as_str(),
        "--end-of-options",
        rev_or_head,
        "--",
        current_path,
    ];

    let output = run_git_bytes(repo, &args).with_context(|| {
        format!(
            "reading file revisions for {:?} at {:?}",
            current_path, rev_or_head
        )
    })?;

    parse_log(&output, current_path, limit)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusEntry {
    status: String,
    path: String,
    orig_path: String,
}

fn parse_log(bytes: &[u8], current_path: &str, limit: usize) -> Result<Vec<Revision>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let tokens: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    let mut index = 0usize;
    let mut tracked_path = current_path.to_owned();
    let mut revisions = Vec::new();

    while revisions.len() < limit {
        while index < tokens.len()
            && (tokens[index].is_empty() || is_blank_line_token(tokens[index]))
        {
            index += 1;
        }

        if index >= tokens.len() {
            break;
        }

        if index + 5 > tokens.len() {
            bail!("malformed git log output: incomplete commit metadata");
        }

        let hash = token_string(tokens[index]);
        if !matches!(hash.len(), 40 | 64) || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid revision hash in git log");
        }
        let short_hash = token_string(tokens[index + 1]);
        let author = token_string(tokens[index + 2]);
        let date = token_string(tokens[index + 3]);
        let subject = token_string(tokens[index + 4]);
        index += 5;

        if tokens.get(index).is_some_and(|token| token.is_empty()) {
            index += 1;
        }
        let entries = parse_status_entries(&tokens, &mut index)?;

        let Some(entry) = entries.iter().find(|entry| entry.path == tracked_path) else {
            continue;
        };

        let exists = !is_deletion_status(&entry.status);

        revisions.push(Revision {
            hash,
            short_hash,
            author,
            date,
            subject,
            path: entry.path.clone(),
            orig_path: entry.orig_path.clone(),
            status: entry.status.clone(),
            exists,
        });

        if is_rename_status(&entry.status) && !entry.orig_path.is_empty() {
            tracked_path = entry.orig_path.clone();
        }
    }

    Ok(revisions)
}

fn parse_status_entries(tokens: &[&[u8]], index: &mut usize) -> Result<Vec<StatusEntry>> {
    let mut entries = Vec::new();

    while *index < tokens.len() {
        let token = tokens[*index];

        if token.is_empty() {
            *index += 1;
            break;
        }

        if is_blank_line_token(token) {
            *index += 1;
            continue;
        }

        let status_bytes = token.strip_prefix(b"\n").unwrap_or(token);

        if status_bytes.is_empty() || !is_name_status_code(status_bytes[0]) {
            bail!(
                "malformed git log output: expected name-status record, got {:?}",
                String::from_utf8_lossy(token)
            );
        }

        let status = token_string(status_bytes);
        let needs_two_paths = matches!(status_bytes[0], b'R' | b'C');

        *index += 1;

        if *index >= tokens.len() {
            bail!("malformed git log output: name-status record is missing a path");
        }

        if tokens[*index].is_empty() {
            bail!("malformed git log output: name-status record has an empty path");
        }

        let first_path = token_string(tokens[*index]);
        *index += 1;

        if needs_two_paths {
            if *index >= tokens.len() {
                bail!("malformed git log output: rename/copy record is missing its target path");
            }

            if tokens[*index].is_empty() {
                bail!("malformed git log output: rename/copy record has an empty target path");
            }

            let second_path = token_string(tokens[*index]);
            *index += 1;

            entries.push(StatusEntry {
                status,
                path: second_path,
                orig_path: first_path,
            });
        } else {
            entries.push(StatusEntry {
                status,
                path: first_path,
                orig_path: String::new(),
            });
        }
    }

    Ok(entries)
}

fn token_string(token: &[u8]) -> String {
    String::from_utf8_lossy(token).into_owned()
}

fn is_name_status_code(byte: u8) -> bool {
    matches!(
        byte,
        b'A' | b'B' | b'C' | b'D' | b'M' | b'R' | b'T' | b'U' | b'X'
    )
}

fn is_blank_line_token(token: &[u8]) -> bool {
    !token.is_empty() && token.iter().all(|byte| *byte == b'\n' || *byte == b'\r')
}

fn is_deletion_status(status: &str) -> bool {
    status.as_bytes().first() == Some(&b'D')
}

fn is_rename_status(status: &str) -> bool {
    status.as_bytes().first() == Some(&b'R')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_field(out: &mut Vec<u8>, value: &str) {
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }

    fn push_commit(
        out: &mut Vec<u8>,
        hash: &str,
        short_hash: &str,
        author: &str,
        date: &str,
        subject: &str,
    ) {
        push_field(out, "");
        push_field(out, hash);
        push_field(out, short_hash);
        push_field(out, author);
        push_field(out, date);
        push_field(out, subject);
        push_field(out, "");
    }

    #[test]
    fn follows_rename_preserves_paths_and_rejects_malformed_data() {
        let old_path = "old\n naïve file.txt ";
        let new_path = "new\n naïve file.txt ";

        let mut log = Vec::new();

        push_commit(
            &mut log,
            "2222222222222222222222222222222222222222",
            "2222222",
            "Ada Lovelace",
            "2024-01-02T03:04:05+00:00",
            "rename with unicode and spaces",
        );
        push_field(&mut log, "\nR100");
        push_field(&mut log, old_path);
        push_field(&mut log, new_path);

        push_commit(
            &mut log,
            "1111111111111111111111111111111111111111",
            "",
            "",
            "",
            "",
        );
        push_field(&mut log, "\nA");
        push_field(&mut log, old_path);

        let revisions = parse_log(&log, new_path, MAX_REVISIONS).unwrap();

        assert_eq!(revisions.len(), 2);

        assert_eq!(
            revisions[0].hash,
            "2222222222222222222222222222222222222222"
        );
        assert_eq!(revisions[0].short_hash, "2222222");
        assert_eq!(revisions[0].author, "Ada Lovelace");
        assert_eq!(revisions[0].date, "2024-01-02T03:04:05+00:00");
        assert_eq!(revisions[0].subject, "rename with unicode and spaces");
        assert_eq!(revisions[0].path, new_path);
        assert_eq!(revisions[0].orig_path, old_path);
        assert_eq!(revisions[0].status, "R100");
        assert!(revisions[0].exists);

        assert_eq!(
            revisions[1].hash,
            "1111111111111111111111111111111111111111"
        );
        assert!(revisions[1].short_hash.is_empty());
        assert!(revisions[1].author.is_empty());
        assert!(revisions[1].date.is_empty());
        assert!(revisions[1].subject.is_empty());
        assert_eq!(revisions[1].path, old_path);
        assert!(revisions[1].orig_path.is_empty());
        assert_eq!(revisions[1].status, "A");
        assert!(revisions[1].exists);

        let mut deletion = Vec::new();
        push_commit(
            &mut deletion,
            "3333333333333333333333333333333333333333",
            "3333333",
            "Grace Hopper",
            "2024-02-03T04:05:06+00:00",
            "delete file",
        );
        push_field(&mut deletion, "\nD");
        push_field(&mut deletion, old_path);

        let revisions = parse_log(&deletion, old_path, MAX_REVISIONS).unwrap();
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].path, old_path);
        assert_eq!(revisions[0].status, "D");
        assert!(!revisions[0].exists);

        assert!(parse_log(b"\0only", new_path, MAX_REVISIONS).is_err());
        assert!(
            parse_log(
                b"\0h\0s\0a\0d\0subject\0\nR100\0old\0",
                new_path,
                MAX_REVISIONS,
            )
            .is_err()
        );
    }
}
