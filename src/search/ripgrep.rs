use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::CancelToken;

const MAX_ARGV_BYTES: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn matching_files(
    binary: &Path,
    candidates: &[PathBuf],
    pattern: &str,
    case_sensitive: bool,
    multiline: bool,
    cancel: &CancelToken,
) -> io::Result<HashSet<PathBuf>> {
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }
    cancelled(cancel)?;

    let paths: HashMap<_, _> = candidates
        .iter()
        .map(|path| (path.as_os_str().as_encoded_bytes(), path))
        .collect();
    let budget: usize = paths.keys().map(|path| path.len() + 1).sum();
    if budget + pattern.len() + 1024 > MAX_ARGV_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "rg argument budget exceeded",
        ));
    }

    let mut command = Command::new(binary);
    command
        .arg("--no-config")
        .arg("--no-ignore")
        .arg("--hidden")
        .arg("--encoding")
        .arg("none")
        .arg("--null")
        .arg("--files-with-matches")
        .arg("--text")
        .arg(if case_sensitive {
            "--case-sensitive"
        } else {
            "--ignore-case"
        });

    if multiline {
        command.arg("--multiline");
    }

    command.arg("--regexp").arg(pattern).arg("--");
    for path in candidates {
        command.arg(path);
    }

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let pipe = child.stdout.take().expect("piped rg stdout");
    let reader = thread::Builder::new().spawn(move || {
        let mut output = Vec::new();
        pipe.take(budget as u64 + 1).read_to_end(&mut output)?;
        Ok::<_, io::Error>(output)
    });
    let deadline = Instant::now() + TIMEOUT;
    let status = reader
        .as_ref()
        .map_err(|err| io::Error::other(err.to_string()))
        .and_then(|_| {
            loop {
                cancelled(cancel)?;
                if let Some(status) = child.try_wait()? {
                    break Ok(status);
                }
                if Instant::now() >= deadline {
                    break Err(io::Error::new(io::ErrorKind::TimedOut, "rg timed out"));
                }
                thread::sleep(POLL_INTERVAL);
            }
        });
    if status.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let output = reader?
        .join()
        .map_err(|_| io::Error::other("rg reader disconnected"))??;
    if !matches!(status?.code(), Some(0 | 1)) {
        return Err(io::Error::other("rg returned an unsuccessful status"));
    }
    if output.len() > budget || (!output.is_empty() && output.last() != Some(&0)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid rg path list",
        ));
    }
    output
        .strip_suffix(&[0])
        .unwrap_or_default()
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            paths
                .get(raw)
                .map(|path| (*path).clone())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unexpected rg path"))
        })
        .collect()
}

fn cancelled(cancel: &CancelToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        Err(io::Error::new(io::ErrorKind::Interrupted, "rg cancelled"))
    } else {
        Ok(())
    }
}
