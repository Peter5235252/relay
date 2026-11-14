//! Interactive user shell panel backed by a PTY.

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use smol::channel::{Receiver, Sender, unbounded};
use std::io::{Read, Write};

const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Events emitted by the shell session.
#[derive(Debug, Clone)]
pub enum ShellEvent {
    Output(String),
    Exited,
}

/// Interactive shell session connected to the user's default shell.
pub struct ShellSession {
    pub output: String,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    pub events: Receiver<ShellEvent>,
    _events_tx: Sender<ShellEvent>,
}

impl ShellSession {
    pub fn spawn(rows: u16, cols: u16) -> Result<Self, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        // Drop slave in parent after spawn so EOF is delivered on child exit.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        let (tx, rx) = unbounded::<ShellEvent>();
        let tx_reader = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                        if tx_reader.send_blocking(ShellEvent::Output(chunk)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx_reader.send_blocking(ShellEvent::Exited);
        });

        Ok(Self {
            output: String::new(),
            writer: Some(writer),
            child: Some(child),
            master: Some(pair.master),
            events: rx,
            _events_tx: tx,
        })
    }

    pub fn write(&mut self, data: &str) -> Result<(), String> {
        if let Some(w) = self.writer.as_mut() {
            w.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
            w.flush().map_err(|e| e.to_string())?;
            Ok(())
        } else {
            Err("terminal not running".to_owned())
        }
    }

    pub fn push_output(&mut self, chunk: &str) {
        self.output.push_str(chunk);
        if self.output.len() > MAX_OUTPUT_BYTES {
            let excess = self.output.len() - MAX_OUTPUT_BYTES;
            // trim at char boundary
            let mut trim_at = excess;
            while trim_at < self.output.len() && !self.output.is_char_boundary(trim_at) {
                trim_at += 1;
            }
            self.output.drain(..trim_at.min(self.output.len()));
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if let Some(m) = self.master.as_mut() {
            let _ = m.resize(PtySize {
                rows: rows.max(2),
                cols: cols.max(10),
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        self.writer.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.master.take();
    }
}
