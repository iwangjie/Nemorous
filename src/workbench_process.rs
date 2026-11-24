use std::collections::VecDeque;
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

const OUTPUT_LIMIT: usize = 512 * 1024;
const DIAGNOSTICS_LIMIT: usize = 16 * 1024;

pub(crate) struct ChildGroup {
    pub child: Child,
    stopped: bool,
    #[cfg(windows)]
    job: std::os::windows::io::OwnedHandle,
}

impl ChildGroup {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(command, 0);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let child = command.spawn().context("无法启动命令")?;
        #[cfg(windows)]
        let job = match windows_job::attach(&child) {
            Ok(job) => job,
            Err(error) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("无法接管命令进程树");
            }
        };
        Ok(Self {
            child,
            stopped: false,
            #[cfg(windows)]
            job,
        })
    }

    pub(crate) fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        {
            let group = -(self.child.id() as libc::pid_t);
            // The command started a dedicated process group. Negative pid addresses that group.
            unsafe { libc::kill(group, libc::SIGKILL) };
        }
        #[cfg(windows)]
        windows_job::terminate(&self.job);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildGroup {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(windows)]
mod windows_job {
    use std::io;
    use std::mem::size_of_val;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::Child;
    use std::ptr::null;

    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub(super) fn attach(child: &Child) -> io::Result<OwnedHandle> {
        let raw = unsafe { CreateJobObjectW(null(), null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&info) as u32,
            )
        };
        if configured == 0 || unsafe { AssignProcessToJobObject(raw, child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(super) fn terminate(job: &OwnedHandle) {
        unsafe { TerminateJobObject(job.as_raw_handle(), 1) };
    }
}

#[derive(Default)]
struct Capture {
    bytes: VecDeque<u8>,
    dropped: bool,
}

impl Capture {
    fn push(&mut self, bytes: &[u8], limit: usize) {
        let excess = self.bytes.len() + bytes.len();
        if excess > limit {
            self.bytes.drain(..(excess - limit));
            self.dropped = true;
        }
        self.bytes.extend(bytes);
    }
}

pub(crate) struct ProcessDiagnostics {
    capture: Arc<Mutex<Capture>>,
    reader: JoinHandle<io::Result<()>>,
}

pub(crate) struct ProcessReport {
    pub status: Option<ExitStatus>,
    pub stderr: String,
    pub finished: bool,
}

impl ProcessDiagnostics {
    pub(crate) fn capture(child: &mut ChildGroup) -> Result<Self> {
        let pipe = child
            .child
            .stderr
            .take()
            .context("无法读取适配器的错误输出")?;
        let capture = Arc::new(Mutex::new(Capture::default()));
        let reader = read_output(pipe, &capture, DIAGNOSTICS_LIMIT)?;
        Ok(Self { capture, reader })
    }

    pub(crate) fn report(&self, child: &mut ChildGroup) -> ProcessReport {
        let status = child.child.try_wait().ok().flatten();
        let finished = self.reader.is_finished();
        let capture = self.capture.lock().unwrap_or_else(|err| err.into_inner());
        let stderr = String::from_utf8_lossy(&capture.bytes.iter().copied().collect::<Vec<_>>())
            .trim()
            .to_owned();
        ProcessReport {
            status,
            stderr,
            finished,
        }
    }
}

pub(crate) struct RunProcess {
    group: ChildGroup,
    readers: Vec<JoinHandle<io::Result<()>>>,
    capture: Arc<Mutex<Capture>>,
    status: Option<ExitStatus>,
    exited: Option<Instant>,
}

pub(crate) struct RunUpdate {
    pub output: Vec<u8>,
    pub truncated: bool,
    pub status: Option<ExitStatus>,
}

impl RunProcess {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self> {
        let mut group = ChildGroup::spawn(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )?;
        let capture = Arc::new(Mutex::new(Capture::default()));
        let stdout = group.child.stdout.take().context("无法读取标准输出")?;
        let stderr = group.child.stderr.take().context("无法读取错误输出")?;
        let readers = vec![
            read_output(stdout, &capture, OUTPUT_LIMIT)?,
            read_output(stderr, &capture, OUTPUT_LIMIT)?,
        ];
        Ok(Self {
            group,
            readers,
            capture,
            status: None,
            exited: None,
        })
    }

    pub(crate) fn poll(&mut self) -> Result<RunUpdate> {
        if self.status.is_none() {
            self.status = self.group.child.try_wait()?;
            if self.status.is_some() {
                self.exited = Some(Instant::now());
            }
        }
        let readers_done = self.readers.iter().all(JoinHandle::is_finished);
        if self
            .exited
            .is_some_and(|start| start.elapsed() >= Duration::from_millis(400))
            && !readers_done
        {
            self.group.stop();
        }
        let status = if readers_done { self.status } else { None };
        if status.is_some() {
            for reader in self.readers.drain(..) {
                reader
                    .join()
                    .map_err(|_| anyhow::anyhow!("进程输出读取失败"))??;
            }
            self.group.stop();
        }
        let mut capture = self.capture.lock().unwrap_or_else(|err| err.into_inner());
        Ok(RunUpdate {
            output: capture.bytes.drain(..).collect(),
            truncated: std::mem::take(&mut capture.dropped),
            status,
        })
    }

    pub(crate) fn stop(&mut self) {
        self.group.stop();
    }
}

fn read_output(
    mut reader: impl Read + Send + 'static,
    capture: &Arc<Mutex<Capture>>,
    limit: usize,
) -> io::Result<JoinHandle<io::Result<()>>> {
    let capture = Arc::clone(capture);
    thread::Builder::new()
        .name("nemorous-process-output".into())
        .spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(count) => capture
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .push(&buffer[..count], limit),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
        })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn streams_both_pipes_and_reports_exit() {
        let mut child = RunProcess::spawn(
            Command::new("/bin/sh").args(["-c", "printf 'first\n'; printf 'second\n' >&2; exit 7"]),
        )
        .unwrap();
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let update = child.poll().unwrap();
            output.extend(update.output);
            if let Some(status) = update.status {
                assert_eq!(status.code(), Some(7));
                break;
            }
            assert!(Instant::now() < deadline, "command did not exit");
            thread::sleep(Duration::from_millis(10));
        }
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("first\n") && output.contains("second\n"));
    }

    #[test]
    fn stop_releases_pipes_inherited_by_a_child() {
        let mut child = RunProcess::spawn(
            Command::new("/bin/sh").args(["-c", "sleep 20 & printf '%s\n' \"$!\"; wait"]),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if !child.poll().unwrap().output.is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "shell did not launch a child");
            thread::sleep(Duration::from_millis(10));
        }
        child.stop();
        while child.poll().unwrap().status.is_none() {
            assert!(Instant::now() < deadline, "descendant retained output pipe");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
