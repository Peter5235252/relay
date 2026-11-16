//! Terminal output helpers shared by every CLI command.
//!
//! Relay keeps this deliberately small: no terminal UI framework, no ANSI
//! dependency, and no clipboard daemon of its own. When the user asks to copy
//! text, the CLI hands it to whichever native clipboard tool is installed.

use std::io::{IsTerminal as _, Write};
use std::process::{Command, Stdio};

use chrono::{DateTime, Local, Utc};
use serde::Serialize;

/// Print pretty JSON to stdout.
pub fn json<T: Serialize>(value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    println!("{text}");
    Ok(())
}

/// Print a compact single-line JSON value (used inside tables and messages).
pub fn json_compact<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

/// Print an aligned, pipe-free table.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|header| width(header)).collect();
    for row in rows {
        for (index, cell) in row.iter().take(columns).enumerate() {
            widths[index] = widths[index].max(width(cell));
        }
    }
    let line = |cells: &[String]| {
        let mut out = String::new();
        for (index, cell) in cells.iter().take(columns).enumerate() {
            if index > 0 {
                out.push_str("  ");
            }
            out.push_str(cell);
            if index + 1 < columns {
                let pad = widths[index].saturating_sub(width(cell));
                out.extend(std::iter::repeat_n(' ', pad));
            }
        }
        println!("{}", out.trim_end());
    };
    line(
        &headers
            .iter()
            .map(|header| (*header).to_owned())
            .collect::<Vec<_>>(),
    );
    let underline: Vec<String> = widths.iter().map(|size| "─".repeat(*size)).collect();
    line(&underline);
    for row in rows {
        line(row);
    }
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// Shorten text to `max` characters, adding an ellipsis when clipped.
pub fn truncate(text: &str, max: usize) -> String {
    let single_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= max {
        return single_line;
    }
    let mut out: String = single_line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Ask a yes/no question. Destructive operations never proceed silently.
///
/// `assume_yes` is only set by an explicit `--yes` flag. A non-interactive
/// stdin is treated as "no" so a pipeline can never approve a deletion.
pub fn confirm(question: &str, assume_yes: bool) -> bool {
    ask(question, assume_yes, false)
}

/// Like [`confirm`], but the question is printed to stderr.
///
/// Used for tool approvals while stdout carries a machine-readable answer.
pub fn confirm_stderr(question: &str, assume_yes: bool) -> bool {
    ask(question, assume_yes, true)
}

fn ask(question: &str, assume_yes: bool, stderr: bool) -> bool {
    if assume_yes {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        return false;
    }
    if stderr {
        eprint!("{question} [y/N] ");
        let _ = std::io::stderr().flush();
    } else {
        print!("{question} [y/N] ");
        let _ = std::io::stdout().flush();
    }
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Local wall-clock timestamp for message headers.
pub fn local_time(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// Compact identifier used in session lists.
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Format a token count with thousands separators.
pub fn tokens(count: u64) -> String {
    relay_core::format_tokens(count)
}

/// Copy text to the system clipboard through a native helper.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let candidates: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (program, args) in candidates {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => continue,
        };
        let wrote = match child.stdin.take() {
            Some(mut stdin) => stdin.write_all(text.as_bytes()).is_ok(),
            None => false,
        };
        let status = child.wait().map_err(|error| error.to_string())?;
        if wrote && status.success() {
            return Ok(());
        }
    }
    Err("no clipboard tool found; install wl-clipboard, xclip, or xsel".to_owned())
}

/// Dim ANSI text when the destination is a terminal.
pub fn dim(text: &str) -> String {
    if std::io::stderr().is_terminal() {
        format!("\x1b[2m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Print a section heading.
pub fn heading(title: &str) {
    println!("{title}");
    println!("{}", "─".repeat(title.chars().count().max(8)));
}

/// Read one line from stdin without echo when it is a terminal.
///
/// `rpassword` handles the terminal case. Piped input is read normally so
/// scripts can provide keys through stdin without exposing them in `ps`.
pub fn read_secret(prompt: &str) -> Result<String, String> {
    if std::io::stdin().is_terminal() {
        rpassword::prompt_password(prompt).map_err(|error| error.to_string())
    } else {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut value = String::new();
        std::io::stdin()
            .read_line(&mut value)
            .map_err(|error| error.to_string())?;
        Ok(value.trim_end_matches(['\r', '\n']).to_owned())
    }
}

/// Explain why an interactive confirmation was refused.
pub fn cancelled() -> String {
    "cancelled (pass --yes to confirm non-interactively)".to_owned()
}
