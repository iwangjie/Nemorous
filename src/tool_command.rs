use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let inherited = std::env::var_os("PATH");
    #[cfg(target_os = "macos")]
    return macos_search_dirs(inherited.as_deref());
    #[cfg(not(target_os = "macos"))]
    inherited
        .as_deref()
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn macos_search_dirs(inherited: Option<&OsStr>) -> Vec<PathBuf> {
    let system = OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin");
    let mut dirs: Vec<_> = std::env::split_paths(inherited.unwrap_or(system)).collect();
    // A desktop launch can omit directories that Homebrew adds to a login shell.
    let fallback = if cfg!(target_arch = "aarch64") {
        ["/opt/homebrew/bin", "/usr/local/bin"]
    } else {
        ["/usr/local/bin", "/opt/homebrew/bin"]
    };
    for dir in fallback.map(PathBuf::from) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let command = Command::new(program);
    #[cfg(target_os = "macos")]
    {
        let mut command = command;
        if let Ok(path) = std::env::join_paths(search_dirs()) {
            command.env("PATH", path);
        }
        command
    }
    #[cfg(not(target_os = "macos"))]
    command
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn desktop_fallback_preserves_configured_directory_precedence() {
        let configured = OsStr::new("/custom/bin:/usr/local/bin:/usr/bin");
        let dirs = macos_search_dirs(Some(configured));
        assert_eq!(
            dirs,
            [
                "/custom/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/opt/homebrew/bin"
            ]
            .map(PathBuf::from)
        );
        let defaults = macos_search_dirs(None);
        assert_eq!(defaults.first(), Some(&PathBuf::from("/usr/bin")));
        assert!(defaults.contains(&PathBuf::from("/opt/homebrew/bin")));
    }
}
