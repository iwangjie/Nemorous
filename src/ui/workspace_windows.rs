#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::fs::{self, File, OpenOptions, TryLockError};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use nsh_core::prelude::*;
    use nsh_platform_macos::objc2::{ClassType, msg_send, rc::Retained};
    use nsh_platform_macos::objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

    struct Registration {
        file: File,
        entry: PathBuf,
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.entry);
        }
    }

    fn directory() -> PathBuf {
        crate::config::config_dir().join("open-projects")
    }

    fn publish(slot: &mut Option<Registration>, path: &str) -> std::io::Result<()> {
        if path.is_empty() {
            slot.take();
            return Ok(());
        }
        if slot.is_none() {
            let dir = directory();
            fs::create_dir_all(&dir)?;
            let entry = dir.join(std::process::id().to_string());
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&entry)?;
            file.try_lock().map_err(std::io::Error::from)?;
            *slot = Some(Registration { file, entry });
        }
        let file = &mut slot.as_mut().expect("window registration").file;
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(path.as_bytes())
    }

    pub(super) fn observe() {
        let registration = Rc::new(RefCell::new(None));
        let workspace =
            computed(|| crate::state::app_signal().with(|s| s.cfg.workspace_path.clone()));
        effect(move || {
            workspace.with(|path| {
                if let Err(err) = publish(&mut registration.borrow_mut(), path) {
                    log::warn!("cannot publish open project: {err}");
                }
            })
        });
    }

    pub(super) fn opened() -> Vec<String> {
        live_entries().map(|(_, path)| path).collect()
    }

    fn live_entries() -> impl Iterator<Item = (i32, String)> {
        fs::read_dir(directory())
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
                if pid <= 0 || pid as u32 == std::process::id() {
                    return None;
                }
                if !entry.file_type().ok()?.is_file() {
                    return None;
                }
                let file = File::open(entry.path()).ok()?;
                if !matches!(file.try_lock_shared(), Err(TryLockError::WouldBlock)) {
                    return None;
                }
                let mut workspace = String::new();
                file.take(16 * 1024).read_to_string(&mut workspace).ok()?;
                Path::new(&workspace)
                    .is_absolute()
                    .then_some((pid, workspace))
            })
    }

    pub(super) fn focus(path: &str) -> bool {
        live_entries().filter(|(_, workspace)| path == workspace).any(|(pid, _)| {
            // AppKit takes a pid_t (i32 on macOS) and returns a nullable running application.
            let app: Option<Retained<NSRunningApplication>> = unsafe {
                msg_send![NSRunningApplication::class(), runningApplicationWithProcessIdentifier: pid]
            };
            app.is_some_and(|app| app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows))
        })
    }
}

#[cfg(target_os = "macos")]
pub(super) fn observe() {
    macos::observe();
}

#[cfg(not(target_os = "macos"))]
pub(super) fn observe() {}

pub(super) fn opened() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        macos::opened()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

pub(super) fn focus(_path: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::focus(_path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
