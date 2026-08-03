use serde_json::Value;
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

pub const DEFAULT_MODEL_ID: &str = "qwen3:4b";
const DEFAULT_MODEL_DIGEST: &str =
    "359d7dd4bcdab3d86b87d73ac27966f4dbb9f5efdfcc75d34a8764a09474fae7";
const OLLAMA_VERSION: &str = "0.32.5";
const OLLAMA_ORIGIN: &str = "http://127.0.0.1:11434";
const MAX_OLLAMA_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAiRuntimeView {
    pub state: &'static str,
    pub model_id: &'static str,
    pub runtime_version: Option<String>,
    pub model_digest: Option<String>,
    pub message: String,
}

#[derive(Clone)]
pub struct LocalAiRuntime(pub Arc<Mutex<LocalAiRuntimeView>>);

impl Default for LocalAiRuntime {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(LocalAiRuntimeView {
            state: "detecting",
            model_id: DEFAULT_MODEL_ID,
            runtime_version: None,
            model_digest: None,
            message: "Checking the local AI runtime.".into(),
        })))
    }
}

impl LocalAiRuntime {
    fn update(
        &self,
        state: &'static str,
        message: impl Into<String>,
        runtime_version: Option<String>,
        model_digest: Option<String>,
    ) {
        if let Ok(mut view) = self.0.lock() {
            *view = LocalAiRuntimeView {
                state,
                model_id: DEFAULT_MODEL_ID,
                runtime_version,
                model_digest,
                message: message.into(),
            };
        }
    }

    pub fn view(&self) -> Result<LocalAiRuntimeView, String> {
        self.0
            .lock()
            .map(|view| view.clone())
            .map_err(|_| "local AI runtime state lock poisoned".to_string())
    }
}

pub fn schedule(runtime: LocalAiRuntime, disabled: bool) {
    if disabled || bootstrap_disabled(std::env::var("RAMPAGE_DISABLE_LOCAL_AI_BOOTSTRAP").ok()) {
        runtime.update(
            "disabled",
            "Automatic local AI setup is disabled for this diagnostic run.",
            None,
            None,
        );
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(error) = bootstrap(&runtime) {
            runtime.update(
                "failed",
                format!("Local AI setup needs attention: {error}"),
                None,
                None,
            );
        }
    });
}

fn bootstrap_disabled(value: Option<String>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1")
}

fn bootstrap(runtime: &LocalAiRuntime) -> Result<(), String> {
    runtime.update("detecting", "Checking Ollama on loopback.", None, None);
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(7_200))
        .build()
        .map_err(|error| format!("could not create the local AI client: {error}"))?;

    if ollama_version(&client).is_none() {
        runtime.update(
            "installing",
            "Installing the pinned Ollama runtime. This one-time download is about 1.6 GB.",
            None,
            None,
        );
        install_ollama()?;
        ensure_ollama_started()?;
        wait_for_ollama(&client)?;
    }
    let version = ollama_version(&client)
        .ok_or_else(|| "Ollama did not expose its loopback version endpoint".to_string())?;
    if version != OLLAMA_VERSION {
        return Err(format!(
            "Ollama {version} is running, but this Rampage build qualifies {OLLAMA_VERSION}"
        ));
    }

    let tags = ollama_tags(&client)?;
    if let Some(digest) = qualified_model_digest(&tags, DEFAULT_MODEL_ID, DEFAULT_MODEL_DIGEST) {
        runtime.update(
            "ready",
            format!("{DEFAULT_MODEL_ID} is installed and qualified for signed whole-model work."),
            Some(version),
            Some(digest),
        );
        return Ok(());
    }

    runtime.update(
        "pulling_model",
        format!("Downloading {DEFAULT_MODEL_ID} once (about 2.5 GB), then verifying its digest."),
        Some(version.clone()),
        None,
    );
    let response = client
        .post(format!("{OLLAMA_ORIGIN}/api/pull"))
        .json(&serde_json::json!({"model": DEFAULT_MODEL_ID, "stream": false}))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Ollama could not pull {DEFAULT_MODEL_ID}: {error}"))?;
    let payload = read_bounded(response, MAX_OLLAMA_RESPONSE_BYTES)?;
    let result: Value = serde_json::from_slice(&payload)
        .map_err(|error| format!("Ollama returned an invalid pull result: {error}"))?;
    if result.get("status").and_then(Value::as_str) != Some("success") {
        return Err("Ollama did not report a successful model pull".into());
    }
    let tags = ollama_tags(&client)?;
    let digest =
        qualified_model_digest(&tags, DEFAULT_MODEL_ID, DEFAULT_MODEL_DIGEST).ok_or_else(|| {
            "the pulled model digest does not match Rampage's qualified inventory".to_string()
        })?;
    runtime.update(
        "ready",
        format!("{DEFAULT_MODEL_ID} is installed and qualified for signed whole-model work."),
        Some(version),
        Some(digest),
    );
    Ok(())
}

fn ollama_version(client: &reqwest::blocking::Client) -> Option<String> {
    client
        .get(format!("{OLLAMA_ORIGIN}/api/version"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .ok()?
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn ollama_tags(client: &reqwest::blocking::Client) -> Result<Value, String> {
    let response = client
        .get(format!("{OLLAMA_ORIGIN}/api/tags"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("could not read Ollama's model inventory: {error}"))?;
    let payload = read_bounded(response, MAX_OLLAMA_RESPONSE_BYTES)?;
    serde_json::from_slice(&payload)
        .map_err(|error| format!("Ollama returned an invalid model inventory: {error}"))
}

fn read_bounded(mut response: reqwest::blocking::Response, limit: u64) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(format!("local AI response exceeds the {limit}-byte limit"));
    }
    let mut payload = Vec::new();
    response
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut payload)
        .map_err(|error| format!("could not read local AI response: {error}"))?;
    if payload.len() as u64 > limit {
        return Err(format!("local AI response exceeds the {limit}-byte limit"));
    }
    Ok(payload)
}

fn qualified_model_digest(tags: &Value, model_id: &str, expected_digest: &str) -> Option<String> {
    tags.get("models")?.as_array()?.iter().find_map(|model| {
        let candidate = model.get("model").or_else(|| model.get("name"))?.as_str()?;
        let digest = model.get("digest")?.as_str()?.trim().to_ascii_lowercase();
        (candidate.eq_ignore_ascii_case(model_id) && digest == expected_digest)
            .then(|| format!("sha256:{digest}"))
    })
}

fn wait_for_ollama(client: &reqwest::blocking::Client) -> Result<(), String> {
    for _ in 0..180 {
        if ollama_version(client).is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err("Ollama did not become ready within three minutes".into())
}

#[cfg(target_os = "windows")]
fn install_ollama() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = Command::new("winget.exe")
        .args([
            "install",
            "--id",
            "Ollama.Ollama",
            "--exact",
            "--version",
            OLLAMA_VERSION,
            "--source",
            "winget",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|error| format!("could not start the pinned winget installer: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "winget returned {status} while installing Ollama {OLLAMA_VERSION}"
        ))
    }
}

#[cfg(not(target_os = "windows"))]
fn install_ollama() -> Result<(), String> {
    Err("automatic Ollama installation is currently implemented only for Windows".into())
}

#[cfg(target_os = "windows")]
fn ensure_ollama_started() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "LOCALAPPDATA is unavailable".to_string())?;
    let executable = std::path::PathBuf::from(local)
        .join("Programs")
        .join("Ollama")
        .join("ollama.exe");
    if !executable.is_file() {
        return Err("the Ollama installer completed but ollama.exe is missing".into());
    }
    Command::new(executable)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not start Ollama's loopback service: {error}"))
}

#[cfg(not(target_os = "windows"))]
fn ensure_ollama_started() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_inventory_requires_an_exact_full_digest() {
        let digest = "a".repeat(64);
        let tags = serde_json::json!({"models": [{
            "name": "qwen3:4b",
            "digest": digest,
            "size": 2_500_000_000_u64
        }]});
        assert_eq!(
            qualified_model_digest(&tags, "qwen3:4b", &"a".repeat(64)),
            Some(format!("sha256:{}", "a".repeat(64)))
        );
        assert!(qualified_model_digest(&tags, "qwen3:8b", &"a".repeat(64)).is_none());
        let truncated = serde_json::json!({"models": [{
            "name": "qwen3:4b",
            "digest": "359d7dd4bcda"
        }]});
        assert!(qualified_model_digest(&truncated, "qwen3:4b", DEFAULT_MODEL_DIGEST).is_none());
        assert!(qualified_model_digest(&tags, "qwen3:4b", DEFAULT_MODEL_DIGEST).is_none());
    }

    #[test]
    fn diagnostic_disable_flag_is_exact() {
        assert!(bootstrap_disabled(Some("true".into())));
        assert!(bootstrap_disabled(Some("1".into())));
        assert!(!bootstrap_disabled(Some("yes".into())));
        assert!(!bootstrap_disabled(None));
    }
}
