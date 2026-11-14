//! Bundled local model: auto-download and run Nemotron 3.5 Bolt E4B (4GB)
//! so Relay works immediately without user configuration.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

pub const MODEL_ID: &str = "Nemotron-3.5-Bolt-E4B-Q4_K_M";
pub const DISPLAY_NAME: &str = "Nemotron 3.5 Bolt E4B (local, 4GB)";

const GGUF_URL: &str = "https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Bolt-E4B-4GB/resolve/main/Nemotron-3.5-Bolt-E4B-Q4_K_M.gguf";
const GGUF_NAME: &str = "Nemotron-3.5-Bolt-E4B-Q4_K_M.gguf";

const LLAMA_TAG: &str = "b12383";
const LLAMA_ARCHIVE: &str = "llama-b12383-bin-ubuntu-vulkan-x64.tar.gz";
const LLAMA_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b12383/llama-b12383-bin-ubuntu-vulkan-x64.tar.gz";

#[derive(Clone, Debug, Default)]
pub struct BundledStatus {
    pub ready: bool,
    pub endpoint: Option<String>,
    pub message: String,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct BundledState {
    pub status: BundledStatus,
    child: Option<Child>,
}

impl BundledState {
    pub fn status(&self) -> BundledStatus {
        self.status.clone()
    }
}

pub fn base_dir() -> Option<PathBuf> {
    relay_store::default_data_dir()
        .ok()
        .map(|d| d.join("bundled").join("nemotron35-bolt"))
}

fn model_path(dir: &Path) -> PathBuf {
    dir.join(GGUF_NAME)
}

fn llama_dir(dir: &Path) -> PathBuf {
    dir.join("llama")
}

fn find_llama_server(dir: &Path) -> Option<PathBuf> {
    // search extracted tree for llama-server
    let candidates = [
        llama_dir(dir).join("llama-server"),
        llama_dir(dir).join("bin").join("llama-server"),
    ];
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    // walk one level deep
    if let Ok(rd) = std::fs::read_dir(llama_dir(dir)) {
        for e in rd.flatten() {
            let p = e.path();
            let c1 = p.join("llama-server");
            let c2 = p.join("bin").join("llama-server");
            if c1.exists() {
                return Some(c1);
            }
            if c2.exists() {
                return Some(c2);
            }
        }
    }
    // fallback to PATH
    if Command::new("llama-server")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
    {
        return Some(PathBuf::from("llama-server"));
    }
    None
}

async fn download_file(
    url: &str,
    dest: &Path,
    state: &Arc<Mutex<BundledState>>,
    label: &str,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("part");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("download failed: {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut stream = resp.bytes_stream();
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        if total > 0 {
            let pct = (downloaded * 100 / total) as u32;
            if let Ok(mut s) = state.lock() {
                s.status.message = format!("Downloading {label}… {pct}%");
            }
        } else {
            if let Ok(mut s) = state.lock() {
                s.status.message = format!("Downloading {label}… {} MB", downloaded / 1_000_000);
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

async fn ensure_llama_server(
    dir: &Path,
    state: &Arc<Mutex<BundledState>>,
) -> Result<PathBuf, String> {
    if let Some(p) = find_llama_server(dir) {
        return Ok(p);
    }
    let ldir = llama_dir(dir);
    std::fs::create_dir_all(&ldir).map_err(|e| e.to_string())?;
    let archive_path = dir.join(LLAMA_ARCHIVE);
    if !archive_path.exists() {
        download_file(LLAMA_URL, &archive_path, state, "llama-server").await?;
    }
    if let Ok(mut s) = state.lock() {
        s.status.message = "Extracting llama-server…".to_owned();
    }
    let archive_path2 = archive_path.clone();
    let ldir2 = ldir.clone();
    tokio::task::spawn_blocking(move || {
        let f = std::fs::File::open(&archive_path2).map_err(|e| e.to_string())?;
        let gz = flate2::read::GzDecoder::new(f);
        let mut ar = tar::Archive::new(gz);
        ar.unpack(&ldir2).map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    // make executable
    if let Some(p) = find_llama_server(dir) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        return Ok(p);
    }
    Err("llama-server not found after extraction".to_owned())
}

async fn wait_for_server(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/v1/models");
    let client = reqwest::Client::new();
    for _ in 0..120 {
        if let Ok(resp) = client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            if resp.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

pub async fn ensure_ready(state: Arc<Mutex<BundledState>>) {
    let Some(dir) = base_dir() else {
        if let Ok(mut s) = state.lock() {
            s.status.error = Some("No data dir".into());
        }
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        if let Ok(mut s) = state.lock() {
            s.status.error = Some(e.to_string());
        }
        return;
    }
    let mp = model_path(&dir);
    if let Ok(mut s) = state.lock() {
        s.status.message = "Preparing bundled local model…".into();
    }

    if !mp.exists() {
        if let Err(e) = download_file(GGUF_URL, &mp, &state, "Nemotron 3.5 Bolt (4GB)").await {
            if let Ok(mut s) = state.lock() {
                s.status.error = Some(e);
                s.status.message = "Bundled model download failed".into();
            }
            return;
        }
    }
    let server = match ensure_llama_server(&dir, &state).await {
        Ok(p) => p,
        Err(e) => {
            if let Ok(mut s) = state.lock() {
                s.status.error = Some(e);
                s.status.message = "Could not prepare llama-server".into();
            }
            return;
        }
    };

    let port: u16 = 11435;
    if let Ok(mut s) = state.lock() {
        s.status.message = "Starting local model server…".into();
    }

    // stop previous if any
    if let Ok(mut s) = state.lock() {
        if let Some(mut c) = s.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    let mut cmd = Command::new(&server);
    cmd.arg("-m")
        .arg(&mp)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("-c")
        .arg("8192")
        .arg("-ngl")
        .arg("99")
        .arg("--jinja")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            if let Ok(mut s) = state.lock() {
                s.child = Some(child);
            }
        }
        Err(e) => {
            if let Ok(mut s) = state.lock() {
                s.status.error = Some(e.to_string());
                s.status.message = "Failed to start llama-server".into();
            }
            return;
        }
    }

    if wait_for_server(port).await {
        let endpoint = format!("http://127.0.0.1:{port}/v1");
        if let Ok(mut s) = state.lock() {
            s.status.ready = true;
            s.status.endpoint = Some(endpoint);
            s.status.message = "Bundled local model ready".into();
            s.status.error = None;
        }
    } else {
        if let Ok(mut s) = state.lock() {
            s.status.error = Some("Server did not become ready".into());
            s.status.message = "Bundled model failed to start".into();
        }
    }
}

pub fn stop(state: &Arc<Mutex<BundledState>>) {
    if let Ok(mut s) = state.lock() {
        if let Some(mut c) = s.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        s.status.ready = false;
    }
}
