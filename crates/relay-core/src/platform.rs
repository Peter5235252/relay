//! Platform shell defaults shared by the desktop app, the CLI, and model tools.
//!
//! Relay uses the user's native shell everywhere: PowerShell on Windows, zsh
//! on macOS, and bash (or the login shell) on Linux. The whole product must
//! agree on this so a command suggested by a model behaves the same as one
//! typed into the integrated terminal.

use std::path::PathBuf;
use std::process::Command;

/// Shell families Relay knows how to invoke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellFlavor {
    /// POSIX shell driven with `-c` / `-i` (bash, zsh, sh, fish).
    Posix,
    /// PowerShell (`pwsh` or Windows PowerShell) with `-Command`.
    PowerShell,
    /// Legacy Windows command interpreter with `/C`.
    Cmd,
}

impl ShellFlavor {
    /// Arguments used to run a single command string.
    pub fn command_args(self) -> &'static [&'static str] {
        match self {
            Self::Posix => &["-c"],
            Self::PowerShell => &["-NoProfile", "-NonInteractive", "-Command"],
            Self::Cmd => &["/C"],
        }
    }

    /// Arguments used to start an interactive shell.
    pub fn interactive_args(self) -> &'static [&'static str] {
        match self {
            Self::Posix => &["-i"],
            Self::PowerShell => &["-NoLogo"],
            Self::Cmd => &[],
        }
    }
}

/// Host platform name used in prompts and diagnostics.
pub const fn platform_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macOS"
    }
    #[cfg(target_os = "linux")]
    {
        "Linux"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "an unsupported platform"
    }
}

/// Guidance appended to the system prompt so models use the right shell.
pub fn shell_instructions() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Host platform: Windows. Shell commands run in PowerShell (pwsh when available, otherwise Windows PowerShell). Use PowerShell syntax and cmdlets: $env:NAME for environment variables, Get-ChildItem/Get-Content/Select-String, and PowerShell quoting rules. Do not assume bash, zsh, or Unix utilities are present."
    }
    #[cfg(target_os = "macos")]
    {
        "Host platform: macOS. Shell commands run in zsh. Use macOS/zsh syntax and paths (for example ~/Desktop, /Applications, /opt/homebrew/bin). Do not assume Linux-only tools are present."
    }
    #[cfg(target_os = "linux")]
    {
        "Host platform: Linux. Shell commands run in bash (or the user's login shell). Use POSIX/bash syntax and standard Linux paths."
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "Host platform: unknown. Prefer portable shell syntax."
    }
}

/// Classify a shell executable by name.
pub fn shell_flavor(shell: &str) -> ShellFlavor {
    let name = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase();
    if name == "pwsh" || name == "pwsh.exe" || name.starts_with("powershell") {
        ShellFlavor::PowerShell
    } else if name == "cmd" || name == "cmd.exe" {
        ShellFlavor::Cmd
    } else {
        ShellFlavor::Posix
    }
}

/// Resolve a program name through `PATH` (and `PATHEXT` on Windows).
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    let lookup = std::path::Path::new(name);
    if lookup.components().count() > 1 {
        return lookup.is_file().then(|| lookup.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(|ext| ext.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for ext in &extensions {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The native interactive shell for this platform, as a short display name.
pub const fn default_shell_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "PowerShell"
    }
    #[cfg(target_os = "macos")]
    {
        "zsh"
    }
    #[cfg(target_os = "linux")]
    {
        "bash"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "shell"
    }
}

/// The native interactive shell for this platform.
///
/// `RELAY_SHELL` overrides the default, which makes it possible to point the
/// app at a specific PowerShell or zsh installation.
pub fn default_shell() -> String {
    if let Ok(custom) = std::env::var("RELAY_SHELL")
        && !custom.trim().is_empty()
    {
        return custom;
    }

    #[cfg(target_os = "windows")]
    {
        for candidate in ["pwsh", "powershell.exe", "powershell", "cmd.exe"] {
            if let Some(path) = resolve_program(candidate) {
                return path.display().to_string();
            }
        }
        std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".to_owned())
    }
    #[cfg(target_os = "macos")]
    {
        if std::path::Path::new("/bin/zsh").is_file() {
            return "/bin/zsh".to_owned();
        }
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/bin/bash").is_file() {
            return "/bin/bash".to_owned();
        }
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "sh".to_owned())
    }
}

/// Build a command that runs `command` in `shell` with native quoting rules.
pub fn shell_command(shell: &str, command: &str) -> Command {
    let mut cmd = Command::new(shell);
    cmd.args(shell_flavor(shell).command_args());
    cmd.arg(command);
    cmd
}

/// The user's home directory, honoring the platform's environment variable.
pub fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE")
            && !profile.is_empty()
        {
            return Some(PathBuf::from(profile));
        }
        let drive = std::env::var_os("HOMEDRIVE")?;
        let path = std::env::var_os("HOMEPATH")?;
        let mut home = PathBuf::from(drive);
        home.push(path);
        return Some(home);
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_shell_flavors() {
        assert_eq!(shell_flavor("/bin/zsh"), ShellFlavor::Posix);
        assert_eq!(
            shell_flavor("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            ShellFlavor::PowerShell
        );
        assert_eq!(shell_flavor("powershell.exe"), ShellFlavor::PowerShell);
        assert_eq!(shell_flavor("cmd.exe"), ShellFlavor::Cmd);
        assert_eq!(shell_flavor("bash"), ShellFlavor::Posix);
    }

    #[test]
    fn command_arguments_match_shell_family() {
        assert_eq!(ShellFlavor::Posix.command_args(), &["-c"]);
        assert_eq!(ShellFlavor::PowerShell.command_args()[2], "-Command");
        assert_eq!(ShellFlavor::Cmd.command_args(), &["/C"]);
    }

    #[test]
    fn default_shell_is_platform_native() {
        let shell = default_shell();
        assert!(!shell.trim().is_empty());
        #[cfg(target_os = "windows")]
        assert!(matches!(
            shell_flavor(&shell),
            ShellFlavor::PowerShell | ShellFlavor::Cmd
        ));
        #[cfg(target_os = "macos")]
        assert_eq!(shell, "/bin/zsh");
        #[cfg(target_os = "linux")]
        assert!(shell.ends_with("bash") || shell.ends_with("sh"));
    }
}
