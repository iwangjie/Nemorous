#![cfg(unix)]

use anyhow::Context;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// Maximum bytes returned by one `read_available` call.
const READ_LIMIT: usize = 64 * 1024;
const READ_BUF_SIZE: usize = 8 * 1024;
const WRITE_POLL_MS: libc::c_int = 16;

pub struct Terminal {
    master: Option<OwnedFd>,
    child: Option<Child>,
    pgid: libc::pid_t,
    status: Option<ExitStatus>,
    stopped: bool,
}

impl Terminal {
    pub fn spawn(cwd: &Path, shell: &Path, rows: u16, columns: u16) -> anyhow::Result<Self> {
        let mut master_fd: libc::c_int = -1;
        let mut slave_fd: libc::c_int = -1;
        let mut winsize = libc::winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let rc = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut winsize,
            )
        };
        if rc != 0 {
            return Err(anyhow::anyhow!(
                "openpty failed: {}",
                io::Error::last_os_error()
            ));
        }
        if master_fd < 0 || slave_fd < 0 {
            return Err(anyhow::anyhow!("openpty returned an invalid descriptor"));
        }

        // From here on, dropping either owned fd closes it on every error path.
        let master = unsafe { OwnedFd::from_raw_fd(master_fd) };
        let slave = unsafe { OwnedFd::from_raw_fd(slave_fd) };

        // Descriptors 0..=2 are intentionally inherited into the child and
        // overwritten in pre_exec. Only higher descriptors need CLOEXEC.
        if master.as_raw_fd() > 2 {
            set_cloexec(master.as_raw_fd()).context("setting master CLOEXEC")?;
        }
        if slave.as_raw_fd() > 2 {
            set_cloexec(slave.as_raw_fd()).context("setting slave CLOEXEC")?;
        }
        set_nonblocking(master.as_raw_fd()).context("setting master nonblocking")?;

        let master_raw = master.as_raw_fd();
        let slave_raw = slave.as_raw_fd();

        let mut command = Command::new(shell);
        command.current_dir(cwd).env("TERM", "xterm-256color");

        // Only async-signal-safe syscalls are used in the child between fork
        // and exec. The slave becomes the controlling tty and all stdio.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(slave_raw, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }

                for target in [0, 1, 2] {
                    if slave_raw != target && libc::dup2(slave_raw, target) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                }

                // CLOEXEC also covers failure of these best-effort closes.
                if slave_raw > 2 && slave_raw != master_raw {
                    libc::close(slave_raw);
                }
                if master_raw > 2 && master_raw != slave_raw {
                    libc::close(master_raw);
                }

                Ok(())
            });
        }

        let child = command
            .spawn()
            .with_context(|| format!("spawning shell {} in {}", shell.display(), cwd.display()))?;
        drop(slave);

        let pgid = child.id() as libc::pid_t;
        Ok(Self {
            master: Some(master),
            child: Some(child),
            pgid,
            status: None,
            stopped: false,
        })
    }

    pub fn resize(&self, rows: u16, columns: u16) {
        let Some(fd) = self.master.as_ref().map(|fd| fd.as_raw_fd()) else {
            return;
        };
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        loop {
            let rc = unsafe {
                libc::ioctl(
                    fd,
                    libc::TIOCSWINSZ as libc::c_ulong,
                    &winsize as *const libc::winsize,
                )
            };
            if rc == 0 {
                return;
            }
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
    }

    /// Best-effort raw write. The fd is nonblocking, so a full PTY input queue
    /// causes a short bounded wait rather than an indefinite block.
    pub fn write(&self, bytes: &[u8]) {
        let Some(fd) = self.master.as_ref().map(|fd| fd.as_raw_fd()) else {
            return;
        };
        let mut written = 0usize;
        let deadline = Instant::now() + Duration::from_millis(32);

        while written < bytes.len() && Instant::now() < deadline {
            let n = unsafe {
                libc::write(
                    fd,
                    bytes[written..].as_ptr().cast::<libc::c_void>(),
                    bytes.len() - written,
                )
            };
            if n > 0 {
                written += n as usize;
                continue;
            }
            if n == 0 {
                return;
            }

            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            let code = err.raw_os_error();
            if code == Some(libc::EAGAIN) || code == Some(libc::EWOULDBLOCK) {
                let mut pollfd = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let rc = unsafe { libc::poll(&mut pollfd, 1, WRITE_POLL_MS) };
                if rc < 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                if rc == 0 {
                    return;
                }
                if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    return;
                }
                continue;
            }
            return;
        }
    }

    /// Nonblocking read. At most `READ_LIMIT` bytes are returned per call.
    /// EIO/EOF closes the master fd; any remaining kernel-buffered bytes were
    /// already returned by the loop before the close.
    pub fn read_available(&mut self) -> Vec<u8> {
        let Some(fd) = self.master.as_ref().map(|fd| fd.as_raw_fd()) else {
            return Vec::new();
        };

        let mut out = Vec::new();
        let mut buffer = [0u8; READ_BUF_SIZE];

        while out.len() < READ_LIMIT {
            let want = (READ_LIMIT - out.len()).min(buffer.len());
            let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast::<libc::c_void>(), want) };
            if n > 0 {
                out.extend_from_slice(&buffer[..n as usize]);
                continue;
            }
            if n == 0 {
                drop(self.master.take());
                break;
            }

            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            let code = err.raw_os_error();
            if code == Some(libc::EAGAIN) || code == Some(libc::EWOULDBLOCK) {
                break;
            }
            if code == Some(libc::EIO) {
                drop(self.master.take());
            }
            break;
        }

        out
    }

    fn try_status(&mut self) -> anyhow::Result<Option<ExitStatus>> {
        if let Some(status) = self.status.as_ref() {
            return Ok(Some(status.clone()));
        }
        if self.stopped {
            return Ok(None);
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        match child.try_wait().context("checking terminal child status")? {
            Some(status) => {
                self.status = Some(status.clone());
                Ok(Some(status))
            }
            None => Ok(None),
        }
    }

    pub fn exit_code(&mut self) -> anyhow::Result<Option<u32>> {
        Ok(self
            .try_status()?
            .map(|status| status.code().map_or(1, |code| code as u32)))
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;

        if let Some(master) = &self.master {
            let foreground = unsafe { libc::tcgetpgrp(master.as_raw_fd()) };
            if foreground > 0 && foreground != self.pgid {
                unsafe { libc::kill(-foreground, libc::SIGKILL) };
            }
        }
        if self.pgid > 0 && self.status.is_none() {
            unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        }
        drop(self.master.take());

        if let Some(mut child) = self.child.take() {
            if self.status.is_none() {
                let _ = child.kill();
                if let Ok(status) = child.wait() {
                    self.status = Some(status);
                }
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop();
    }
}

fn set_cloexec(fd: libc::c_int) -> io::Result<()> {
    let flags = fcntl_retry(fd, libc::F_GETFD, 0)?;
    fcntl_retry(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC)?;
    Ok(())
}

fn set_nonblocking(fd: libc::c_int) -> io::Result<()> {
    let flags = fcntl_retry(fd, libc::F_GETFL, 0)?;
    fcntl_retry(fd, libc::F_SETFL, flags | libc::O_NONBLOCK)?;
    Ok(())
}

fn fcntl_retry(
    fd: libc::c_int,
    command: libc::c_int,
    argument: libc::c_int,
) -> io::Result<libc::c_int> {
    loop {
        let rc = unsafe { libc::fcntl(fd, command, argument) };
        if rc != -1 {
            return Ok(rc);
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::ExitStatus;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);

            let nonce = NEXT.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "omp-workbench-terminal-{}-{nanos}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create synthetic working directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    fn read_until(terminal: &mut Terminal, needle: &[u8], timeout: Duration) -> Vec<u8> {
        const MAX_TEST_OUTPUT: usize = 1024 * 1024;

        let deadline = Instant::now() + timeout;
        let mut collected = Vec::new();

        loop {
            let chunk = terminal.read_available();
            assert!(
                chunk.len() <= READ_LIMIT,
                "read_available exceeded its bound"
            );

            if !chunk.is_empty() {
                let room = MAX_TEST_OUTPUT.saturating_sub(collected.len());
                if room > 0 {
                    collected.extend_from_slice(&chunk[..chunk.len().min(room)]);
                }
                if contains(&collected, needle) {
                    return collected;
                }
            }

            if Instant::now() >= deadline {
                panic!(
                    "timed out waiting for {:?}; output was {:?}",
                    String::from_utf8_lossy(needle),
                    String::from_utf8_lossy(&collected)
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_status(terminal: &mut Terminal, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        loop {
            output.extend(terminal.read_available());
            if let Some(status) = terminal.try_status().expect("poll child status") {
                return status;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for child exit: {:?}", String::from_utf8_lossy(&output));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn spawn_sets_tty_and_resize_is_visible_to_shell() {
        let directory = TempDir::new();
        let mut terminal = Terminal::spawn(directory.path(), Path::new("/bin/sh"), 24, 80)
            .expect("spawn terminal");

        terminal.resize(40, 132);
        terminal.write(
            b"if [ -t 0 ] && [ -t 1 ] && [ -t 2 ]; then \
              printf 'tty%s\\n' '-ok'; \
              else printf 'tty%s\\n' '-bad'; fi\n",
        );
        terminal.write(b"stty size\n");

        let output = read_until(&mut terminal, b"40 132", Duration::from_secs(5));
        assert!(
            contains(&output, b"tty-ok"),
            "child did not observe a tty on all stdio: {:?}",
            String::from_utf8_lossy(&output)
        );
        assert!(
            !contains(&output, b"tty-bad"),
            "child observed a non-tty stdio: {:?}",
            String::from_utf8_lossy(&output)
        );

        terminal.write(b"exit 0\n");
        let status = wait_for_status(&mut terminal, Duration::from_secs(5));
        assert!(status.success(), "unexpected shell status: {status:?}");

        terminal.stop();
        assert!(terminal.master.is_none());
    }

    #[test]
    fn stop_kills_and_reaps_without_waiting_for_the_process() {
        let directory = TempDir::new();
        let mut terminal = Terminal::spawn(directory.path(), Path::new("/bin/sh"), 24, 80)
            .expect("spawn terminal");

        terminal.write(b"printf 'ready%s\\n' '-for-exec'; exec sleep 30\n");
        let output = read_until(&mut terminal, b"ready-for-exec", Duration::from_secs(5));
        assert!(
            contains(&output, b"ready-for-exec"),
            "shell never reached the exec marker: {:?}",
            String::from_utf8_lossy(&output)
        );
        assert!(
            terminal.try_status().expect("status").is_none(),
            "sleeping child should still be running"
        );
        thread::sleep(Duration::from_millis(100));

        let started = Instant::now();
        terminal.stop();
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "stop blocked for {elapsed:?}"
        );
        assert!(terminal.master.is_none());
        assert!(
            terminal.try_status().expect("status").is_some(),
            "stopped child must have a reaped status"
        );
    }
}
