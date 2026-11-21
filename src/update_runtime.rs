use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::VerifyingKey;
use flate2::read::GzDecoder;

use crate::update_manifest::{self, MAX_MANIFEST_BYTES, Manifest};

const PUBLIC_KEY: &str = "ox37vmP3aFxe/cbZ19NuQUJd33TC0ZRn4TjtoCI1L2s=";
const FEED: &str = "https://github.com/iwangjie/Nemorous/releases/latest/download";
const MAX_INFLATED: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn platform() -> Result<(&'static str, &'static str)> {
    let system = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => bail!("当前平台没有更新通道"),
    };
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => bail!("当前架构没有更新通道"),
    };
    Ok((system, architecture))
}

fn urls(mirror: &str, original: &str) -> Result<Vec<String>> {
    update_manifest::validate_https_url(original)?;
    let mut sources = Vec::new();
    if !mirror.trim().is_empty() {
        let mirror = mirror.trim().trim_end_matches('/');
        update_manifest::validate_https_url(mirror)?;
        ensure!(!mirror.contains('?'), "更新镜像必须是 HTTPS 路径前缀");
        sources.push(format!("{mirror}/{original}"));
    }
    sources.push(original.to_owned());
    Ok(sources)
}

fn client(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_connect(Some(Duration::from_secs(8)))
        .timeout_global(Some(timeout))
        .user_agent(format!("Nemorous/{}", current_version()))
        .build()
        .into()
}

pub(crate) fn check(mirror: &str, skipped: Option<&str>) -> Result<Option<Manifest>> {
    let (system, arch) = platform()?;
    let feed = format!("{FEED}/update-{system}-{arch}.json");
    let agent = client(Duration::from_secs(45));
    for url in urls(mirror, &feed)? {
        let result = (|| -> Result<Manifest> {
            let response = agent.get(&url).call()?;
            let mut manifest = Vec::new();
            response
                .into_parts()
                .1
                .into_reader()
                .take((MAX_MANIFEST_BYTES + 1) as u64)
                .read_to_end(&mut manifest)?;
            update_manifest::parse(&manifest)
        })();
        if let Ok(manifest) = result {
            return Ok(manifest
                .newer_than(current_version(), skipped, false)?
                .then_some(manifest));
        }
    }
    bail!("无法读取有效的更新清单")
}

struct Scratch(PathBuf);

impl Scratch {
    fn create(parent: &Path) -> Result<Self> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = parent.join(format!(".nemorous-update-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn current_bundle() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let binary = std::env::current_exe().ok()?;
    let bundle = binary.parent()?.parent()?.parent()?;
    if bundle.extension()? != "app" || !bundle.join("Contents/Resources/nsh/app.json").is_file() {
        return None;
    }
    Some(bundle.to_owned())
}

fn download(
    scratch: &Scratch,
    manifest: &Manifest,
    mirror: &str,
    key: &VerifyingKey,
    cancel: &AtomicBool,
) -> Result<File> {
    let agent = client(Duration::from_secs(180));
    let path = scratch.0.join("asset.tar.gz");
    let mut last_error = None;
    for url in urls(mirror, &manifest.url)? {
        ensure!(!cancel.load(Ordering::Acquire), "更新已取消");
        let result = (|| -> Result<File> {
            let mut response = agent
                .get(&url)
                .call()?
                .into_parts()
                .1
                .into_reader()
                .take(manifest.size + 1);
            let mut file = File::create(&path)?;
            let mut bytes = [0; 64 * 1024];
            loop {
                ensure!(!cancel.load(Ordering::Acquire), "更新已取消");
                let n = response.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                file.write_all(&bytes[..n])?;
            }
            file.sync_all()?;
            manifest.verify_archive(File::open(&path)?, key)?;
            Ok(File::open(&path)?)
        })();
        match result {
            Ok(file) => return Ok(file),
            Err(error) => last_error = Some(error),
        };
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("没有可用的更新源")))
        .context("下载或校验更新失败")
}

fn extract(input: impl Read, directory: &Path) -> Result<PathBuf> {
    let mut archive = tar::Archive::new(GzDecoder::new(input).take(MAX_INFLATED + 1));
    let mut name = None;
    let mut count = 0;
    let mut total = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let mut parts = path.components().filter(|part| *part != Component::CurDir);
        let Some(Component::Normal(root)) = parts.next() else {
            bail!("更新包路径无效");
        };
        ensure!(
            parts.all(|part| matches!(part, Component::Normal(_))),
            "更新包含越界路径"
        );
        if let Some(expected) = &name {
            ensure!(expected == root, "更新包包含多个顶层目录");
        } else {
            name = Some(root.to_os_string());
        }
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "更新包含符号链接或特殊文件"
        );
        count += 1;
        total = total
            .checked_add(entry.header().size()?)
            .context("更新解包超出限制")?;
        ensure!(count <= 15_000 && total <= MAX_INFLATED, "更新解包超出限制");
        ensure!(entry.unpack_in(directory)?, "更新包含无效路径");
    }
    let path = directory.join(name.context("更新包为空")?);
    ensure!(
        path.extension().is_some_and(|suffix| suffix == "app") && path.is_dir(),
        "更新包缺少 App"
    );
    Ok(path)
}

fn validate(bundle: &Path, version: &semver::Version) -> Result<()> {
    use std::process::Command;
    let plist = bundle.join("Contents/Info.plist");
    let property = |key: &str| -> Result<String> {
        let out = Command::new("/usr/bin/plutil")
            .args(["-extract", key, "raw"])
            .arg(&plist)
            .output()?;
        ensure!(out.status.success(), "更新包缺少必要的 Info.plist 字段");
        Ok(String::from_utf8(out.stdout)?.trim().to_owned())
    };
    let id = property("CFBundleIdentifier")?;
    ensure!(
        matches!(id.as_str(), "dev.nemorous.app" | "com.nemorous.app"),
        "更新包的应用标识不匹配"
    );
    let got = semver::Version::parse(&property("CFBundleShortVersionString")?)?;
    ensure!(
        got.cmp_precedence(version).is_eq(),
        "更新包版本和清单不匹配"
    );
    ensure!(
        bundle.join("Contents/MacOS/Nemorous").is_file(),
        "更新包没有 Nemorous 可执行文件"
    );
    ensure!(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(bundle)
            .status()?
            .success(),
        "更新包的 macOS 应用签名校验失败"
    );
    Ok(())
}

pub(crate) struct Staged {
    scratch: Scratch,
    current: PathBuf,
    candidate: PathBuf,
    version: semver::Version,
    _lock: File,
}

pub(crate) fn stage(
    current: &Path,
    manifest: &Manifest,
    mirror: &str,
    cancel: &AtomicBool,
) -> Result<Staged> {
    stage_with(current, manifest, cancel, |scratch| {
        let bytes = STANDARD.decode(PUBLIC_KEY)?;
        let key = VerifyingKey::from_bytes(
            &bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("更新公钥无效"))?,
        )?;
        download(scratch, manifest, mirror, &key, cancel)
    })
}

fn stage_with<R: Read>(
    current: &Path,
    manifest: &Manifest,
    cancel: &AtomicBool,
    archive: impl FnOnce(&Scratch) -> Result<R>,
) -> Result<Staged> {
    ensure!(
        fs::symlink_metadata(current).is_ok_and(|meta| meta.is_dir() && !meta.is_symlink()),
        "当前应用包路径无效"
    );
    let parent = current.parent().context("无法查找应用目录")?;
    let scratch = Scratch::create(parent)?;
    let lock_path = parent.join(format!(
        ".{}.update.lock",
        current
            .file_name()
            .context("应用名称无效")?
            .to_string_lossy()
    ));
    ensure!(
        !fs::symlink_metadata(&lock_path).is_ok_and(|meta| meta.is_symlink()),
        "更新锁路径无效"
    );
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.try_lock().context("另一个窗口正在准备更新")?;
    let candidate = extract(archive(&scratch)?, &scratch.0)?;
    ensure!(!cancel.load(Ordering::Acquire), "更新已取消");
    validate(&candidate, &manifest.parsed_version)?;
    Ok(Staged {
        scratch,
        current: current.to_owned(),
        candidate,
        version: manifest.parsed_version.clone(),
        _lock: lock,
    })
}

impl Staged {
    pub(crate) fn apply(self) -> Result<PathBuf> {
        let basename = self
            .current
            .file_stem()
            .context("应用名称无效")?
            .to_string_lossy();
        let parent = self.current.parent().context("无法查找应用目录")?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let backup = parent.join(format!("{basename}-previous-{stamp}.app"));
        promote(&self.current, &self.candidate, &backup, || {
            validate(&self.current, &self.version)
        })
        .with_context(|| format!("更新应用失败；原包备份路径 {}", backup.display()))?;
        drop(self.scratch);
        Ok(backup)
    }
}

fn promote(
    current: &Path,
    candidate: &Path,
    backup: &Path,
    verify: impl FnOnce() -> Result<()>,
) -> Result<()> {
    ensure!(!backup.exists(), "旧版备份路径已存在");
    fs::rename(current, backup)?;
    if let Err(error) = fs::rename(candidate, current) {
        fs::rename(backup, current).context("无法自动恢复原包")?;
        return Err(error.into());
    }
    if let Err(error) = verify() {
        fs::rename(current, candidate).context("无法移出失败的候选包")?;
        fs::rename(backup, current).context("无法自动恢复原包")?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_promotion_failure_keeps_old_executable() {
        let work = Scratch::create(&std::env::temp_dir()).unwrap();
        let current = work.0.join("current");
        let incoming = work.0.join("incoming");
        let backup = work.0.join("backup");
        fs::write(&current, b"original executable").unwrap();
        fs::write(&incoming, b"replacement executable").unwrap();
        assert!(promote(&current, &incoming, &backup, || bail!("fixture rejection")).is_err());
        assert_eq!(fs::read(&current).unwrap(), b"original executable");
        assert_eq!(fs::read(&incoming).unwrap(), b"replacement executable");
        assert!(!backup.exists());
        promote(&current, &incoming, &backup, || Ok(())).unwrap();
        assert_eq!(fs::read(&current).unwrap(), b"replacement executable");
        assert_eq!(fs::read(&backup).unwrap(), b"original executable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires two signed AppKit release fixtures"]
    fn signed_app_install_and_go_rollback() {
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};
        use std::process::Command;

        fn cli(program: &str, args: &[&std::ffi::OsStr]) {
            let out = Command::new(program).args(args).output().unwrap();
            assert!(
                out.status.success(),
                "{} {}",
                program,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let legacy = PathBuf::from(std::env::var_os("NEMOROUS_TEST_GO_APP").unwrap());
        let rust = PathBuf::from(std::env::var_os("NEMOROUS_TEST_RUST_APP").unwrap());
        let root = Scratch::create(&std::env::temp_dir()).unwrap();
        let current = root.0.join("Nemorous.app");
        let candidate = root.0.join("fixture-new.app");
        cli("/usr/bin/ditto", &[legacy.as_os_str(), current.as_os_str()]);
        cli("/usr/bin/ditto", &[rust.as_os_str(), candidate.as_os_str()]);
        let current_exe = current.join("Contents/MacOS/Nemorous");
        let digest_file = |path: &Path| Sha256::digest(fs::read(path).unwrap());
        let legacy_digest = digest_file(&current_exe);
        let candidate_exe = candidate.join("Contents/MacOS/Nemorous");
        let plist = candidate.join("Contents/Info.plist");
        cli(
            "/usr/bin/plutil",
            &[
                "-replace".as_ref(),
                "CFBundleShortVersionString".as_ref(),
                "-string".as_ref(),
                "0.10.22".as_ref(),
                plist.as_os_str(),
            ],
        );
        cli(
            "/usr/bin/codesign",
            &[
                "--force".as_ref(),
                "--deep".as_ref(),
                "--sign".as_ref(),
                "-".as_ref(),
                candidate.as_os_str(),
            ],
        );
        let candidate_digest = digest_file(&candidate_exe);
        assert_ne!(legacy_digest, candidate_digest);
        let mut arch = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        arch.append_dir_all("Nemorous.app", &candidate).unwrap();
        let asset = arch.into_inner().unwrap().finish().unwrap();
        let ephemeral = SigningKey::from_bytes(&[17; 32]);
        let digest = Sha256::digest(&asset);
        let sig = STANDARD.encode(ephemeral.sign(&digest).to_bytes());
        let manifest = update_manifest::parse(
            &serde_json::to_vec(&serde_json::json!({
                "version":"v0.10.22", "url":"https://example.invalid/update.tar.gz",
                "notes":"Isolated signed smoke fixture", "size":asset.len(), "signature":sig,
            }))
            .unwrap(),
        )
        .unwrap();
        let key = ephemeral.verifying_key();
        let cancel = AtomicBool::new(false);
        assert!(manifest.newer_than("0.10.21", None, false).unwrap());
        assert!(
            !manifest
                .newer_than("0.10.21", Some("0.10.22"), false)
                .unwrap()
        );
        let mut corrupt = asset.clone();
        corrupt[30] ^= 1;
        assert!(
            stage_with(&current, &manifest, &cancel, |_| {
                manifest.verify_archive(&corrupt[..], &key)?;
                Ok(&corrupt[..])
            })
            .is_err()
        );
        assert_eq!(digest_file(&current_exe), legacy_digest);
        let staged = stage_with(&current, &manifest, &cancel, |_| {
            manifest.verify_archive(&asset[..], &key)?;
            Ok(&asset[..])
        })
        .unwrap();
        assert!(stage_with(&current, &manifest, &cancel, |_| Ok(&asset[..])).is_err());
        let backup = staged.apply().unwrap();
        assert_eq!(digest_file(&current_exe), candidate_digest);
        assert_eq!(
            digest_file(&backup.join("Contents/MacOS/Nemorous")),
            legacy_digest
        );
        let rolled = root.0.join("rust-tested-rollback.app");
        promote(&current, &backup, &rolled, || {
            validate(&current, &semver::Version::parse("0.10.21").unwrap())
        })
        .unwrap();
        assert_eq!(digest_file(&current_exe), legacy_digest);
        assert_eq!(
            digest_file(&rolled.join("Contents/MacOS/Nemorous")),
            candidate_digest
        );
        println!(
            "SIGNED_MACOS_UPDATE_ROLLBACK PASS size={} mounted_version=v0.10.22 restored=Go/0.10.21",
            asset.len()
        );
    }

    #[test]
    fn updating_mirror_preserves_a_direct_fallback() {
        let base = "https://github.com/example/asset";
        assert_eq!(
            urls("https://mirror.example/", base).unwrap(),
            [
                "https://mirror.example/https://github.com/example/asset",
                base,
            ]
        );
        assert!(urls("https://secret@mirror.example", base).is_err());
    }
}
