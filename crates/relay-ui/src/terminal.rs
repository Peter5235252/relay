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

/// Interactive shell session connected to the user's shell.
pub struct ShellSession {
    pub output: String,
    /// Whether the shell process has exited.
    pub exited: bool,
    shell: String,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    pub events: Receiver<ShellEvent>,
    _events_tx: Sender<ShellEvent>,
}

fn command_exists(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn default_shell() -> String {
    #[cfg(target_os = "windows")]
    {
        if command_exists("pwsh") {
            "pwsh".to_owned()
        } else {
            "powershell".to_owned()
        }
    }
    #[cfg(target_os = "macos")]
    {
        if std::path::Path::new("/bin/zsh").exists() {
            "/bin/zsh".to_owned()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
        }
    }
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/bin/bash").exists() {
            "/bin/bash".to_owned()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
}

fn sanitize_output(input: &str) -> String {
    // Minimal ANSI/control stripping for readable display.
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\x1b' => {
                i += 1;
                if i >= bytes.len() {
                    break;
                }
                match bytes[i] {
                    b'[' => {
                        // CSI: ESC [ ... <final byte @-~>
                        i += 1;
                        while i < bytes.len() {
                            let b = bytes[i];
                            i += 1;
                            if (0x40..=0x7E).contains(&b) {
                                break;
                            }
                        }
                    }
                    b']' => {
                        // OSC: ESC ] ... BEL or ESC \
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == 0x07 {
                                i += 1;
                                break;
                            }
                            if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    b'P' | b'^' | b'_' => {
                        // DCS/PM/AP: ESC P/^/_ ... ESC \
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                                i += 2;
                                break;
                            }
                            i += 1;
                        }
                    }
                    _ => {
                        // other escape sequence, skip introducer
                        i += 1;
                    }
                }
            }
            b'\r' => {
                // normalize CRLF/CR
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    out.push('\n');
                    i += 2;
                } else {
                    i += 1; // drop lone CR (often used for overwrite)
                }
            }
            b'\t' => {
                out.push_str("    ");
                i += 1;
            }
            b if b < 0x20 && b != b'\n' => {
                // drop other control chars
                i += 1;
            }
            _ => {
                let ch = input[i..].chars().next().unwrap_or('\u{FFFD}');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
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

        let shell = default_shell();
        let mut cmd = CommandBuilder::new(&shell);
        if shell.ends_with("bash") || shell.ends_with("zsh") {
            cmd.arg("-i");
        }
        #[cfg(target_os = "windows")]
        {
            cmd.arg("-NoLogo");
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok();
        if let Some(home) = home {
            cmd.cwd(home);
        } else if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
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
                        let clean = sanitize_output(&chunk);
                        if !clean.is_empty()
                            && tx_reader.send_blocking(ShellEvent::Output(clean)).is_err()
                        {
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
            exited: false,
            shell,
            writer: Some(writer),
            child: Some(child),
            master: Some(pair.master),
            events: rx,
            _events_tx: tx,
        })
    }

    /// Display name of the shell backing this session.
    pub fn shell_name(&self) -> &str {
        std::path::Path::new(&self.shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&self.shell)
    }

    /// Current working directory of the shell, abbreviated with `~`.
    pub fn cwd_label(&self) -> Option<String> {
        if self.exited {
            return None;
        }
        #[cfg(not(unix))]
        {
            return None;
        }
        #[cfg(unix)]
        {
            let pid = self.child.as_ref()?.process_id()?;
            let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let label = match home {
                Some(home) => match path.strip_prefix(&home) {
                    Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
                    Ok(rest) => format!("~/{}", rest.display()),
                    Err(_) => path.display().to_string(),
                },
                None => path.display().to_string(),
            };
            Some(label)
        }
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
            let mut trim_at = excess;
            while trim_at < self.output.len() && !self.output.is_char_boundary(trim_at) {
                trim_at += 1;
            }
            if let Some(rel) = self.output[trim_at..].find('\n') {
                trim_at += rel + 1;
            }
            self.output.drain(..trim_at.min(self.output.len()));
        }
    }

    pub fn clear(&mut self) {
        self.output.clear();
    }

    #[allow(dead_code)]
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
