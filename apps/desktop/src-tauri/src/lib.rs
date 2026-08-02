mod pairing;

use rand::{TryRng as _, rngs::SysRng};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};
use sysinfo::{Pid, System};
use tauri::{
    AppHandle, Manager, State, WindowEvent,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use pairing::{PairingManager, PairingRequestView, PairingWindowView, WorkerPairingView};
#[cfg(not(target_os = "windows"))]
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_shell::{ShellExt, process::CommandChild};
#[cfg(target_os = "windows")]
use winreg::{
    RegKey, RegValue,
    enums::{HKEY_CURRENT_USER, RegType},
};

#[derive(Default)]
struct Sidecars(Mutex<Vec<CommandChild>>);

const BACKGROUND_ARG: &str = "--background";
const AUTOSTART_NAME: &str = "Rampage";
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

fn kill_process_tree(child: CommandChild) {
    // PyInstaller's one-file launcher supervises a child process. Terminate descendants first so
    // closing Rampage cannot strand an intelligence server or any future sidecar subprocess.
    let system = System::new_all();
    let mut descendants = Vec::new();
    descendant_pids(&system, Pid::from_u32(child.pid()), &mut descendants);
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
    tauri::async_runtime::spawn(async move {
        let _ = reqwest::Client::new()
            .post("http://127.0.0.1:47831/v1/stop")
            .header("x-rampage-token", token)
            .send()
            .await;
    });
}

#[tauri::command]
fn fabric_mode(app: AppHandle) -> Result<&'static str, String> {
    Ok(if runtime_dir(&app)?.join("remote-invite.json").is_file() {
        "worker"
    } else {
        "owner"
    })
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
    if fabric_mode(app)? != "owner" {
        return Err("only the owner PC can approve a new machine".into());
    }
    pairing::open_owner_window(&pairing, local_device_name()).await
}

#[tauri::command]
fn pairing_window(pairing: State<'_, PairingManager>) -> Result<PairingWindowView, String> {
    pairing::owner_window(&pairing)
}

#[tauri::command]
fn begin_pairing(
    app: AppHandle,
    pairing: State<'_, PairingManager>,
) -> Result<WorkerPairingView, String> {
    if fabric_mode(app.clone())? == "worker" {
        return Err("this machine is already enrolled as a worker".into());
    }
    pairing::begin_worker(app, &pairing, local_device_name())
}

#[tauri::command]
fn pairing_status(pairing: State<'_, PairingManager>) -> Result<WorkerPairingView, String> {
    pairing::worker_status(&pairing)
}

#[tauri::command]
fn cancel_pairing(pairing: State<'_, PairingManager>) -> Result<(), String> {
    pairing::cancel_worker(&pairing)
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
    let response = reqwest::Client::new()
        .post("http://127.0.0.1:47831/v1/enrollment/invites")
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
    persist_remote_invite(&app, &invitation)?;
    app.restart()
}

fn persist_remote_invite(app: &AppHandle, invitation: &str) -> Result<(), String> {
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
    let data_dir = runtime_dir(app)?;
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    let destination = data_dir.join("remote-invite.json");
    if destination.exists() {
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
    Ok(())
}

fn launch_remote_worker(app: &AppHandle, data_dir: &std::path::Path) -> Result<(), String> {
    let invite_file = data_dir.join("remote-invite.json");
    let key_file = data_dir.join("agent.key");
    let display_name = local_device_name();
    let (_, agent) = app
        .shell()
        .sidecar("rampage-agent")
        .map_err(|error| error.to_string())?
        .env("RAMPAGE_DATA_DIR", data_dir)
        .args([
            "--invite-file".into(),
            invite_file.to_string_lossy().into_owned(),
            "--key-file".into(),
            key_file.to_string_lossy().into_owned(),
            "--display-name".into(),
            display_name,
            "--device-kind".into(),
            "desktop".into(),
            "--serve".into(),
        ])
        .spawn()
        .map_err(|error| format!("could not start remote worker: {error}"))?;
    app.state::<Sidecars>()
        .0
        .lock()
        .map_err(|_| "sidecar state lock poisoned".to_string())?
        .push(agent);
    Ok(())
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
    if data_dir.join("remote-invite.json").is_file() {
        return launch_remote_worker(app, &data_dir);
    }
    let intelligence_dir = data_dir.join("intelligence");
    std::fs::create_dir_all(&intelligence_dir).map_err(|error| error.to_string())?;
    let private_relay = configured_private_relay(&data_dir)?;
    let mut controller_command = app
        .shell()
        .sidecar("rampage-controller")
        .map_err(|error| error.to_string())?
        .env("RAMPAGE_DATA_DIR", &data_dir);
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
    for _ in 0..100 {
        if token_path.is_file() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
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
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        let mut ready = false;
        for _ in 0..80 {
            if client
                .get("http://127.0.0.1:47831/health")
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
        let Ok(response) = client
            .post("http://127.0.0.1:47831/v1/enrollment/invites")
            .header("x-rampage-token", &token)
            .json(&serde_json::json!({}))
            .send()
            .await
        else {
            return;
        };
        let Ok(invite) = response.json::<serde_json::Value>().await else {
            return;
        };
        let Some(enrollment_code) = invite
            .get("enrollment_code")
            .and_then(|value| value.as_str())
        else {
            return;
        };
        let Ok(data_dir) = runtime_dir(&handle) else {
            return;
        };
        let key_file = data_dir.join("agent.key");
        let Ok(command) = handle.shell().sidecar("rampage-agent") else {
            return;
        };
        let Ok((_, agent)) = command
            .env("RAMPAGE_DATA_DIR", &data_dir)
            .args([
                "--controller".into(),
                "http://127.0.0.1:47831".into(),
                "--key-file".into(),
                key_file.to_string_lossy().into_owned(),
                "--enrollment-code".into(),
                enrollment_code.into(),
                "--display-name".into(),
                "This Device".into(),
                "--device-kind".into(),
                "desktop".into(),
                "--serve".into(),
            ])
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

fn install_desktop_lifecycle(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let role = if runtime_dir(app.handle())?
        .join("remote-invite.json")
        .is_file()
    {
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

    if let Some(window) = app.get_webview_window("main")
        && launched_in_background(std::env::args())
    {
        let _ = window.hide();
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if should_focus_existing_instance(&args) {
                show_main_window(app);
            }
        }))
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
        .manage(PairingManager::default())
        .setup(|app| {
            launch_fabric(app.handle()).map_err(std::io::Error::other)?;
            install_desktop_lifecycle(app)?;
            schedule_diagnostic_exit(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            local_stop,
            fabric_mode,
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
            set_autostart
        ])
        .build(tauri::generate_context!())
        .expect("error while building Rampage");
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit)
            && let Ok(mut sidecars) = handle.state::<Sidecars>().0.lock()
        {
            for child in sidecars.drain(..) {
                kill_process_tree(child);
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
    fn second_instance_restores_only_interactive_launches() {
        assert!(should_focus_existing_instance(&[
            "rampage-desktop.exe".into()
        ]));
        assert!(!should_focus_existing_instance(&[
            "rampage-desktop.exe".into(),
            BACKGROUND_ARG.into()
        ]));
    }
}
