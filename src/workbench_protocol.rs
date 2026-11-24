use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::workbench_process::{ChildGroup, ProcessDiagnostics, ProcessReport};

const FRAME_LIMIT: usize = 16 * 1024 * 1024;
const HEADER_LIMIT: usize = 8192;

enum Transport {
    Process(ChildGroup, Option<ProcessDiagnostics>),
    Tcp(TcpStream),
}

pub(crate) struct Connection {
    write: SyncSender<Vec<u8>>,
    read: Receiver<Result<Value>>,
    transport: Transport,
}

impl Drop for Transport {
    fn drop(&mut self) {
        match self {
            Self::Tcp(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            Self::Process(child, _) => child.stop(),
        }
    }
}

impl Connection {
    pub(crate) fn stdio(command: &mut Command) -> Result<Self> {
        Self::from_stdio(command, false)
    }

    pub(crate) fn stdio_with_diagnostics(command: &mut Command) -> Result<Self> {
        Self::from_stdio(command, true)
    }

    fn from_stdio(command: &mut Command, diagnostics: bool) -> Result<Self> {
        let mut process =
            ChildGroup::spawn(command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(
                if diagnostics {
                    Stdio::piped()
                } else {
                    Stdio::null()
                },
            ))?;
        let diagnostics = diagnostics
            .then(|| ProcessDiagnostics::capture(&mut process))
            .transpose()?;
        let send = process.child.stdin.take().context("无协议输入")?;
        let receive = process.child.stdout.take().context("无协议输出")?;
        Self::new(send, receive, Transport::Process(process, diagnostics))
    }

    pub(crate) fn process_report(&mut self) -> Option<ProcessReport> {
        match &mut self.transport {
            Transport::Process(child, Some(diagnostics)) => Some(diagnostics.report(child)),
            _ => None,
        }
    }

    pub(crate) fn tcp(address: &str) -> Result<Self> {
        let addresses = address.to_socket_addrs()?.collect::<Vec<_>>();
        for endpoint in addresses {
            if let Ok(stream) = TcpStream::connect_timeout(&endpoint, Duration::from_millis(700)) {
                stream.set_nodelay(true)?;
                return Self::new(
                    stream.try_clone()?,
                    stream.try_clone()?,
                    Transport::Tcp(stream),
                );
            }
        }
        bail!("无法连接到调试适配器 {address}")
    }

    fn new(
        send: impl Write + Send + 'static,
        receive: impl Read + Send + 'static,
        transport: Transport,
    ) -> Result<Self> {
        let (out, to_write) = mpsc::sync_channel::<Vec<u8>>(8);
        let (from_read, input) = mpsc::sync_channel::<Result<Value>>(8);
        let write_errors = from_read.clone();
        std::thread::Builder::new()
            .name("nemorous-protocol-out".into())
            .spawn(move || {
                let mut send = send;
                while let Ok(payload) = to_write.recv() {
                    let result = (|| -> std::io::Result<()> {
                        write!(send, "Content-Length: {}\r\n\r\n", payload.len())?;
                        send.write_all(&payload)?;
                        send.flush()
                    })();
                    if let Err(error) = result {
                        let _ = write_errors.try_send(Err(error.into()));
                        return;
                    }
                }
            })?;
        std::thread::Builder::new()
            .name("nemorous-protocol-in".into())
            .spawn(move || {
                let mut receive = BufReader::new(receive);
                loop {
                    let next = read_frame(&mut receive);
                    let failed = next.is_err();
                    if from_read.send(next).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            write: out,
            read: input,
            transport,
        })
    }

    pub(crate) fn send(&self, value: Value) -> Result<()> {
        let frame = serde_json::to_vec(&value)?;
        ensure!(frame.len() <= FRAME_LIMIT, "协议消息超过大小限制");
        self.write
            .try_send(frame)
            .map_err(|_| anyhow::anyhow!("协议发送队列已满或连接已关闭"))
    }

    pub(crate) fn poll(&self) -> Result<Option<Value>> {
        match self.read.try_recv() {
            Ok(value) => value.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => bail!("协议连接已关闭"),
        }
    }
}

fn read_frame(reader: &mut impl BufRead) -> Result<Value> {
    let mut length = None;
    let mut remaining = HEADER_LIMIT;
    loop {
        let mut line = String::new();
        let bytes = (&mut *reader).take(remaining as u64).read_line(&mut line)?;
        ensure!(bytes > 0, "协议连接已关闭");
        ensure!(line.ends_with('\n'), "协议消息头不完整或过长");
        remaining -= bytes;
        if line == "\r\n" || line == "\n" {
            break;
        }
        let (key, value) = line.split_once(':').context("无效的协议消息头")?;
        if key.eq_ignore_ascii_case("Content-Length") {
            ensure!(length.is_none(), "协议包含重复的长度");
            let value = value.trim().parse::<usize>()?;
            ensure!(value > 0 && value <= FRAME_LIMIT, "协议消息长度无效");
            length = Some(value);
        }
        ensure!(remaining > 0, "协议消息头过长");
    }
    let mut data = vec![0; length.context("协议消息缺少 Content-Length")?];
    reader.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_jsonrpc_and_dap_frames_and_size_limits() {
        let payload = br#"{"type":"event","event":"stopped"}"#;
        let mut framed = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
        framed.extend(payload);
        let twice = [framed.as_slice(), framed.as_slice()].concat();
        let mut reader = BufReader::with_capacity(3, &twice[..]);
        assert_eq!(read_frame(&mut reader).unwrap()["event"], "stopped");
        assert_eq!(read_frame(&mut reader).unwrap()["type"], "event");
        assert!(read_frame(&mut reader).is_err());
        for bad in [
            "Content-Length: 99999999\r\n\r\n",
            "Content-Length: 1\r\nContent-Length: 2\r\n\r\n{}",
            "Content-Length: 5\r\n\r\n{}",
        ] {
            assert!(read_frame(&mut bad.as_bytes()).is_err());
        }
    }
}
