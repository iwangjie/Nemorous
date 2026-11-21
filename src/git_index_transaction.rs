use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail, ensure};

use crate::git::run_git_string;

static NEXT_INDEX: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempIndex {
    directory: PathBuf,
    path: PathBuf,
}

impl TempIndex {
    fn new(parent: &Path) -> Result<Self> {
        loop {
            let id = NEXT_INDEX.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!("nemorous-index-{}-{id}", std::process::id()));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    let path = directory.join("index");
                    return Ok(Self { directory, path });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err).context("无法创建临时 Git 索引目录"),
            }
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

pub(crate) struct IndexTransaction {
    index: PathBuf,
    lock_path: PathBuf,
    lock: Option<File>,
    scratch: TempIndex,
    installed: bool,
}

impl IndexTransaction {
    pub(crate) fn begin(repo: &Path) -> Result<Self> {
        let path = run_git_string(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )?;
        let index = PathBuf::from(path.strip_suffix('\n').unwrap_or(&path));
        ensure!(index.is_absolute(), "Git 未返回绝对索引路径");
        let parent = index.parent().context("Git 索引缺少父目录")?;
        let scratch = TempIndex::new(parent)?;
        let mut lock_name = index.as_os_str().to_os_string();
        lock_name.push(OsStr::new(".lock"));
        let lock_path = PathBuf::from(lock_name);
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .context("无法取得 Git 索引锁，可能有另一项 Git 操作正在运行")?;

        let tx = Self {
            index,
            lock_path,
            lock: Some(lock),
            scratch,
            installed: false,
        };
        let shared = run_git_string(repo, &["rev-parse", "--shared-index-path"])?;
        ensure!(shared.trim().is_empty(), "当前操作不支持拆分索引");
        match fs::symlink_metadata(&tx.index) {
            Ok(meta) => {
                ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "Git 索引必须是普通文件"
                );
                fs::copy(&tx.index, tx.index_path()).context("复制 Git 索引失败")?;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).context("读取 Git 索引失败"),
        }
        Ok(tx)
    }

    pub(crate) fn index_path(&self) -> &Path {
        self.scratch.path()
    }

    pub(crate) fn fresh_index(&self) -> Result<TempIndex> {
        TempIndex::new(self.index.parent().context("Git 索引缺少父目录")?)
    }

    pub(crate) fn install(mut self) -> Result<()> {
        let mut source = File::open(self.index_path()).context("临时 Git 索引未生成")?;
        let Some(mut lock) = self.lock.take() else {
            bail!("Git 索引锁已失效");
        };
        io::copy(&mut source, &mut lock).context("写入 Git 索引锁文件失败")?;
        lock.set_permissions(source.metadata()?.permissions())?;
        lock.sync_all().context("同步 Git 索引失败")?;
        drop(lock);
        fs::rename(&self.lock_path, &self.index).context("安装 Git 索引失败")?;
        self.installed = true;
        Ok(())
    }
}

impl Drop for IndexTransaction {
    fn drop(&mut self) {
        if !self.installed {
            drop(self.lock.take());
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}
