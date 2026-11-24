use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;

use crate::git::{BoundedGitOutput, CommitEntry, RepoInfo, run_git_bytes, run_git_limited};

const LOG_FORMAT: &str = "--format=%H%x00%h%x00%P%x00%an%x00%ae%x00%aI%x00%s%x00%D%x00%b";
const DEFAULT_LIMIT: usize = 80;
const MAX_LIMIT: usize = 500;
const FETCH_BATCH: usize = 100;
const MAX_SCANNED: usize = 2_000;
const MAX_LOG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub full_ref: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub branch: String,
    pub author: String,
    pub path: String,
    pub since_days: u32,
    pub text: String,
    pub literal_text: bool,
    pub case_sensitive: bool,
    pub limit: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub commits: Vec<CommitEntry>,
    pub more: bool,
    pub scan_limited: bool,
}

fn run_git(repo_path: &Path, args: &[String]) -> Result<Vec<u8>> {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match run_git_limited(repo_path, &refs, MAX_LOG_BYTES, None)? {
        BoundedGitOutput::Ok(bytes) => Ok(bytes),
        BoundedGitOutput::TooLarge => bail!("提交历史输出超过 16 MiB"),
        BoundedGitOutput::Failed { stderr, .. } => bail!("读取提交历史失败：{stderr}"),
    }
}

pub fn list_branches(repo: &RepoInfo) -> Result<Vec<Branch>> {
    let repo_path = Path::new(&repo.path);
    let out = run_git_bytes(
        repo_path,
        &[
            "for-each-ref",
            "--format=%(refname:short)%09%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut branches = Vec::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let Some((name, full_ref)) = line.split_once('\t') else {
            continue;
        };
        let (name, full_ref) = (name.trim(), full_ref.trim());
        if name.is_empty() || full_ref.is_empty() || full_ref.ends_with("/HEAD") {
            continue;
        }
        branches.push(Branch {
            name: name.to_string(),
            full_ref: full_ref.to_string(),
        });
    }
    Ok(branches)
}

fn git_config_value(repo_path: &Path, key: &str) -> Option<String> {
    run_git_bytes(repo_path, &["config", "--get", key])
        .ok()
        .map(|out| String::from_utf8_lossy(&out).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_commit_log(
    out: &[u8],
    repo: &RepoInfo,
    text_re: Option<&Regex>,
) -> Result<(usize, Vec<CommitEntry>)> {
    let out = out.strip_suffix(&[b'\n']).unwrap_or(out);
    let mut fields: Vec<&[u8]> = out.split(|byte| *byte == 0).collect();
    // git log -z terminates the last record too; tolerate versions that
    // only insert separators between records.
    if fields.len() % 9 == 1 && fields.last().map_or(false, |field| field.is_empty()) {
        fields.pop();
    }
    if fields.len() % 9 != 0 {
        return Err(anyhow!(
            "git log returned {} fields (not a multiple of 9)",
            fields.len()
        ));
    }

    let raw_count = fields.len() / 9;
    let mut commits = Vec::with_capacity(raw_count);
    let repo_path = Path::new(&repo.path).to_string_lossy().into_owned();

    for chunk in fields.chunks_exact(9) {
        let field = |index: usize| String::from_utf8_lossy(chunk[index]).into_owned();
        let hash = field(0);
        let short_hash = field(1);
        let parents = field(2).split_whitespace().map(str::to_string).collect();
        let author_name = field(3);
        let author_email = field(4);
        let date = field(5);
        let subject = field(6);
        let refs = field(7);
        let body = field(8);

        if let Some(re) = text_re {
            let haystack = format!(
                "{subject}\n{body}\n{hash}\n{short_hash}\n{author_name}\n{author_email}\n{refs}"
            );
            if !re.is_match(&haystack) {
                continue;
            }
        }

        commits.push(CommitEntry {
            repo_id: repo.id.clone(),
            repo_name: repo.label().to_owned(),
            repo_path: repo_path.clone(),
            hash,
            short_hash,
            parents,
            author: author_name,
            date,
            subject,
            refs,
        });
    }
    Ok((raw_count, commits))
}

pub fn query(repo: &RepoInfo, q: &Query) -> Result<Page> {
    let repo_path = Path::new(&repo.path);
    let limit = if q.limit == 0 {
        DEFAULT_LIMIT
    } else {
        q.limit.min(MAX_LIMIT)
    };

    let mut args = vec![
        "log".to_string(),
        "-z".to_string(),
        "--no-color".to_string(),
        "--no-show-signature".to_string(),
        "--topo-order".to_string(),
        LOG_FORMAT.to_owned(),
    ];

    let branch = q.branch.trim();
    let revision = if branch.is_empty() {
        args.push("--all".to_owned());
        run_git_bytes(repo_path, &["rev-parse", "--verify", "--quiet", "HEAD"])
            .is_ok()
            .then(|| "HEAD".to_owned())
    } else {
        Some(
            list_branches(repo)?
                .into_iter()
                .find(|candidate| candidate.name == branch || candidate.full_ref == branch)
                .ok_or_else(|| anyhow!("未找到分支：{branch}"))?
                .full_ref,
        )
    };

    let author = q.author.trim();
    if !author.is_empty() {
        let pattern = if author.eq_ignore_ascii_case("me") || author == "我" {
            if let Some(email) = git_config_value(repo_path, "user.email") {
                format!("<{}>", regex::escape(&email))
            } else if let Some(name) = git_config_value(repo_path, "user.name") {
                format!("^{} <", regex::escape(&name))
            } else {
                return Ok(Page::default());
            }
        } else {
            regex::escape(author)
        };
        args.extend([
            "--extended-regexp".to_owned(),
            "--regexp-ignore-case".to_owned(),
            format!("--author={pattern}"),
        ]);
    }

    if q.since_days > 0 {
        args.push(format!("--since={} days ago", q.since_days));
    }

    let path = q.path.trim().trim_start_matches("./");
    anyhow::ensure!(
        !path.starts_with(['/', ':'])
            && !path.contains('\0')
            && !path.split('/').any(|part| part == ".."),
        "请输入仓库内相对路径或 glob",
    );
    let pathspec = if path.is_empty() || path == "." {
        None
    } else if path.contains(['*', '?', '[']) {
        let prefix = if path.contains('/') { "" } else { "**/" };
        Some(format!(":(glob){prefix}{path}"))
    } else {
        Some(format!(":(literal){path}"))
    };

    let text_re = if q.text.trim().is_empty() {
        None
    } else {
        let raw = q.text.trim();
        let pattern = if q.literal_text { regex::escape(raw) } else { raw.to_owned() };
        let compile = |pattern: &str| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(!q.case_sensitive)
                .build()
        };
        Some(
            compile(&pattern)
                .or_else(|_| compile(&regex::escape(raw)))
                .context("compile commit text regex")?,
        )
    };

    let mut commits = Vec::new();
    let mut skip = 0usize;
    let mut more = false;

    let mut exhausted = false;
    while skip < MAX_SCANNED {
        // Author/path/since filtering runs in git, so --max-count counts
        // matching commits. Text regex filtering remains here and needs more
        // matching commits when it skips entries.
        let want = (limit + 1 - commits.len())
            .max(FETCH_BATCH)
            .min(MAX_SCANNED - skip);
        let mut batch_args = args.clone();
        batch_args.push(format!("--max-count={want}"));
        if skip > 0 {
            batch_args.push(format!("--skip={skip}"));
        }
        batch_args.push("--end-of-options".into());
        batch_args.extend(revision.iter().cloned());
        batch_args.push("--".into());
        batch_args.extend(pathspec.iter().cloned());

        let out = run_git(repo_path, &batch_args)?;
        let (raw, matched) = parse_commit_log(&out, repo, text_re.as_ref())?;
        skip += raw;
        commits.extend(matched);

        if commits.len() > limit {
            commits.truncate(limit);
            more = true;
            break;
        }
        if raw < want {
            exhausted = true;
            break;
        }
    }

    Ok(Page {
        commits,
        more,
        scan_limited: !exhausted && skip >= MAX_SCANNED && !more,
    })
}

pub(crate) fn lookup(repo: &RepoInfo, hash: &str) -> Result<CommitEntry> {
    anyhow::ensure!(
        matches!(hash.len(), 40 | 64) && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "无效的提交哈希"
    );
    let args = [
        "log",
        "-z",
        "-n1",
        "--no-color",
        "--no-show-signature",
        LOG_FORMAT,
        "--end-of-options",
        hash,
        "--",
    ]
    .map(str::to_owned);
    let out = run_git(Path::new(&repo.path), &args)?;
    parse_commit_log(&out, repo, None)?
        .1
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("未找到提交 {hash}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn fixture() -> (PathBuf, RepoInfo) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("omp-commit-forest-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(&dir, &["config", "user.name", "Test User"]);
        git(&dir, &["config", "user.email", "test@example.com"]);

        fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("other.txt"), "first\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "initial commit"]);

        fs::write(dir.join("main.rs"), "fn main() { println!(\"needle\"); }\n").unwrap();
        git(&dir, &["add", "main.rs"]);
        git(
            &dir,
            &[
                "commit",
                "-q",
                "-m",
                "second commit",
                "-m",
                "body has needle too",
            ],
        );
        git(&dir, &["branch", "feature"]);

        let repo = RepoInfo {
            id: "fixture".into(),
            name: "fixture".into(),
            display_name: None,
            path: dir.to_string_lossy().into_owned(),
            rel_path: String::new(),
            desc: String::new(),
            monogram: "fi".into(),
            color_idx: 0,
            is_root: true,
        };
        (dir, repo)
    }

    #[test]
    fn queries_real_git_history() {
        let (dir, repo) = fixture();

        let branches = list_branches(&repo).unwrap();
        assert!(
            branches
                .iter()
                .any(|b| b.name == "feature" && b.full_ref == "refs/heads/feature")
        );
        assert!(
            branches
                .iter()
                .any(|b| b.name == "main" && b.full_ref == "refs/heads/main")
        );

        let page = query(
            &repo,
            &Query {
                author: "me".into(),
                path: "main.rs".into(),
                text: "needle".into(),
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(page.commits.len(), 1);
        assert_eq!(page.commits[0].subject, "second commit");
        assert_eq!(page.commits[0].author, "Test User");
        assert!(!page.more);

        let page = query(
            &repo,
            &Query {
                branch: "feature".into(),
                path: "*.rs".into(),
                limit: 1,
                ..Query::default()
            },
        )
        .unwrap();
        assert_eq!(page.commits.len(), 1);
        assert!(page.more);

        let page = query(
            &repo,
            &Query {
                text: "[".into(),
                ..Query::default()
            },
        )
        .unwrap();
        assert!(page.commits.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }
}
