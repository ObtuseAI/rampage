use std::{path::PathBuf, sync::Mutex, time::Duration};
use sysinfo::{Pid, System};
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::{ShellExt, process::CommandChild};

#[derive(Default)]
struct Sidecars(Mutex<Vec<CommandChild>>);

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
    std::fs::write(data_dir.join("KILL"), b"owner-stop-v1\n").map_err(|error| error.to_string())
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
fn controller_token(app: AppHandle) -> Result<String, String> {
    std::fs::read_to_string(runtime_dir(&app)?.join("controller.token"))
        .map(|value| value.trim().to_string())
        .map_err(|error| format!("controller token unavailable: {error}"))
}

#[tauri::command]
fn join_remote(app: AppHandle, invitation: String) -> Result<(), String> {
    let parsed: serde_json::Value = serde_json::from_str(&invitation)
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
    let data_dir = runtime_dir(&app)?;
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    let destination = data_dir.join("remote-invite.json");
    let temporary = data_dir.join("remote-invite.tmp");
    std::fs::write(&temporary, invitation).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
    app.restart()
}

fn launch_remote_worker(app: &AppHandle, data_dir: &std::path::Path) -> Result<(), String> {
    let invite_file = data_dir.join("remote-invite.json");
    let key_file = data_dir.join("agent.key");
    let display_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Rampage Worker".into());
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

fn launch_fabric(app: &AppHandle) -> Result<(), String> {
    let data_dir = runtime_dir(app)?;
    std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
    if data_dir.join("remote-invite.json").is_file() {
        return launch_remote_worker(app, &data_dir);
    }
    let intelligence_dir = data_dir.join("intelligence");
    std::fs::create_dir_all(&intelligence_dir).map_err(|error| error.to_string())?;
    let (_, controller) = app
        .shell()
        .sidecar("rampage-controller")
        .map_err(|error| error.to_string())?
        .env("RAMPAGE_DATA_DIR", &data_dir)
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
    let token = std::fs::read_to_string(&token_path)
        .map_err(|error| format!("controller token unavailable: {error}"))?;
    let (_, intelligence) = app
        .shell()
        .sidecar("rampage-intelligence")
        .map_err(|error| error.to_string())?
        .env("RAMPAGE_DATA_DIR", &intelligence_dir)
        .env("RAMPAGE_ENABLE_MODELS", "false")
        .env("RAMPAGE_TOKEN", token.trim())
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
            .header("x-rampage-token", token.trim())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(Sidecars::default())
        .setup(|app| {
            launch_fabric(app.handle()).map_err(std::io::Error::other)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            local_stop,
            fabric_mode,
            controller_token,
            join_remote
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
