//! Curated open-weight local models and Ollama process helpers.
//!
//! These are the same entries shown in the desktop model picker. A model is
//! "ready" when Ollama reports it installed, otherwise it is installable.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// One curated model that can be installed through Ollama.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CuratedLocalModel {
    /// Ollama tag used by `ollama pull`.
    pub tag: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Upstream license.
    pub license: &'static str,
}

/// Curated open-weight roster. Licenses were checked upstream by the project.
pub const CURATED: &[CuratedLocalModel] = &[
    CuratedLocalModel {
        tag: "deepseek-v4.1:flash",
        name: "DeepSeek V4.1 Flash",
        license: "MIT",
    },
    CuratedLocalModel {
        tag: "reflection-beam",
        name: "Reflection Beam",
        license: "Apache 2.0",
    },
    CuratedLocalModel {
        tag: "glm-5.3",
        name: "GLM-5.3",
        license: "GLM-5.3 (MIT-like, $10B MaaS review clause)",
    },
    CuratedLocalModel {
        tag: "glm-5.3:flash",
        name: "GLM-5.3 Flash",
        license: "MIT",
    },
    CuratedLocalModel {
        tag: "mistral-small-4",
        name: "Mistral Small 4",
        license: "Apache 2.0",
    },
];

/// Look up a curated entry by exact Ollama tag.
pub fn curated(tag: &str) -> Option<&'static CuratedLocalModel> {
    let tag = tag.trim();
    CURATED.iter().find(|model| model.tag == tag)
}

/// Whether a tag belongs to the curated roster.
pub fn is_curated(tag: &str) -> bool {
    curated(tag).is_some()
}

/// Build an `ollama` command without flashing a console window on Windows.
fn ollama_command() -> Command {
    let program = relay_core::platform::resolve_program("ollama")
        .unwrap_or_else(|| std::path::PathBuf::from("ollama"));
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Model names installed in the local Ollama daemon.
pub fn installed_ollama_models() -> Vec<String> {
    let out = ollama_command().arg("list").output();
    let Ok(out) = out else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// Whether a tag is installed, accepting Ollama's implicit `:latest` suffix.
pub fn installed(tag: &str) -> bool {
    let tag = tag.trim();
    installed_ollama_models().iter().any(|name| {
        name == tag
            || name
                .strip_prefix(tag)
                .is_some_and(|rest| rest.starts_with(':'))
            || (!tag.contains(':') && name == &format!("{tag}:latest"))
    })
}

/// Run `ollama pull` with live progress on the user's terminal.
pub fn pull(tag: &str) -> Result<(), String> {
    let status = ollama_command()
        .arg("pull")
        .arg(tag)
        .status()
        .map_err(|error| format!("could not run `ollama pull {tag}`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`ollama pull {tag}` failed with {status}"))
    }
}

/// Whether an Ollama-compatible HTTP endpoint is responding.
pub async fn ollama_reachable(base_url: &str) -> bool {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    client
        .get(url)
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

/// Start `ollama serve` when the configured endpoint is not answering.
///
/// Returns the child process when Relay started the daemon, so the caller can
/// stop it on exit. A daemon that was already running is left untouched.
pub async fn ensure_ollama_running(base_url: &str) -> Result<Option<Child>, String> {
    if ollama_reachable(base_url).await {
        return Ok(None);
    }
    let mut child = match ollama_command()
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Err(format!(
                "Ollama is not running and could not be started ({error}). Install Ollama, then run `ollama serve`."
            ));
        }
    };
    for _ in 0..50 {
        if ollama_reachable(base_url).await {
            return Ok(Some(child));
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "`ollama serve` exited with {status}. Start it manually and try again."
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(format!(
        "Ollama did not become ready at {base_url}. Start it manually and try again."
    ))
}

/// List models through the Ollama HTTP API instead of the CLI.
pub async fn list_models_api(base_url: &str) -> Result<Vec<String>, String> {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("could not reach Ollama: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Ollama returned {}", response.status()));
    }
    let json: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
    Ok(json
        .get("models")
        .and_then(|models| models.as_array())
        .map(|models| {
            models
                .iter()
                .filter_map(|model| model.get("name").and_then(|name| name.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default())
}
