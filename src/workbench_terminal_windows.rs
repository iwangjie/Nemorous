use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Result;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub struct Terminal {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    input: Option<mpsc::SyncSender<Vec<u8>>>,
    output: Arc<Mutex<VecDeque<u8>>>,
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl Terminal {
    pub fn spawn(cwd: &Path, shell: &Path, rows: u16, columns: u16) -> Result<Self> {
        let pair = native_pty_system().openpty(size(rows, columns))?;
        let mut command = CommandBuilder::new(shell.as_os_str());
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        let mut writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);
        let (send, recv) = mpsc::sync_channel::<Vec<u8>>(32);
        let output = Arc::new(Mutex::new(VecDeque::new()));
        let buffer = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut chunk = [0; 8192];
            while let Ok(count) = reader.read(&mut chunk) {
                if count == 0 {
                    break;
                }
                let mut bytes = buffer.lock().unwrap_or_else(|err| err.into_inner());
                let excess = (bytes.len() + count).saturating_sub(1024 * 1024);
                bytes.drain(..excess);
                bytes.extend(&chunk[..count]);
            }
        });
        std::thread::spawn(move || {
            while let Ok(bytes) = recv.recv() {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            master: pair.master,
            child,
            input: Some(send),
            output,
        })
    }

    pub fn read_available(&mut self) -> Vec<u8> {
        self.output
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .drain(..)
            .collect()
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Some(input) = &self.input {
            let _ = input.try_send(bytes.to_vec());
        }
    }

    pub fn resize(&self, rows: u16, columns: u16) {
        let _ = self.master.resize(size(rows, columns));
    }

    pub fn exit_code(&mut self) -> Result<Option<u32>> {
        Ok(self.child.try_wait()?.map(|status| status.exit_code()))
    }

    pub fn stop(&mut self) {
        if self.input.take().is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop();
    }
}
