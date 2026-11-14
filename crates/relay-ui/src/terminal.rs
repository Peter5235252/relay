#![allow(dead_code)]

use std::process::Command;

use relay_core::AgentMode;

pub fn run_command(command: &str, mode: AgentMode) -> String {
    if mode == AgentMode::Plan {
        // Plan mode: only allow read-only-ish commands? We still run but warn.
    }
    let output = Command::new("sh").arg("-c").arg(command).output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            format!(
                "exit={}\nstdout:\n{}\nstderr:\n{}",
                out.status, stdout, stderr
            )
        }
        Err(e) => format!("failed to spawn: {e}"),
    }
}
