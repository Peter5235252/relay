//! Terminal output helpers shared by every CLI command.
//!
//! Relay keeps this deliberately small: no terminal UI framework, no ANSI
//! dependency, and no clipboard daemon of its own. When the user asks to copy
//! text, the CLI hands it to whichever native clipboard tool is installed.

use std::io::{IsTerminal as _, Write};
use std::process::{Command, Stdio};

use chrono::{DateTime, Local, Utc};
use serde::Serialize;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

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
    style(text, "2")
}

/// Dim text on stderr, for diagnostics while stdout carries data.
pub fn dim_stderr(text: &str) -> String {
    if color_enabled_stderr() {
        format!("\x1b[2m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Print a section heading.
pub fn heading(title: &str) {
    println!("{}", bold(&cyan(title)));
    println!("{}", dim(&"─".repeat(title.chars().count().max(8))));
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

// MARK: Color and Markdown rendering

/// Whether ANSI color should be emitted on stdout.
pub fn color_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Whether ANSI color should be emitted on stderr.
pub fn color_enabled_stderr() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stderr().is_terminal()
}

/// Wrap text in an ANSI style when colors are enabled.
pub fn style(text: &str, code: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

pub fn bold(text: &str) -> String {
    style(text, "1")
}

pub fn italic(text: &str) -> String {
    style(text, "3")
}

pub fn red(text: &str) -> String {
    style(text, "31")
}

pub fn green(text: &str) -> String {
    style(text, "32")
}

pub fn yellow(text: &str) -> String {
    style(text, "33")
}

pub fn blue(text: &str) -> String {
    style(text, "34")
}

pub fn magenta(text: &str) -> String {
    style(text, "35")
}

pub fn cyan(text: &str) -> String {
    style(text, "36")
}

/// Render Markdown to terminal-friendly text with syntax highlighting.
pub fn render_markdown(input: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = input.lines().collect();
    let mut i = 0;
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code = String::new();

    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("```") {
            if !in_code {
                in_code = true;
                code_lang = rest.trim().to_owned();
                code.clear();
            } else {
                out.push_str(&render_code_block(&code, &code_lang));
                in_code = false;
            }
            i += 1;
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            i += 1;
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            i += 1;
            continue;
        }
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push_str(&dim(&"─".repeat(48)));
            out.push('\n');
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push_str(&bold(&cyan(rest)));
            out.push('\n');
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push_str(&bold(&blue(rest)));
            out.push('\n');
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push_str(&bold(&magenta(rest)));
            out.push('\n');
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("> ") {
            out.push_str(&format!("{} {}\n", dim("│"), dim(rest)));
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix(">") {
            out.push_str(&format!("{}{}\n", dim("│"), dim(rest)));
            i += 1;
            continue;
        }

        let indent = line.len() - line.trim_start().len();
        let indent_str = " ".repeat(indent);
        let body = line.trim_start();
        if let Some(rest) = body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")) {
            out.push_str(&format!("{indent_str}{} {}\n", cyan("•"), format_inline(rest)));
            i += 1;
            continue;
        }
        if let Some(rest) = body.strip_prefix("+ ") {
            out.push_str(&format!("{indent_str}{} {}\n", cyan("•"), format_inline(rest)));
            i += 1;
            continue;
        }
        // Numbered list.
        let mut consumed_number = false;
        let mut number_end = 0;
        for (offset, ch) in body.char_indices() {
            if ch.is_ascii_digit() {
                number_end = offset + ch.len_utf8();
            } else if ch == '.' && number_end > 0 {
                let rest = body[number_end + 1..].trim_start();
                out.push_str(&format!(
                    "{indent_str}{} {}\n",
                    cyan(&body[..number_end + 1]),
                    format_inline(rest)
                ));
                consumed_number = true;
                break;
            } else {
                break;
            }
        }
        if consumed_number {
            i += 1;
            continue;
        }

        out.push_str(&format_inline(line));
        out.push('\n');
        i += 1;
    }
    if in_code {
        out.push_str(&code);
    }
    out
}

fn render_code_block(code: &str, language: &str) -> String {
    if !color_enabled() {
        let mut out = String::new();
        out.push_str(code);
        if !code.ends_with('\n') {
            out.push('\n');
        }
        return out;
    }
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let syntax = if language.is_empty() {
        syntax_set.find_syntax_plain_text()
    } else {
        syntax_set
            .find_syntax_by_token(language)
            .or_else(|| syntax_set.find_syntax_by_extension(language))
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
    };
    let theme = &theme_set.themes["base16-ocean-dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out = String::new();
    out.push_str(&dim("┌─"));
    if !language.is_empty() {
        out.push_str(&format!(" {} ", language));
    }
    out.push_str(&dim(&"─".repeat(36)));
    out.push('\n');
    for line in code.lines() {
        let ranges = highlighter.highlight_line(line, &syntax_set).unwrap_or_default();
        out.push_str("│ ");
        out.push_str(&as_24_bit_terminal_escaped(&ranges, true));
        out.push('\n');
    }
    if code.ends_with('\n') && code.lines().last().map(|line| !line.is_empty()).unwrap_or(false) {
        // keep the final blank line visible
    }
    out.push_str(&dim("└─"));
    out.push('\n');
    out.push_str("\x1b[0m");
    out
}

fn format_inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        // Inline code.
        if chars[i] == '`' {
            if let Some(end) = find_char(&chars, '`', i + 1) {
                let code: String = chars[i + 1..end].iter().collect();
                out.push_str(&cyan(&code));
                i = end + 1;
                continue;
            }
        }
        // Bold with **.
        if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_str(&chars, &['*', '*'], i + 2) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str(&bold(&format_inline(&inner)));
                i = end + 2;
                continue;
            }
        }
        // Bold with __.
        if i + 1 < chars.len() && chars[i] == '_' && chars[i + 1] == '_' {
            if let Some(end) = find_str(&chars, &['_', '_'], i + 2) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str(&bold(&format_inline(&inner)));
                i = end + 2;
                continue;
            }
        }
        // Italic with *.
        if chars[i] == '*' {
            if let Some(end) = find_char(&chars, '*', i + 1) {
                let inner: String = chars[i + 1..end].iter().collect();
                out.push_str(&italic(&format_inline(&inner)));
                i = end + 1;
                continue;
            }
        }
        // Italic with _.
        if chars[i] == '_' {
            if let Some(end) = find_char(&chars, '_', i + 1) {
                let inner: String = chars[i + 1..end].iter().collect();
                out.push_str(&italic(&format_inline(&inner)));
                i = end + 1;
                continue;
            }
        }
        // Link [text](url).
        if chars[i] == '[' {
            if let Some(close) = find_char(&chars, ']', i + 1) {
                if close + 1 < chars.len() && chars[close + 1] == '(' {
                    if let Some(end) = find_char(&chars, ')', close + 2) {
                        let text: String = chars[i + 1..close].iter().collect();
                        let url: String = chars[close + 2..end].iter().collect();
                        out.push_str(&format!(
                            "{} {}",
                            format_inline(&text),
                            dim(&format!("({url})"))
                        ));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn find_char(chars: &[char], needle: char, start: usize) -> Option<usize> {
    chars[start..].iter().position(|&ch| ch == needle).map(|pos| start + pos)
}

fn find_str(chars: &[char], needle: &[char], start: usize) -> Option<usize> {
    if needle.is_empty() || start + needle.len() > chars.len() {
        return None;
    }
    for pos in start..=chars.len() - needle.len() {
        if chars[pos..pos + needle.len()] == *needle {
            return Some(pos);
        }
    }
    None
}
