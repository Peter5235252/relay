//! Bundled local models: auto-download and run Nemotron 3.5 Bolt E4B variants
//! so Relay works immediately without user configuration.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundledVariant {
    Standard,
    Uncensored,
}

impl BundledVariant {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Standard => "Nemotron-3.5-Bolt-E4B-Q4_K_M",
            Self::Uncensored => "Nemotron-3.5-Bolt-E4B-Uncensored-AD-Q4_K_M",
        }
    }
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Standard => "Nemotron 3.5 Bolt E4B (local, 4GB)",
            Self::Uncensored => "Nemotron 3.5 Bolt E4B Uncensored (local, ~5GB)",
        }
    }
    pub const fn tagline(self) -> &'static str {
        match self {
            Self::Standard => {
                "Bundled Nemotron 3.5 Bolt E4B · downloads & runs automatically (4-6GB VRAM)."
            }
            Self::Uncensored => {
                "Bundled uncensored/abliterated Nemotron 3.5 Bolt E4B (AtomicChat) · creative writing, fewer refusals (~5GB)."
            }
        }
    }
    pub const fn gguf_url(self) -> &'static str {
        match self {
            Self::Standard => {
                "https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Bolt-E4B-4GB/resolve/main/Nemotron-3.5-Bolt-E4B-Q4_K_M.gguf"
            }
            Self::Uncensored => {
                "https://huggingface.co/AtomicChat/NVIDIA-Nemotron-3.5-Bolt-E4B-Uncensored-GGUF/resolve/main/NVIDIA-Nemotron-3.5-Bolt-E4B-Uncensored-AD-Q4_K_M/NVIDIA-Nemotron-3.5-Bolt-E4B-Uncensored-AD-Q4_K_M-00001-of-00001.gguf"
            }
        }
    }
    pub const fn gguf_name(self) -> &'static str {
        match self {
            Self::Standard => "Nemotron-3.5-Bolt-E4B-Q4_K_M.gguf",
            Self::Uncensored => {
                "NVIDIA-Nemotron-3.5-Bolt-E4B-Uncensored-AD-Q4_K_M-00001-of-00001.gguf"
            }
        }
    }
    pub const fn download_label(self) -> &'static str {
        match self {
            Self::Standard => "Nemotron 3.5 Bolt (4GB)",
            Self::Uncensored => "Nemotron 3.5 Bolt Uncensored (~5GB)",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim();
        if id == Self::Standard.id()
            || id.eq_ignore_ascii_case("standard")
            || id.eq_ignore_ascii_case("base")
        {
            Some(Self::Standard)
        } else if id == Self::Uncensored.id()
            || id.eq_ignore_ascii_case("uncensored")
            || id.eq_ignore_ascii_case("abliterated")
        {
            Some(Self::Uncensored)
        } else {
            None
        }
    }
    pub fn from_model_slug(slug: &str) -> Option<Self> {
        let id = slug.strip_prefix("local/").unwrap_or(slug);
        Self::from_id(id)
    }
    pub const fn all() -> [Self; 2] {
        [Self::Standard, Self::Uncensored]
    }
}

pub const DEFAULT_VARIANT: BundledVariant = BundledVariant::Standard;
pub const PORT: u16 = 11435;

const LLAMA_ARCHIVE: &str = "llama-b12383-bin-ubuntu-vulkan-x64.tar.gz";
const LLAMA_URL: &str = "https://github.com/ggml-org/llama.cpp/releases/download/b12383/llama-b12383-bin-ubuntu-vulkan-x64.tar.gz";

#[derive(Clone, Debug, Default)]
pub struct BundledStatus {
    pub ready: bool,
    pub endpoint: Option<String>,
    pub active_variant: Option<String>,
    pub message: String,
    pub error: Option<String>,
    /// Download progress 0-100 when a download is in progress.
    pub progress: Option<u32>,
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

fn model_path(dir: &Path, variant: BundledVariant) -> PathBuf {
    dir.join(variant.gguf_name())
}

/// On-disk GGUF path for a bundled variant, when a data directory is available.
pub fn model_path_for(variant: BundledVariant) -> Option<PathBuf> {
    base_dir().map(|dir| model_path(&dir, variant))
}

/// Whether the GGUF for a variant has already been downloaded.
pub fn downloaded(variant: BundledVariant) -> bool {
    model_path_for(variant).is_some_and(|path| path.exists())
}

/// Path to a usable `llama-server` binary, if one is installed.
pub fn llama_server_path() -> Option<PathBuf> {
    base_dir().and_then(|dir| find_llama_server(&dir))
}

/// Whether the bundled server is already answering on its fixed port.
pub async fn probe_running() -> bool {
    let client = reqwest::Client::new();
    client
        .get(format!("http://127.0.0.1:{PORT}/v1/models"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

/// OpenAI-compatible endpoint used by the bundled server.
pub fn endpoint() -> String {
    format!("http://127.0.0.1:{PORT}/v1")
}

fn llama_dir(dir: &Path) -> PathBuf {
    dir.join("llama")
}

fn find_llama_server(dir: &Path) -> Option<PathBuf> {
    let candidates = [
        llama_dir(dir).join("llama-server"),
        llama_dir(dir).join("bin").join("llama-server"),
    ];
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
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
        .timeout(Duration::from_secs(7200))
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
        if let Ok(mut s) = state.lock() {
            if total > 0 {
                let pct = (downloaded * 100 / total) as u32;
                s.status.progress = Some(pct);
                s.status.message = format!("Downloading {label}… {pct}%");
            } else {
                s.status.progress = None;
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
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (dir, state);
        return Err("Bundled local models are currently supported on Linux only".to_owned());
    }
    #[cfg(target_os = "linux")]
    {
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
            s.status.progress = None;
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
}

async fn wait_for_server(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/v1/models");
    let client = reqwest::Client::new();
    for _ in 0..180 {
        if let Ok(resp) = client
            .get(&url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            && resp.status().is_success()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

pub fn stop_child(state: &Arc<Mutex<BundledState>>) {
    if let Ok(mut s) = state.lock() {
        if let Some(mut c) = s.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        s.status.ready = false;
        s.status.endpoint = None;
        s.status.active_variant = None;
    }
}

pub async fn ensure_ready(
    state: Arc<Mutex<BundledState>>,
    variant: BundledVariant,
    context_size: u32,
) {
    // If already ready with requested variant, nothing to do.
    if let Ok(s) = state.lock()
        && s.status.ready
        && s.status.active_variant.as_deref() == Some(variant.id())
    {
        return;
    }
    // Stop existing server if switching or not ready.
    stop_child(&state);

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
    let mp = model_path(&dir, variant);
    if let Ok(mut s) = state.lock() {
        s.status.message = format!("Preparing {}…", variant.display_name());
        s.status.error = None;
        s.status.progress = None;
    }

    if !mp.exists()
        && let Err(e) =
            download_file(variant.gguf_url(), &mp, &state, variant.download_label()).await
    {
        if let Ok(mut s) = state.lock() {
            s.status.error = Some(e);
            s.status.message = "Bundled model download failed".into();
        }
        return;
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

    if let Ok(mut s) = state.lock() {
        s.status.message = format!("Starting {}…", variant.display_name());
    }

    let mut cmd = Command::new(&server);
    cmd.arg("-m")
        .arg(&mp)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(PORT.to_string())
        .arg("-c")
        .arg(context_size.to_string())
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

    if wait_for_server(PORT).await {
        let endpoint = format!("http://127.0.0.1:{PORT}/v1");
        if let Ok(mut s) = state.lock() {
            s.status.ready = true;
            s.status.endpoint = Some(endpoint);
            s.status.active_variant = Some(variant.id().to_owned());
            s.status.message = format!("{} ready", variant.display_name());
            s.status.error = None;
            s.status.progress = None;
        }
    } else {
        if let Ok(mut s) = state.lock() {
            s.status.error = Some("Server did not become ready".into());
            s.status.message = "Bundled model failed to start".into();
        }
    }
}

// Back-compat convenience for callers expecting old API.
pub const MODEL_ID: &str = "Nemotron-3.5-Bolt-E4B-Q4_K_M";
pub const DISPLAY_NAME: &str = "Nemotron 3.5 Bolt E4B (local, 4GB)";

pub async fn ensure_ready_default(state: Arc<Mutex<BundledState>>, context_size: u32) {
    ensure_ready(state, DEFAULT_VARIANT, context_size).await;
}

pub fn stop(state: &Arc<Mutex<BundledState>>) {
    stop_child(state);
}
