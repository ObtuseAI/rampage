mod local_ai;
mod pairing;

#[cfg(target_os = "windows")]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rand::{TryRng as _, rngs::SysRng};
use std::{
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use sysinfo::{Pid, System};
use tauri::{
    AppHandle, Manager, State, WindowEvent,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use local_ai::{LocalAiRuntime, LocalAiRuntimeView};
use pairing::{PairingManager, PairingRequestView, PairingWindowView, WorkerPairingView};
#[cfg(not(target_os = "windows"))]
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_shell::{
    ShellExt,
    process::{CommandChild, CommandEvent},
};
#[cfg(target_os = "windows")]
use winreg::{
    RegKey, RegValue,
    enums::{HKEY_CURRENT_USER, RegType},
};

#[derive(Default)]
struct Sidecars(Mutex<Vec<CommandChild>>);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerRuntimeView {
    state: &'static str,
    node_id: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteAssistStatusView {
    supported: bool,
    enabled: bool,
    active: bool,
    session_id: Option<String>,
    mode: Option<String>,
    expires_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryStatusView {
    schema: &'static str,
    version: &'static str,
    role: &'static str,
    state: &'static str,
    healthy: bool,
    issues: Vec<String>,
    can_leave_fabric: bool,
    can_factory_reset: bool,
}

#[derive(Debug, serde::Deserialize)]
struct RecoveryControllerEndpointDisk {
    endpoint_id: String,
}

#[derive(Debug, serde::Deserialize)]
struct RecoveryControllerPinDisk {
    schema: String,
    endpoint: RecoveryControllerEndpointDisk,
    governor_public_key: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteAssistPolicyDisk {
    schema: String,
    enabled: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteAssistActiveDisk {
    schema: String,
    session_id: String,
    mode: String,
    controller_endpoint_id: String,
    updated_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
struct WorkerRuntime(Arc<Mutex<WorkerRuntimeView>>);

impl Default for WorkerRuntime {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(WorkerRuntimeView {
            state: "inactive",
            node_id: None,
            message: None,
        })))
    }
}

const BACKGROUND_ARG: &str = "--background";
const AUTOSTART_NAME: &str = "Rampage";
const OWNER_FABRIC_MARKER: &str = "owner-fabric-v1.ready";
const OWNER_CONFIRMED_MARKER: &str = "owner-confirmed-v1.ready";
const SETUP_REQUIRED_MARKER: &str = "setup-required-v1.ready";
const WORKER_PAIRING_INTENT_MARKER: &str = "worker-pairing-v1.pending";
const REMOTE_ASSIST_POLICY_FILE: &str = "remote-assist-policy.json";
const REMOTE_ASSIST_ACTIVE_FILE: &str = "remote-assist-active.json";
#[cfg(target_os = "windows")]
const FIREWALL_MARKER_FILE: &str = "firewall-private-v2.ready";
const SIDECAR_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const LEAVE_FABRIC_CONFIRMATION: &str = "LEAVE FABRIC";
const FACTORY_RESET_CONFIRMATION: &str = "RESET RAMPAGE";
#[cfg(target_os = "windows")]
const AUTOSTART_RUN_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(target_os = "windows")]
const AUTOSTART_APPROVAL_KEY: &str =
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";

fn fresh_intelligence_token() -> String {
    let mut token = [0_u8; 32];
    SysRng
        .try_fill_bytes(&mut token)
        .expect("system randomness is required for intelligence API tokens");
    hex::encode(token)
}

fn launched_in_background<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().any(|arg| arg.as_ref() == BACKGROUND_ARG)
}

fn should_focus_existing_instance(args: &[String]) -> bool {
    !launched_in_background(args)
}

fn diagnostic_exit_delay(value: Option<&str>) -> Option<Duration> {
    value
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|milliseconds| (1_000..=180_000).contains(milliseconds))
        .map(Duration::from_millis)
}

fn schedule_diagnostic_exit(app: &AppHandle) {
    let value = std::env::var("RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS").ok();
    let Some(delay) = diagnostic_exit_delay(value.as_deref()) else {
        return;
    };
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        handle.exit(0);
    });
}

fn diagnostic_instance_is_bounded() -> bool {
    diagnostic_exit_delay(
        std::env::var("RAMPAGE_DIAGNOSTIC_EXIT_AFTER_MS")
            .ok()
            .as_deref(),
    )
    .is_some()
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn descendant_pids(system: &System, root: Pid, output: &mut Vec<Pid>) {
    for (pid, process) in system.processes() {
        if process.parent() == Some(root) {
            descendant_pids(system, *pid, output);
            output.push(*pid);
        }
    }
}

fn wait_for_process_exit(pids: &[Pid], timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let system = System::new_all();
        let remaining = pids
            .iter()
            .copied()
            .filter(|pid| system.process(*pid).is_some())
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "sidecar processes did not exit before the recovery deadline: {}",
                remaining
                    .iter()
                    .map(|pid| pid.as_u32().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn kill_process_tree(child: CommandChild) -> Result<(), String> {
    // PyInstaller's one-file launcher supervises a child process. Terminate descendants first so
    // closing Rampage cannot strand an intelligence server or any future sidecar subprocess.
    let system = System::new_all();
    let mut descendants = Vec::new();
    let root = Pid::from_u32(child.pid());
    descendant_pids(&system, root, &mut descendants);
    let mut watched = descendants.clone();
    watched.push(root);
    for pid in descendants {
        if let Some(process) = system.process(pid) {
            let _ = process.kill();
        }
    }
    // Kill the root through the OS snapshot before asking the shell plugin to release it. During
    // Tauri shutdown the plugin runtime is already winding down, so relying on its channel alone
    // can strand a direct sidecar such as the controller.
    if let Some(process) = system.process(Pid::from_u32(child.pid())) {
        let _ = process.kill();
    }
    let _ = child.kill();
    wait_for_process_exit(&watched, SIDECAR_STOP_TIMEOUT)
}

fn stop_all_sidecars(app: &AppHandle) -> Result<usize, String> {
    let children = {
        let state = app.state::<Sidecars>();
        let mut sidecars = state
            .0
            .lock()
            .map_err(|_| "sidecar state lock poisoned".to_string())?;
        sidecars.drain(..).collect::<Vec<_>>()
    };
    let count = children.len();
    for child in children {
        kill_process_tree(child)?;
    }
    Ok(count)
}

fn runtime_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("RAMPAGE_DATA_DIR") {
        return Ok(PathBuf::from(path));
    }
    app.path()
        .app_data_dir()
        .map(|path| path.join("runtime"))
        .map_err(|error| error.to_string())
}

fn controller_bind() -> Result<SocketAddr, String> {
    let address = std::env::var("RAMPAGE_BIND").unwrap_or_else(|_| "127.0.0.1:47831".into());
    let address = address
        .parse::<SocketAddr>()
        .map_err(|error| format!("invalid RAMPAGE_BIND: {error}"))?;
    if !address.ip().is_loopback() {
        return Err("RAMPAGE_BIND must remain loopback-only".into());
    }
    Ok(address)
}

fn controller_origin() -> Result<String, String> {
    Ok(format!("http://{}", controller_bind()?))
}

fn setup_required(data_dir: &Path) -> bool {
    data_dir.join(SETUP_REQUIRED_MARKER).is_file()
}

fn ensure_neutral_first_run(data_dir: &Path) -> Result<bool, String> {
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let has_fabric_state = [
        SETUP_REQUIRED_MARKER,
        OWNER_FABRIC_MARKER,
        "remote-invite.json",
        "agent.controller-pin.json",
        "agent.enrolled",
        "agent.identity.json",
        "agent.key",
        "controller.token",
        "controller.db",
        "governor.key",
        "mesh.key",
        "storage.key",
    ]
    .iter()
    .any(|name| data_dir.join(name).exists());
    if has_fabric_state {
        return Ok(false);
    }
    write_new_marker(
        &data_dir.join(SETUP_REQUIRED_MARKER),
        b"rampage.setup-required.v1\n",
    )?;
    Ok(true)
}

fn fabric_role_at(data_dir: &Path) -> &'static str {
    if setup_required(data_dir) {
        "setup"
    } else if worker_enrollment_exists(data_dir) {
        "worker"
    } else {
        "owner"
    }
}

fn validate_owner_local_enrollment(data_dir: &Path) -> Result<(), String> {
    let pin_bytes = read_bounded_regular_file(
        &data_dir.join("agent.controller-pin.json"),
        64 * 1024,
        "local controller pin",
    )?;
    let pin: RecoveryControllerPinDisk = serde_json::from_slice(&pin_bytes)
        .map_err(|error| format!("local controller pin is invalid: {error}"))?;
    if pin.schema != "rampage.pinned-controller.v1" {
        return Err("local controller pin has an unexpected schema".into());
    }

    let enrollment = String::from_utf8(read_bounded_regular_file(
        &data_dir.join("agent.enrolled"),
        4 * 1024,
        "local enrollment marker",
    )?)
    .map_err(|error| format!("local enrollment marker is invalid UTF-8: {error}"))?;
    if enrollment.trim() != pin.endpoint.endpoint_id {
        return Err("local enrollment marker does not match its pinned controller".into());
    }

    let governor_secret = String::from_utf8(read_bounded_regular_file(
        &data_dir.join("governor.key"),
        128,
        "local governor key",
    )?)
    .map_err(|error| format!("local governor key is invalid UTF-8: {error}"))?;
    let governor_bytes = hex::decode(governor_secret.trim())
        .map_err(|error| format!("local governor key is invalid: {error}"))?;
    let governor_bytes: [u8; 32] = governor_bytes
        .try_into()
        .map_err(|_| "local governor key must contain exactly 32 bytes".to_string())?;
    let expected_public_key = hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&governor_bytes)
            .verifying_key()
            .to_bytes(),
    );
    if !pin
        .governor_public_key
        .eq_ignore_ascii_case(&expected_public_key)
    {
        return Err("local agent is pinned to a different owner controller".into());
    }
    Ok(())
}

fn enrollment_file_state(data_dir: &Path) -> (&'static str, Vec<String>) {
    let setup = setup_required(data_dir);
    let owner = data_dir.join(OWNER_FABRIC_MARKER).is_file();
    let invite = data_dir.join("remote-invite.json").is_file();
    let pin = data_dir.join("agent.controller-pin.json").is_file();
    let enrolled = data_dir.join("agent.enrolled").is_file();
    let identity = data_dir.join("agent.identity.json").is_file();
    let key = data_dir.join("agent.key").is_file();
    let mut issues = Vec::new();

    if setup {
        if owner || invite || pin || enrolled || identity || key {
            issues.push(
                "Setup mode still contains fabric credentials; run Reset Rampage again.".into(),
            );
            return ("cleanup_required", issues);
        }
        return ("ready_to_configure", issues);
    }
    if owner {
        if invite {
            issues.push("Owner runtime still contains a pending worker invitation.".into());
            return ("repair_required", issues);
        }
        if pin {
            if !enrolled || !identity || !key {
                issues.push("The owner's local worker identity is incomplete.".into());
                return ("repair_required", issues);
            }
            if let Err(error) = validate_owner_local_enrollment(data_dir) {
                issues.push(format!(
                    "The owner's local worker identity is inconsistent: {error}."
                ));
                return ("repair_required", issues);
            }
            return ("owner_active", issues);
        }
        if enrolled {
            issues.push(
                "The owner has an enrollment marker without a pinned local controller.".into(),
            );
            return ("repair_required", issues);
        }
        return ("owner_active", issues);
    }
    if pin {
        if !enrolled || !identity || !key {
            issues.push(
                "The paired-worker identity is incomplete or was interrupted during an update."
                    .into(),
            );
            return ("repair_required", issues);
        }
        return ("worker_paired", issues);
    }
    if invite {
        return ("enrollment_pending", issues);
    }
    if enrolled || identity || key {
        issues.push("Local worker identity exists without a pinned owner.".into());
        return ("repair_required", issues);
    }
    ("owner_starting", issues)
}

fn recovery_status_at(data_dir: &Path) -> RecoveryStatusView {
    let role = fabric_role_at(data_dir);
    let (state, issues) = enrollment_file_state(data_dir);
    RecoveryStatusView {
        schema: "rampage.recovery-status.v1",
        version: env!("CARGO_PKG_VERSION"),
        role,
        state,
        healthy: issues.is_empty(),
        issues,
        can_leave_fabric: role == "worker",
        can_factory_reset: true,
    }
}

fn validate_runtime_reset_target(data_dir: &Path) -> Result<(), String> {
    if !data_dir.is_absolute()
        || data_dir.file_name().and_then(|value| value.to_str()) != Some("runtime")
        || data_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case("ai.obtuse.rampage"))
    {
        return Err("refusing to reset a path outside the Rampage runtime directory".into());
    }
    if data_dir.exists() {
        let metadata = std::fs::symlink_metadata(data_dir).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Rampage runtime path is not a regular directory".into());
        }
    }
    Ok(())
}

fn remove_runtime_entry(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        std::fs::remove_file(path).map_err(|error| error.to_string())
    } else if metadata.is_dir() {
        std::fs::remove_dir_all(path).map_err(|error| error.to_string())
    } else {
        Err(format!(
            "refusing to reset unsupported runtime entry {}",
            path.display()
        ))
    }
}

fn reset_runtime_to_setup(data_dir: &Path) -> Result<(), String> {
    validate_runtime_reset_target(data_dir)?;
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(data_dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        remove_runtime_entry(&entry.path())?;
    }
    write_new_marker(
        &data_dir.join(SETUP_REQUIRED_MARKER),
        b"rampage.setup-required.v1\n",
    )
}

fn remove_stale_worker_credentials_in_setup(data_dir: &Path) -> Result<usize, String> {
    if !setup_required(data_dir) {
        return Ok(0);
    }
    let mut removed = 0;
    for name in [
        "remote-invite.json",
        "agent.controller-pin.json",
        "agent.enrolled",
        "agent.identity.json",
        "agent.key",
        REMOTE_ASSIST_POLICY_FILE,
        REMOTE_ASSIST_ACTIVE_FILE,
    ] {
        let path = data_dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(format!(
                    "refusing to replace non-regular stale pairing entry {}",
                    path.display()
                ));
            }
            Ok(_) => {
                std::fs::remove_file(&path).map_err(|error| {
                    format!(
                        "could not remove stale pairing entry {}: {error}",
                        path.display()
                    )
                })?;
                removed += 1;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(removed)
}

fn prepare_worker_pairing_files(data_dir: &Path) -> Result<(), String> {
    match fabric_role_at(data_dir) {
        "worker" => return Err("this machine is already enrolled as a worker".into()),
        "owner" if data_dir.join(OWNER_CONFIRMED_MARKER).is_file() => {
            return Err(
                "this machine owns a configured fabric; use Reset Rampage before joining another owner"
                    .into(),
            );
        }
        "owner" => {
            #[cfg(target_os = "windows")]
            let firewall_marker = read_bounded_regular_file(
                &data_dir.join(FIREWALL_MARKER_FILE),
                4096,
                "private-network firewall marker",
            )
            .ok();
            reset_runtime_to_setup(data_dir)?;
            #[cfg(target_os = "windows")]
            if let Some(marker) = firewall_marker {
                write_new_marker(&data_dir.join(FIREWALL_MARKER_FILE), &marker)?;
            }
        }
        "setup" => {}
        _ => return Err("Rampage could not determine this machine's fabric role".into()),
    }
    remove_stale_worker_credentials_in_setup(data_dir)?;
    let intent = data_dir.join(WORKER_PAIRING_INTENT_MARKER);
    let expected = b"rampage.worker-pairing.v1\n";
    match read_bounded_regular_file(&intent, 128, "worker pairing intent") {
        Ok(bytes) if bytes == expected => Ok(()),
        Ok(_) => Err(
            "worker pairing intent has unexpected contents; use Fix Rampage before retrying".into(),
        ),
        Err(_) if intent.exists() => Err(
            "worker pairing intent is not a bounded regular file; use Fix Rampage before retrying"
                .into(),
        ),
        Err(_) => write_new_marker(&intent, expected),
    }
}

#[tauri::command]
fn recovery_status(app: AppHandle) -> Result<RecoveryStatusView, String> {
    Ok(recovery_status_at(&runtime_dir(&app)?))
}

#[tauri::command]
fn repair_connection(app: AppHandle) -> Result<(), String> {
    let data_dir = runtime_dir(&app)?;
    let (_, issues) = enrollment_file_state(&data_dir);
    if !issues.is_empty() {
        return Err("Enrollment files are inconsistent; use Leave fabric or Reset Rampage.".into());
    }
    app.restart()
}

#[tauri::command]
fn leave_fabric(app: AppHandle, confirmation: String) -> Result<(), String> {
    if confirmation != LEAVE_FABRIC_CONFIRMATION {
        return Err("exact LEAVE FABRIC confirmation is required".into());
    }
    let data_dir = runtime_dir(&app)?;
    if fabric_role_at(&data_dir) != "worker" {
        return Err("Leave fabric is available only on a paired worker".into());
    }
    stop_all_sidecars(&app)?;
    reset_runtime_to_setup(&data_dir)?;
    app.restart()
}

#[tauri::command]
fn factory_reset(app: AppHandle, confirmation: String) -> Result<(), String> {
    if confirmation != FACTORY_RESET_CONFIRMATION {
        return Err("exact RESET RAMPAGE confirmation is required".into());
    }
    let data_dir = runtime_dir(&app)?;
    stop_all_sidecars(&app)?;
    platform_set_autostart(&app, false)?;
    reset_runtime_to_setup(&data_dir)?;
    app.restart()
}

#[tauri::command]
fn activate_owner_fabric(app: AppHandle) -> Result<(), String> {
    let data_dir = runtime_dir(&app)?;
    if worker_enrollment_exists(&data_dir) {
        return Err("this device is still paired as a worker".into());
    }
    let setup_marker = data_dir.join(SETUP_REQUIRED_MARKER);
    if setup_marker.is_file() {
        let owner_marker = data_dir.join(OWNER_FABRIC_MARKER);
        let confirmed_marker = data_dir.join(OWNER_CONFIRMED_MARKER);
        write_new_marker(&owner_marker, b"rampage.owner-fabric.v1\n")?;
        if let Err(error) = write_new_marker(&confirmed_marker, b"rampage.owner-confirmed.v1\n") {
            let _ = std::fs::remove_file(owner_marker);
            return Err(error);
        }
        if let Err(error) = std::fs::remove_file(setup_marker) {
            let _ = std::fs::remove_file(owner_marker);
            let _ = std::fs::remove_file(confirmed_marker);
            return Err(format!("could not activate the owner fabric: {error}"));
        }
        app.restart();
    }
    Ok(())
}

#[tauri::command]
fn confirm_owner_fabric(app: AppHandle) -> Result<(), String> {
    let data_dir = runtime_dir(&app)?;
    if fabric_role_at(&data_dir) != "owner" || !data_dir.join(OWNER_FABRIC_MARKER).is_file() {
        return Err("only an active owner fabric can be confirmed".into());
    }
    write_new_marker(
        &data_dir.join(OWNER_CONFIRMED_MARKER),
        b"rampage.owner-confirmed.v1\n",
    )
}

#[tauri::command]
fn local_stop(app: AppHandle) -> Result<(), String> {
    let data_dir = runtime_dir(&app)?;
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    let kill_path = data_dir.join("KILL");
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&kill_path)
    {
        Ok(mut file) => {
            file.write_all(b"owner-stop-v1\n")
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    match std::fs::remove_file(data_dir.join(REMOTE_ASSIST_ACTIVE_FILE)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    propagate_controller_stop(data_dir);
    Ok(())
}

fn propagate_controller_stop(data_dir: PathBuf) {
    let token_path = data_dir.join("controller.token");
    let Ok(token) = read_bounded_regular_file(&token_path, 512, "controller token")
        .and_then(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
    else {
        return;
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        return;
    }
    let Ok(controller) = controller_origin() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let _ = reqwest::Client::new()
            .post(format!("{controller}/v1/stop"))
            .header("x-rampage-token", token)
            .send()
            .await;
    });
}

#[tauri::command]
fn fabric_mode(app: AppHandle) -> Result<&'static str, String> {
    Ok(fabric_role_at(&runtime_dir(&app)?))
}

fn remote_assist_enabled_at(data_dir: &std::path::Path) -> bool {
    let path = data_dir.join(REMOTE_ASSIST_POLICY_FILE);
    let Ok(bytes) = read_bounded_regular_file(&path, 4 * 1024, "Remote Assist policy") else {
        return false;
    };
    serde_json::from_slice::<RemoteAssistPolicyDisk>(&bytes)
        .is_ok_and(|policy| policy.schema == "rampage.remote-assist-policy.v1" && policy.enabled)
}

fn remote_assist_active_at(data_dir: &std::path::Path) -> Option<RemoteAssistActiveDisk> {
    let path = data_dir.join(REMOTE_ASSIST_ACTIVE_FILE);
    let bytes = read_bounded_regular_file(&path, 16 * 1024, "Remote Assist activity").ok()?;
    let active = serde_json::from_slice::<RemoteAssistActiveDisk>(&bytes).ok()?;
    let now = chrono::Utc::now();
    (active.schema == "rampage.remote-assist-active.v1"
        && !active.session_id.is_empty()
        && !active.controller_endpoint_id.is_empty()
        && matches!(active.mode.as_str(), "view" | "control")
        && active.updated_at <= now + chrono::Duration::seconds(5)
        && active.expires_at > now
        && active.expires_at - active.updated_at <= chrono::Duration::seconds(35))
    .then_some(active)
}

#[tauri::command]
fn remote_assist_status(app: AppHandle) -> Result<RemoteAssistStatusView, String> {
    let data_dir = runtime_dir(&app)?;
    let supported = cfg!(target_os = "windows") && worker_enrollment_exists(&data_dir);
    let enabled = supported && remote_assist_enabled_at(&data_dir);
    let active = enabled
        .then(|| remote_assist_active_at(&data_dir))
        .flatten();
    Ok(RemoteAssistStatusView {
        supported,
        enabled,
        active: active.is_some(),
        session_id: active.as_ref().map(|value| value.session_id.clone()),
        mode: active.as_ref().map(|value| value.mode.clone()),
        expires_at: active.map(|value| value.expires_at.to_rfc3339()),
    })
}

#[tauri::command]
fn set_remote_assist_enabled(
    app: AppHandle,
    enabled: bool,
) -> Result<RemoteAssistStatusView, String> {
    let data_dir = runtime_dir(&app)?;
    if !cfg!(target_os = "windows") {
        return Err("Remote Assist is currently qualified only on Windows workers".into());
    }
    if !worker_enrollment_exists(&data_dir) {
        return Err("Remote Assist can only be enabled on a paired worker".into());
    }
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    let path = data_dir.join(REMOTE_ASSIST_POLICY_FILE);
    if path.exists() {
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("Remote Assist policy path is not a regular non-symlink file".into());
        }
    }
    let payload = serde_json::to_vec(&serde_json::json!({
        "schema": "rampage.remote-assist-policy.v1",
        "enabled": enabled
    }))
    .map_err(|error| error.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options.open(&path).map_err(|error| error.to_string())?;
    file.write_all(&payload)
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    if !enabled {
        match std::fs::remove_file(data_dir.join(REMOTE_ASSIST_ACTIVE_FILE)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    remote_assist_status(app)
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> Result<bool, String> {
    platform_autostart_enabled(&app)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    platform_set_autostart(&app, enabled)?;
    platform_autostart_enabled(&app)
}

#[cfg(target_os = "windows")]
fn autostart_command() -> Result<String, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(format!("\"{}\" {BACKGROUND_ARG}", executable.display()))
}

#[cfg(target_os = "windows")]
fn platform_autostart_enabled(_app: &AppHandle) -> Result<bool, String> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run = current_user
        .open_subkey(AUTOSTART_RUN_KEY)
        .map_err(|error| error.to_string())?;
    let configured: String = match run.get_value(AUTOSTART_NAME) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    Ok(configured == autostart_command()?)
}

#[cfg(target_os = "windows")]
fn platform_set_autostart(_app: &AppHandle, enabled: bool) -> Result<(), String> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run = current_user
        .open_subkey_with_flags(AUTOSTART_RUN_KEY, winreg::enums::KEY_SET_VALUE)
        .map_err(|error| error.to_string())?;
    if enabled {
        run.set_value(AUTOSTART_NAME, &autostart_command()?)
            .map_err(|error| error.to_string())?;
        if let Ok(approved) = current_user
            .open_subkey_with_flags(AUTOSTART_APPROVAL_KEY, winreg::enums::KEY_SET_VALUE)
        {
            approved
                .set_raw_value(
                    AUTOSTART_NAME,
                    &RegValue {
                        vtype: RegType::REG_BINARY,
                        bytes: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    },
                )
                .map_err(|error| error.to_string())?;
        }
    } else {
        if let Err(error) = run.delete_value(AUTOSTART_NAME)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.to_string());
        }
        if let Ok(approved) = current_user
            .open_subkey_with_flags(AUTOSTART_APPROVAL_KEY, winreg::enums::KEY_SET_VALUE)
            && let Err(error) = approved.delete_value(AUTOSTART_NAME)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.to_string());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn platform_autostart_enabled(app: &AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "windows"))]
fn platform_set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn controller_token(app: AppHandle) -> Result<String, String> {
    let path = runtime_dir(&app)?.join("controller.token");
    String::from_utf8(read_bounded_regular_file(&path, 512, "controller token")?)
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("controller token is not UTF-8: {error}"))
}

fn local_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Rampage Device".into())
}

#[tauri::command]
async fn open_pairing_window(
    app: AppHandle,
    pairing: State<'_, PairingManager>,
) -> Result<PairingWindowView, String> {
    if fabric_mode(app.clone())? != "owner" {
        return Err("only the owner PC can approve a new machine".into());
    }
    let firewall_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || ensure_private_network_firewall(&firewall_app))
        .await
        .map_err(|error| error.to_string())??;
    pairing::open_owner_window(&pairing, app, local_device_name()).await
}

#[tauri::command]
fn pairing_window(pairing: State<'_, PairingManager>) -> Result<PairingWindowView, String> {
    pairing::owner_window(&pairing)
}

#[tauri::command]
async fn begin_pairing(
    app: AppHandle,
    pairing: State<'_, PairingManager>,
) -> Result<WorkerPairingView, String> {
    let data_dir = runtime_dir(&app)?;
    if fabric_role_at(&data_dir) == "owner" {
        if data_dir.join(OWNER_CONFIRMED_MARKER).is_file() {
            return Err(
                "this machine owns a configured fabric; use Reset Rampage before joining another owner"
                    .into(),
            );
        }
        stop_all_sidecars(&app)?;
    }
    prepare_worker_pairing_files(&data_dir)?;
    let firewall_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || ensure_private_network_firewall(&firewall_app))
        .await
        .map_err(|error| error.to_string())??;
    pairing::begin_worker(app, &pairing, local_device_name())
}

#[tauri::command]
fn pairing_status(pairing: State<'_, PairingManager>) -> Result<WorkerPairingView, String> {
    pairing::worker_status(&pairing)
}

#[tauri::command]
fn cancel_pairing(app: AppHandle, pairing: State<'_, PairingManager>) -> Result<(), String> {
    pairing::cancel_worker(&pairing)?;
    let intent = runtime_dir(&app)?.join(WORKER_PAIRING_INTENT_MARKER);
    match std::fs::remove_file(intent) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not cancel the pairing transaction: {error}")),
    }
}

#[tauri::command]
async fn approve_pairing(
    app: AppHandle,
    pairing: State<'_, PairingManager>,
    request_id: String,
) -> Result<PairingRequestView, String> {
    if fabric_mode(app.clone())? != "owner" {
        return Err("only the owner PC can approve a new machine".into());
    }
    let token = controller_token(app)?;
    let controller = controller_origin()?;
    let response = reqwest::Client::new()
        .post(format!("{controller}/v1/enrollment/invites"))
        .header("x-rampage-token", token)
        .send()
        .await
        .map_err(|error| format!("could not create the encrypted enrollment invite: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "controller refused enrollment approval with status {}",
            response.status()
        ));
    }
    let invitation = response
        .text()
        .await
        .map_err(|error| format!("could not read the enrollment invite: {error}"))?;
    pairing::approve(&pairing, &request_id, &invitation).await
}

#[tauri::command]
async fn reject_pairing(
    pairing: State<'_, PairingManager>,
    request_id: String,
) -> Result<(), String> {
    pairing::reject(&pairing, &request_id).await
}

#[tauri::command]
fn join_remote(app: AppHandle, invitation: String) -> Result<(), String> {
    let data_dir = runtime_dir(&app)?;
    if fabric_role_at(&data_dir) == "owner" {
        if data_dir.join(OWNER_CONFIRMED_MARKER).is_file() {
            return Err(
                "this machine owns a configured fabric; use Reset Rampage before joining another owner"
                    .into(),
            );
        }
        stop_all_sidecars(&app)?;
    }
    prepare_worker_pairing_files(&data_dir)?;
    persist_remote_invite(&app, &invitation)?;
    app.restart()
}

fn persist_remote_invite(app: &AppHandle, invitation: &str) -> Result<(), String> {
    persist_remote_invite_at(&runtime_dir(app)?, invitation)
}

fn persist_remote_invite_at(data_dir: &Path, invitation: &str) -> Result<(), String> {
    let parsed: serde_json::Value = serde_json::from_str(invitation)
        .map_err(|error| format!("invite is not valid JSON: {error}"))?;
    if parsed.get("schema").and_then(serde_json::Value::as_str)
        != Some("rampage.enrollment-invite.v1")
        || parsed
            .pointer("/controller_mesh/signature")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
        || parsed
            .get("governor_public_key")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("invite is missing its signed Rampage mesh endpoint".into());
    }
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    if !setup_required(data_dir) || !data_dir.join(WORKER_PAIRING_INTENT_MARKER).is_file() {
        return Err(
            "the protected nearby-pairing transaction is not active; start Join my fabric again"
                .into(),
        );
    }
    remove_stale_worker_credentials_in_setup(data_dir)?;
    let destination = data_dir.join("remote-invite.json");
    if destination.exists() || data_dir.join("agent.controller-pin.json").exists() {
        return Err("this machine is already enrolled; remove it from the current fabric before enrolling again".into());
    }
    let temporary = data_dir.join(format!(
        "remote-invite.{}.tmp",
        &fresh_intelligence_token()[..16]
    ));
    let write_result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("could not create the protected enrollment file: {error}"))?;
        file.write_all(invitation.as_bytes())
            .map_err(|error| format!("could not write the enrollment file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("could not durably store the enrollment file: {error}"))?;
        drop(file);
        std::fs::rename(&temporary, &destination)
            .map_err(|error| format!("could not activate the enrollment file: {error}"))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result?;
    let setup_marker = data_dir.join(SETUP_REQUIRED_MARKER);
    if setup_marker.is_file()
        && let Err(error) = std::fs::remove_file(&setup_marker)
    {
        let _ = std::fs::remove_file(&destination);
        return Err(format!(
            "could not leave setup mode after enrollment: {error}"
        ));
    }
    let intent_marker = data_dir.join(WORKER_PAIRING_INTENT_MARKER);
    if let Err(error) = std::fs::remove_file(&intent_marker) {
        let _ = std::fs::remove_file(&destination);
        let _ = write_new_marker(
            &data_dir.join(SETUP_REQUIRED_MARKER),
            b"rampage.setup-required.v1\n",
        );
        return Err(format!(
            "could not commit the protected pairing transaction: {error}"
        ));
    }
    Ok(())
}

fn launch_remote_worker(app: &AppHandle, data_dir: &std::path::Path) -> Result<(), String> {
    let runtime = app.state::<WorkerRuntime>().inner().clone();
    if let Ok(mut status) = runtime.0.lock() {
        *status = WorkerRuntimeView {
            state: "starting",
            node_id: None,
            message: Some(
                "Connecting to the owner PC and publishing a signed resource offer.".into(),
            ),
        };
    }
    let handle = app.clone();
    let data_dir = data_dir.to_path_buf();
    tauri::async_runtime::spawn(async move {
        let mut retry_delay = Duration::from_secs(1);
        loop {
            if !worker_enrollment_exists(&data_dir) || data_dir.join("KILL").is_file() {
                if let Ok(mut status) = runtime.0.lock() {
                    *status = WorkerRuntimeView {
                        state: "inactive",
                        node_id: None,
                        message: Some("Worker sharing is stopped by local policy.".into()),
                    };
                }
                return;
            }
            let invite_file = data_dir.join("remote-invite.json");
            let key_file = data_dir.join("agent.key");
            let mut command = match handle.shell().sidecar("rampage-agent") {
                Ok(command) => command.env("RAMPAGE_DATA_DIR", &data_dir).args([
                    "--key-file".into(),
                    key_file.to_string_lossy().into_owned(),
                    "--display-name".into(),
                    local_device_name(),
                    "--device-kind".into(),
                    "desktop".into(),
                    "--serve".into(),
                ]),
                Err(error) => {
                    set_worker_retrying(&runtime, format!("Worker launch failed: {error}"));
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(10));
                    continue;
                }
            };
            if invite_file.is_file() {
                command = command.args([
                    "--invite-file".into(),
                    invite_file.to_string_lossy().into_owned(),
                ]);
            }
            let (mut events, agent) = match command.spawn() {
                Ok(spawned) => spawned,
                Err(error) => {
                    set_worker_retrying(&runtime, format!("Worker launch failed: {error}"));
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(10));
                    continue;
                }
            };
            if let Ok(mut sidecars) = handle.state::<Sidecars>().0.lock() {
                sidecars.push(agent);
            }
            let mut last_error: Option<String> = None;
            let mut ready_announced = false;
            while let Some(event) = events.recv().await {
                match event {
                    CommandEvent::Stdout(bytes) => {
                        let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
                        if value
                            .as_ref()
                            .and_then(|value| value.get("schema"))
                            .and_then(serde_json::Value::as_str)
                            == Some("rampage.worker-ready.v1")
                        {
                            ready_announced = true;
                            retry_delay = Duration::from_secs(1);
                            if let Ok(mut status) = runtime.0.lock() {
                                *status = WorkerRuntimeView {
                                    state: "active",
                                    node_id: value
                                        .as_ref()
                                        .and_then(|value| value.get("node_id"))
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::to_string),
                                    message: Some(
                                        "Signed compute offer accepted by the owner PC.".into(),
                                    ),
                                };
                            }
                        }
                    }
                    CommandEvent::Stderr(bytes) => {
                        if let Some(message) = bounded_worker_retry_message(&bytes) {
                            last_error = Some(message.clone());
                            if !ready_announced {
                                set_worker_retrying(&runtime, message);
                            }
                        }
                    }
                    CommandEvent::Error(error) => {
                        last_error = Some(error.chars().take(512).collect());
                    }
                    CommandEvent::Terminated(payload) => {
                        last_error.get_or_insert_with(|| {
                            format!("Worker stopped unexpectedly (code {:?}).", payload.code)
                        });
                        break;
                    }
                    _ => {}
                }
            }
            if !worker_enrollment_exists(&data_dir) || data_dir.join("KILL").is_file() {
                continue;
            }
            set_worker_retrying(
                &runtime,
                format!(
                    "{} Restarting automatically in {} second(s).",
                    last_error.unwrap_or_else(|| "Worker connection ended.".into()),
                    retry_delay.as_secs()
                ),
            );
            tokio::time::sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(Duration::from_secs(10));
        }
    });
    Ok(())
}

fn set_worker_retrying(runtime: &WorkerRuntime, message: String) {
    if let Ok(mut status) = runtime.0.lock() {
        *status = WorkerRuntimeView {
            state: "retrying",
            node_id: None,
            message: Some(message),
        };
    }
}

fn bounded_worker_retry_message(bytes: &[u8]) -> Option<String> {
    let detail = String::from_utf8_lossy(bytes)
        .trim()
        .chars()
        .take(384)
        .collect::<String>();
    (!detail.is_empty()).then(|| format!("Automatic signed-route recovery is retrying. {detail}"))
}

#[cfg(target_os = "windows")]
fn ensure_private_network_firewall(app: &AppHandle) -> Result<(), String> {
    let data_dir = runtime_dir(app)?;
    let install_dir = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .ok_or_else(|| "Rampage installation directory is unavailable".to_string())?
        .to_path_buf();
    let marker = data_dir.join(FIREWALL_MARKER_FILE);
    let marker_body = format!(
        "rampage.firewall-private.v2\ninstall_dir={}\n",
        install_dir.to_string_lossy()
    );
    match read_bounded_regular_file(&marker, 4096, "private-network firewall marker") {
        Ok(bytes) if bytes == marker_body.as_bytes() => return Ok(()),
        Ok(_) => std::fs::remove_file(&marker).map_err(|error| error.to_string())?,
        Err(_) if marker.exists() => {
            return Err("private-network firewall marker is not a bounded regular file".into());
        }
        Err(_) => {}
    }
    let escaped = |path: PathBuf| path.to_string_lossy().replace('\'', "''");
    let desktop = escaped(install_dir.join("rampage-desktop.exe"));
    let controller = escaped(install_dir.join("rampage-controller.exe"));
    let agent = escaped(install_dir.join("rampage-agent.exe"));
    let script = format!(
        "$ErrorActionPreference='Stop';\
         $rules=@(\
           @{{Name='Rampage Nearby Pairing (Private UDP 47839)';Program='{desktop}';Port='47839'}},\
           @{{Name='Rampage Fabric Controller (Private UDP)';Program='{controller}';Port='Any'}},\
           @{{Name='Rampage Fabric Worker (Private UDP)';Program='{agent}';Port='Any'}}\
         );\
         foreach($rule in $rules){{\
           Get-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue | Remove-NetFirewallRule;\
           New-NetFirewallRule -DisplayName $rule.Name -Direction Inbound -Action Allow -Profile Private -Protocol UDP -LocalPort $rule.Port -Program $rule.Program | Out-Null\
         }}"
    );
    let encoded_bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = BASE64.encode(encoded_bytes);
    let launcher = format!(
        "$ErrorActionPreference='Stop';$p=Start-Process -FilePath powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile -EncodedCommand {encoded}';exit $p.ExitCode"
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &launcher])
        .status()
        .map_err(|error| {
            format!("could not request the Windows private-network allowance: {error}")
        })?;
    if !status.success() {
        return Err("Windows did not approve Rampage for this private network.".into());
    }
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    write_new_marker(&marker, marker_body.as_bytes())?;
    let previous = data_dir.join("firewall-private-v1.ready");
    if previous.is_file() {
        let _ = std::fs::remove_file(previous);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn ensure_private_network_firewall(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}

fn write_new_marker(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    match std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn worker_runtime_status(runtime: State<'_, WorkerRuntime>) -> Result<WorkerRuntimeView, String> {
    runtime
        .0
        .lock()
        .map(|status| status.clone())
        .map_err(|_| "worker runtime state lock poisoned".to_string())
}

#[tauri::command]
fn local_ai_runtime_status(
    runtime: State<'_, LocalAiRuntime>,
) -> Result<LocalAiRuntimeView, String> {
    runtime.view()
}

#[tauri::command]
async fn run_fabric_benchmark(app: AppHandle) -> Result<serde_json::Value, String> {
    const MAX_RESULT_BYTES: usize = 1024 * 1024;
    if fabric_mode(app.clone())? != "owner" {
        return Err("only the owner PC can benchmark the compute fabric".into());
    }
    let data_dir = runtime_dir(&app)?;
    let output = app
        .shell()
        .sidecar("rampage")
        .map_err(|error| error.to_string())?
        .env("RAMPAGE_DATA_DIR", data_dir)
        .args([
            "benchmark",
            "--cores-per-node",
            "4",
            "--iterations-per-core",
            "5000000",
            "--timeout-seconds",
            "180",
        ])
        .output()
        .await
        .map_err(|error| format!("could not start the signed fabric benchmark: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(1024)
            .collect::<String>();
        return Err(format!("fabric benchmark failed: {}", error.trim()));
    }
    if output.stdout.len() > MAX_RESULT_BYTES {
        return Err("fabric benchmark result exceeded the 1 MiB desktop limit".into());
    }
    let result: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("fabric benchmark returned invalid JSON: {error}"))?;
    if result.get("schema").and_then(serde_json::Value::as_str)
        != Some("rampage.fabric-benchmark-result.v1")
    {
        return Err("fabric benchmark returned an unexpected result schema".into());
    }
    Ok(result)
}

fn worker_enrollment_exists(data_dir: &std::path::Path) -> bool {
    !data_dir.join(SETUP_REQUIRED_MARKER).is_file()
        && !data_dir.join(OWNER_FABRIC_MARKER).is_file()
        && (data_dir.join("remote-invite.json").is_file()
            || data_dir.join("agent.controller-pin.json").is_file())
}

fn launch_owner_relay_if_configured(
    app: &AppHandle,
    data_dir: &std::path::Path,
) -> Result<(), String> {
    let config = data_dir.join("rampage-relay.json");
    if !config.is_file() {
        return Ok(());
    }
    let (_, relay) = app
        .shell()
        .sidecar("rampage-relay")
        .map_err(|error| error.to_string())?
        .args([
            "serve".into(),
            "--config".into(),
            config.to_string_lossy().into_owned(),
        ])
        .spawn()
        .map_err(|error| format!("could not start owner relay: {error}"))?;
    app.state::<Sidecars>()
        .0
        .lock()
        .map_err(|_| "sidecar state lock poisoned".to_string())?
        .push(relay);
    Ok(())
}

fn read_bounded_regular_file(
    path: &std::path::Path,
    limit: u64,
    label: &str,
) -> Result<Vec<u8>, String> {
    let path_metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {label} path: {error}"))?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(format!("{label} is not a regular non-symlink file"));
    }
    let file =
        std::fs::File::open(path).map_err(|error| format!("could not open {label}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect {label}: {error}"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!("{label} is not a bounded regular file"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {label}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} exceeds its size limit"));
    }
    Ok(bytes)
}

fn configured_private_relay(data_dir: &std::path::Path) -> Result<Option<String>, String> {
    let config_path = data_dir.join("rampage-relay.json");
    if !config_path.is_file() {
        return Ok(None);
    }
    let bytes = read_bounded_regular_file(&config_path, 1024 * 1024, "owner relay configuration")?;
    let config: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if config.get("schema").and_then(serde_json::Value::as_str)
        != Some("rampage.owner-relay-config.v1")
    {
        return Err("owner relay configuration has an unsupported schema".into());
    }
    let public_url = config
        .get("public_url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "owner relay configuration is missing public_url".to_string())?;
    let parsed = reqwest::Url::parse(public_url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("owner relay public_url must be credential-free HTTPS".into());
    }
    Ok(Some(public_url.to_string()))
}

fn launch_fabric(app: &AppHandle) -> Result<(), String> {
    let data_dir = runtime_dir(app)?;
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    ensure_neutral_first_run(&data_dir)?;
    if setup_required(&data_dir) {
        return Ok(());
    }
    if worker_enrollment_exists(&data_dir) {
        return launch_remote_worker(app, &data_dir);
    }
    write_new_marker(
        &data_dir.join(OWNER_FABRIC_MARKER),
        b"rampage.owner-fabric.v1\n",
    )?;
    let intelligence_dir = data_dir.join("intelligence");
    std::fs::create_dir_all(&intelligence_dir).map_err(|error| error.to_string())?;
    let private_relay = configured_private_relay(&data_dir)?;
    let controller_bind = controller_bind()?;
    let controller_origin = format!("http://{controller_bind}");
    let mut controller_command = app
        .shell()
        .sidecar("rampage-controller")
        .map_err(|error| error.to_string())?
        .env("RAMPAGE_DATA_DIR", &data_dir)
        .env("RAMPAGE_BIND", controller_bind.to_string());
    if let Some(relay) = private_relay {
        controller_command = controller_command.env("RAMPAGE_PRIVATE_RELAYS", relay);
    }
    let (_, controller) = controller_command
        .spawn()
        .map_err(|error| format!("could not start controller: {error}"))?;
    app.state::<Sidecars>()
        .0
        .lock()
        .map_err(|_| "sidecar state lock poisoned".to_string())?
        .push(controller);
    let token_path = data_dir.join("controller.token");
    // A cold Windows start can include executable reputation scanning and PyInstaller extraction.
    // Keep the setup wait bounded, but do not fail a healthy first launch after only two seconds.
    for _ in 0..300 {
        if token_path.is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let token = String::from_utf8(read_bounded_regular_file(
        &token_path,
        512,
        "controller token",
    )?)
    .map_err(|_| "controller token is not UTF-8".to_string())?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("controller token is empty".into());
    }
    launch_owner_relay_if_configured(app, &data_dir)?;
    let intelligence_token = fresh_intelligence_token();
    let (_, intelligence) = app
        .shell()
        .sidecar("rampage-intelligence")
        .map_err(|error| error.to_string())?
        .env("RAMPAGE_DATA_DIR", &intelligence_dir)
        .env("RAMPAGE_ENABLE_MODELS", "false")
        // The proposal-only sidecar never receives the controller owner token. Compromise of the
        // Python process therefore cannot authenticate directly to controller authority routes.
        .env("RAMPAGE_TOKEN", intelligence_token)
        .spawn()
        .map_err(|error| format!("could not start intelligence service: {error}"))?;
    app.state::<Sidecars>()
        .0
        .lock()
        .map_err(|_| "sidecar state lock poisoned".to_string())?
        .push(intelligence);

    let handle = app.clone();
    let agent_controller = controller_origin.clone();
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        let mut ready = false;
        for _ in 0..80 {
            if client
                .get(format!("{controller_origin}/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            return;
        }
        let Ok(data_dir) = runtime_dir(&handle) else {
            return;
        };
        let key_file = data_dir.join("agent.key");
        let controller_pin = key_file.with_extension("controller-pin.json");
        let mut agent_args = vec![
            "--controller".into(),
            agent_controller,
            "--key-file".into(),
            key_file.to_string_lossy().into_owned(),
        ];
        if !controller_pin.is_file() {
            let Ok(response) = client
                .post(format!("{controller_origin}/v1/enrollment/invites"))
                .header("x-rampage-token", &token)
                .json(&serde_json::json!({}))
                .send()
                .await
            else {
                return;
            };
            if !response.status().is_success() {
                return;
            }
            let Ok(invite_bytes) = response.bytes().await else {
                return;
            };
            if invite_bytes.len() > 256 * 1024 {
                return;
            }
            let Ok(invite) = serde_json::from_slice::<serde_json::Value>(&invite_bytes) else {
                return;
            };
            if invite.get("schema").and_then(serde_json::Value::as_str)
                != Some("rampage.enrollment-invite.v1")
                || invite.get("controller_mesh").is_none()
            {
                return;
            }
            let Ok(invite_bytes) = serde_json::to_vec_pretty(&invite) else {
                return;
            };
            if invite_bytes.len() > 256 * 1024 {
                return;
            }
            let invite_file = data_dir.join(format!(
                "owner-agent-invite-{}.json",
                &fresh_intelligence_token()[..16]
            ));
            let Ok(mut file) = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&invite_file)
            else {
                return;
            };
            if file
                .write_all(&invite_bytes)
                .and_then(|()| file.sync_all())
                .is_err()
            {
                let _ = std::fs::remove_file(&invite_file);
                return;
            }
            agent_args.extend([
                "--invite-file".into(),
                invite_file.to_string_lossy().into_owned(),
            ]);
        }
        agent_args.extend([
            "--display-name".into(),
            "This Device".into(),
            "--device-kind".into(),
            "desktop".into(),
            "--serve".into(),
        ]);
        let Ok(command) = handle.shell().sidecar("rampage-agent") else {
            return;
        };
        let Ok((_, agent)) = command
            .env("RAMPAGE_DATA_DIR", &data_dir)
            .args(agent_args)
            .spawn()
        else {
            return;
        };
        if let Ok(mut sidecars) = handle.state::<Sidecars>().0.lock() {
            sidecars.push(agent);
        }
    });
    Ok(())
}

fn schedule_remote_assist_indicator(
    app: &AppHandle,
    status_item: MenuItem<tauri::Wry>,
    role: &'static str,
) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(data_dir) = runtime_dir(&handle) else {
            return;
        };
        let mut was_active = false;
        loop {
            let active =
                remote_assist_enabled_at(&data_dir) && remote_assist_active_at(&data_dir).is_some();
            if active != was_active {
                let status = if active {
                    "REMOTE CONTROL ACTIVE — Emergency stop ends access"
                } else {
                    role
                };
                let _ = status_item.set_text(status);
                if let Some(tray) = handle.tray_by_id("rampage") {
                    let _ = tray.set_tooltip(Some(format!("Rampage — {status}")));
                }
                was_active = active;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}

fn install_desktop_lifecycle(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = runtime_dir(app.handle())?;
    let role = if setup_required(&data_dir) {
        "Setup required"
    } else if worker_enrollment_exists(&data_dir) {
        "Worker active"
    } else {
        "Owner fabric active"
    };
    let status = MenuItem::with_id(app, "fabric_status", role, false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Rampage", true, None::<&str>)?;
    let start_at_login = CheckMenuItem::with_id(
        app,
        "start_at_login",
        "Start with Windows",
        true,
        platform_autostart_enabled(app.handle()).unwrap_or(false),
        None::<&str>,
    )?;
    let emergency_stop = MenuItem::with_id(
        app,
        "emergency_stop",
        "Emergency stop sharing",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Rampage", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &open,
            &start_at_login,
            &emergency_stop,
            &separator,
            &quit,
        ],
    )?;
    TrayIconBuilder::with_id("rampage")
        .icon(
            app.default_window_icon()
                .ok_or("Rampage application icon is missing")?
                .clone(),
        )
        .tooltip(format!("Rampage — {role}"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "start_at_login" => {
                let enabled = platform_autostart_enabled(app).unwrap_or(false);
                let result = platform_set_autostart(app, !enabled);
                if result.is_err() {
                    show_main_window(app);
                }
            }
            "emergency_stop" => {
                let _ = local_stop(app.clone());
                show_main_window(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    schedule_remote_assist_indicator(app.handle(), status.clone(), role);

    if let Some(window) = app.get_webview_window("main")
        && launched_in_background(std::env::args())
    {
        let _ = window.hide();
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    if !diagnostic_instance_is_bounded() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if should_focus_existing_instance(&args) {
                show_main_window(app);
            }
        }));
    }
    let app = builder
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Rampage")
                .arg(BACKGROUND_ARG)
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .manage(Sidecars::default())
        .manage(WorkerRuntime::default())
        .manage(LocalAiRuntime::default())
        .manage(PairingManager::default())
        .setup(|app| {
            launch_fabric(app.handle()).map_err(std::io::Error::other)?;
            let setup_required = setup_required(&runtime_dir(app.handle())?);
            if !setup_required {
                local_ai::schedule(
                    app.state::<LocalAiRuntime>().inner().clone(),
                    diagnostic_instance_is_bounded(),
                );
            }
            install_desktop_lifecycle(app)?;
            schedule_diagnostic_exit(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            local_stop,
            fabric_mode,
            worker_runtime_status,
            local_ai_runtime_status,
            run_fabric_benchmark,
            controller_token,
            join_remote,
            open_pairing_window,
            pairing_window,
            begin_pairing,
            pairing_status,
            cancel_pairing,
            approve_pairing,
            reject_pairing,
            autostart_enabled,
            set_autostart,
            recovery_status,
            repair_connection,
            leave_fabric,
            factory_reset,
            activate_owner_fabric,
            confirm_owner_fabric,
            remote_assist_status,
            set_remote_assist_enabled
        ])
        .build(tauri::generate_context!())
        .expect("error while building Rampage");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit)
            && let Ok(mut sidecars) = handle.state::<Sidecars>().0.lock()
        {
            for child in sidecars.drain(..) {
                let _ = kill_process_tree(child);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_background_argument_hides_the_window() {
        assert!(launched_in_background(["rampage.exe", BACKGROUND_ARG]));
        assert!(!launched_in_background([
            "rampage.exe",
            "--background-worker"
        ]));
    }

    #[test]
    fn intelligence_uses_an_independent_high_entropy_token() {
        let first = fresh_intelligence_token();
        let second = fresh_intelligence_token();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn owner_relay_config_only_exports_credential_free_https() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rampage-relay.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema": "rampage.owner-relay-config.v1",
                "public_url": "https://relay.example.test"
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            configured_private_relay(temp.path()).unwrap().as_deref(),
            Some("https://relay.example.test")
        );
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "schema": "rampage.owner-relay-config.v1",
                "public_url": "https://user:secret@relay.example.test?token=leak"
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(configured_private_relay(temp.path()).is_err());
    }

    #[test]
    fn diagnostic_exit_is_explicit_and_bounded() {
        assert_eq!(
            diagnostic_exit_delay(Some("1000")),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            diagnostic_exit_delay(Some("180000")),
            Some(Duration::from_secs(180))
        );
        assert_eq!(diagnostic_exit_delay(Some("999")), None);
        assert_eq!(diagnostic_exit_delay(Some("not-a-number")), None);
        assert_eq!(diagnostic_exit_delay(None), None);
    }

    #[test]
    fn controller_override_remains_loopback_only() {
        let parsed = "127.0.0.1:49123".parse::<SocketAddr>().unwrap();
        assert!(parsed.ip().is_loopback());
        let remote = "192.0.2.1:49123".parse::<SocketAddr>().unwrap();
        assert!(!remote.ip().is_loopback());
    }

    #[test]
    fn second_instance_restores_only_interactive_launches() {
        assert!(should_focus_existing_instance(&[
            "rampage-desktop.exe".into()
        ]));
        assert!(!should_focus_existing_instance(&[
            "rampage-desktop.exe".into(),
            BACKGROUND_ARG.into()
        ]));
    }

    #[test]
    fn durable_owner_marker_prevents_local_mesh_pin_role_confusion() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("agent.controller-pin.json"), b"pinned").unwrap();
        assert!(worker_enrollment_exists(temp.path()));
        std::fs::write(
            temp.path().join(OWNER_FABRIC_MARKER),
            b"rampage.owner-fabric.v1\n",
        )
        .unwrap();
        assert!(!worker_enrollment_exists(temp.path()));
    }

    fn recovery_runtime() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("ai.obtuse.rampage/runtime")).unwrap();
        root
    }

    #[test]
    fn empty_runtime_boots_into_neutral_setup_instead_of_implicit_owner() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        std::fs::write(
            runtime.join("firewall-private-v2.ready"),
            b"installer-owned",
        )
        .unwrap();

        assert!(ensure_neutral_first_run(&runtime).unwrap());
        assert_eq!(fabric_role_at(&runtime), "setup");
        assert!(runtime.join(SETUP_REQUIRED_MARKER).is_file());
        assert!(!runtime.join(OWNER_FABRIC_MARKER).exists());
    }

    fn write_owner_local_enrollment(runtime: &Path, governor_secret: [u8; 32]) {
        let governor = ed25519_dalek::SigningKey::from_bytes(&governor_secret);
        std::fs::write(
            runtime.join(OWNER_FABRIC_MARKER),
            b"rampage.owner-fabric.v1\n",
        )
        .unwrap();
        std::fs::write(runtime.join("governor.key"), hex::encode(governor_secret)).unwrap();
        std::fs::write(runtime.join("agent.enrolled"), b"local-controller\n").unwrap();
        std::fs::write(runtime.join("agent.identity.json"), b"{}").unwrap();
        std::fs::write(runtime.join("agent.key"), b"agent-secret").unwrap();
        std::fs::write(
            runtime.join("agent.controller-pin.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": "rampage.pinned-controller.v1",
                "endpoint": { "endpoint_id": "local-controller" },
                "governor_public_key": hex::encode(governor.verifying_key().to_bytes())
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn recovery_accepts_owner_self_enrolled_local_agent() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        write_owner_local_enrollment(&runtime, [31_u8; 32]);
        let status = recovery_status_at(&runtime);
        assert_eq!(status.role, "owner");
        assert_eq!(status.state, "owner_active");
        assert!(status.healthy);
        assert!(!status.can_leave_fabric);
    }

    #[test]
    fn recovery_rejects_owner_agent_pinned_to_another_controller() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        write_owner_local_enrollment(&runtime, [31_u8; 32]);
        let foreign = ed25519_dalek::SigningKey::from_bytes(&[47_u8; 32]);
        std::fs::write(
            runtime.join("agent.controller-pin.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": "rampage.pinned-controller.v1",
                "endpoint": { "endpoint_id": "local-controller" },
                "governor_public_key": hex::encode(foreign.verifying_key().to_bytes())
            }))
            .unwrap(),
        )
        .unwrap();
        let status = recovery_status_at(&runtime);
        assert_eq!(status.role, "owner");
        assert_eq!(status.state, "repair_required");
        assert!(!status.healthy);
        assert!(status.issues[0].contains("different owner controller"));
    }

    #[test]
    fn recovery_detects_incomplete_worker_identity() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        std::fs::write(runtime.join("agent.controller-pin.json"), b"pinned").unwrap();
        let status = recovery_status_at(&runtime);
        assert_eq!(status.role, "worker");
        assert_eq!(status.state, "repair_required");
        assert!(!status.healthy);
        assert!(status.can_leave_fabric);
    }

    #[test]
    fn reset_rotates_to_clean_setup_without_following_unrelated_paths() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        std::fs::write(runtime.join("agent.key"), b"secret").unwrap();
        std::fs::write(runtime.join("agent.controller-pin.json"), b"pinned").unwrap();
        std::fs::create_dir(runtime.join("cas")).unwrap();
        std::fs::write(runtime.join("cas/object"), b"encrypted").unwrap();
        reset_runtime_to_setup(&runtime).unwrap();
        assert_eq!(
            std::fs::read_to_string(runtime.join(SETUP_REQUIRED_MARKER)).unwrap(),
            "rampage.setup-required.v1\n"
        );
        assert_eq!(std::fs::read_dir(&runtime).unwrap().count(), 1);
        let status = recovery_status_at(&runtime);
        assert_eq!(status.role, "setup");
        assert_eq!(status.state, "ready_to_configure");
        assert!(status.healthy);
    }

    #[test]
    fn setup_pairing_removes_only_stale_worker_credentials() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        std::fs::write(
            runtime.join(SETUP_REQUIRED_MARKER),
            b"rampage.setup-required.v1\n",
        )
        .unwrap();
        for name in [
            "remote-invite.json",
            "agent.controller-pin.json",
            "agent.enrolled",
            "agent.identity.json",
            "agent.key",
            REMOTE_ASSIST_POLICY_FILE,
            REMOTE_ASSIST_ACTIVE_FILE,
        ] {
            std::fs::write(runtime.join(name), b"stale").unwrap();
        }
        std::fs::write(runtime.join("pairing-diagnostic.keep"), b"preserved").unwrap();

        assert_eq!(
            remove_stale_worker_credentials_in_setup(&runtime).unwrap(),
            7
        );
        assert!(runtime.join(SETUP_REQUIRED_MARKER).is_file());
        assert!(runtime.join("pairing-diagnostic.keep").is_file());
        assert!(!runtime.join("agent.controller-pin.json").exists());
        assert!(!runtime.join("remote-invite.json").exists());
    }

    #[test]
    fn active_worker_credentials_are_never_self_deleted() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        std::fs::write(runtime.join("agent.controller-pin.json"), b"active").unwrap();
        assert_eq!(
            remove_stale_worker_credentials_in_setup(&runtime).unwrap(),
            0
        );
        assert!(runtime.join("agent.controller-pin.json").is_file());
    }

    #[test]
    fn failed_nearby_pairing_can_retry_the_same_protected_transaction() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");

        prepare_worker_pairing_files(&runtime).unwrap();
        prepare_worker_pairing_files(&runtime).unwrap();

        assert!(runtime.join(SETUP_REQUIRED_MARKER).is_file());
        assert_eq!(
            std::fs::read(runtime.join(WORKER_PAIRING_INTENT_MARKER)).unwrap(),
            b"rampage.worker-pairing.v1\n"
        );
    }

    #[test]
    fn live_worker_retry_errors_are_bounded_and_visible() {
        assert!(bounded_worker_retry_message(b"  \r\n").is_none());
        let message = bounded_worker_retry_message(&vec![b'x'; 2_048]).unwrap();
        let detail = message
            .strip_prefix("Automatic signed-route recovery is retrying. ")
            .unwrap();
        assert_eq!(detail.chars().count(), 384);
        assert!(message.chars().count() <= 512);
    }

    #[test]
    fn windows_installer_stops_every_tray_sidecar_before_replacement() {
        let hooks = include_str!("../windows/installer-hooks.nsh");
        assert!(hooks.contains("!macro NSIS_HOOK_PREINSTALL"));
        assert!(hooks.contains("!macro NSIS_HOOK_PREUNINSTALL"));
        for image in [
            "${MAINBINARYNAME}.exe",
            "rampage-controller.exe",
            "rampage-agent.exe",
            "rampage-relay.exe",
            "rampage-intelligence.exe",
            "rampage.exe",
        ] {
            assert!(
                hooks.contains(&format!("/F /T /IM \"{image}\"")),
                "installer omitted the exact {image} process boundary"
            );
        }
    }

    #[test]
    fn explicit_join_retires_unconfirmed_legacy_owner_and_commits_invite() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        write_owner_local_enrollment(&runtime, [31_u8; 32]);
        std::fs::write(runtime.join("controller.db"), b"legacy bootstrap").unwrap();
        #[cfg(target_os = "windows")]
        std::fs::write(
            runtime.join(FIREWALL_MARKER_FILE),
            b"rampage.firewall-private.v2\ninstall_dir=C:\\Rampage\n",
        )
        .unwrap();

        prepare_worker_pairing_files(&runtime).unwrap();
        assert_eq!(fabric_role_at(&runtime), "setup");
        assert!(runtime.join(WORKER_PAIRING_INTENT_MARKER).is_file());
        assert!(!runtime.join(OWNER_FABRIC_MARKER).exists());
        assert!(!runtime.join("agent.controller-pin.json").exists());
        #[cfg(target_os = "windows")]
        assert!(runtime.join(FIREWALL_MARKER_FILE).is_file());

        let invite = serde_json::json!({
            "schema": "rampage.enrollment-invite.v1",
            "controller_mesh": { "signature": "signed-mesh-endpoint" },
            "governor_public_key": "owner-key"
        })
        .to_string();
        persist_remote_invite_at(&runtime, &invite).unwrap();

        assert_eq!(fabric_role_at(&runtime), "worker");
        assert!(runtime.join("remote-invite.json").is_file());
        assert!(!runtime.join(SETUP_REQUIRED_MARKER).exists());
        assert!(!runtime.join(WORKER_PAIRING_INTENT_MARKER).exists());
    }

    #[test]
    fn explicit_join_never_erases_a_confirmed_owner() {
        let root = recovery_runtime();
        let runtime = root.path().join("ai.obtuse.rampage/runtime");
        write_owner_local_enrollment(&runtime, [31_u8; 32]);
        std::fs::write(
            runtime.join(OWNER_CONFIRMED_MARKER),
            b"rampage.owner-confirmed.v1\n",
        )
        .unwrap();

        let error = prepare_worker_pairing_files(&runtime).unwrap_err();
        assert!(error.contains("configured fabric"));
        assert!(runtime.join(OWNER_FABRIC_MARKER).is_file());
        assert!(runtime.join("agent.controller-pin.json").is_file());
        assert!(!runtime.join(SETUP_REQUIRED_MARKER).exists());
    }

    #[test]
    fn reset_refuses_broad_or_renamed_targets() {
        let root = tempfile::tempdir().unwrap();
        assert!(reset_runtime_to_setup(root.path()).is_err());
        let wrong = root.path().join("not-rampage/runtime");
        std::fs::create_dir_all(&wrong).unwrap();
        assert!(reset_runtime_to_setup(&wrong).is_err());
    }
}
