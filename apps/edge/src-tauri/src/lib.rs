use rampage_edge::{EdgeSessionSnapshot, EdgeTelemetry, EdgeWorker};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_rampage_edge::{EdgeDeviceStatus, RampageEdgeExt};

#[derive(Clone, Default)]
struct EdgeRuntime(Arc<Mutex<Option<EdgeWorker>>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EdgeView {
    native: EdgeDeviceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<EdgeSessionSnapshot>,
    message: String,
}

fn telemetry(status: &EdgeDeviceStatus) -> EdgeTelemetry {
    EdgeTelemetry {
        platform: status.platform.clone(),
        device_kind: status.device_kind.clone(),
        foreground: status.foreground,
        donation_requested: status.donation_requested,
        battery_percent: status.battery_percent,
        on_external_power: status.on_external_power,
        low_power_mode: status.low_power_mode,
        thermal_headroom_percent: status.thermal_headroom_percent,
    }
}

fn runtime_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("edge"))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn edge_status(app: AppHandle, state: State<'_, EdgeRuntime>) -> Result<EdgeView, String> {
    let native = app
        .rampage_edge()
        .status()
        .map_err(|error| error.to_string())?;
    let session = state
        .0
        .lock()
        .map_err(|_| "edge runtime lock is poisoned".to_string())?
        .as_ref()
        .map(|worker| worker.snapshot(telemetry(&native).eligible()));
    Ok(EdgeView {
        native,
        session,
        message: "Foreground donation is owner initiated and expires automatically.".into(),
    })
}

#[tauri::command]
async fn edge_start(
    app: AppHandle,
    state: State<'_, EdgeRuntime>,
    invitation: Option<String>,
    display_name: String,
) -> Result<EdgeView, String> {
    let native = app
        .rampage_edge()
        .set_donation(true)
        .map_err(|error| error.to_string())?;
    let native_telemetry = telemetry(&native);
    let data_dir = runtime_dir(&app)?;
    let runtime = state.0.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || -> Result<EdgeSessionSnapshot, String> {
            let mut guard = runtime
                .lock()
                .map_err(|_| "edge runtime lock is poisoned".to_string())?;
            if guard.is_none() {
                *guard = Some(
                    EdgeWorker::open(
                        data_dir,
                        invitation.as_deref(),
                        &display_name,
                        &native_telemetry,
                    )
                    .map_err(|error| error.to_string())?,
                );
            }
            guard
                .as_mut()
                .expect("initialized edge worker")
                .pulse(&native_telemetry)
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string());
    match result {
        Ok(Ok(session)) => Ok(EdgeView {
            native,
            session: Some(session),
            message: "Signed offer refreshed; waiting only for admitted edge work.".into(),
        }),
        Ok(Err(error)) | Err(error) => {
            let _ = app.rampage_edge().set_donation(false);
            Err(error)
        }
    }
}

#[tauri::command]
async fn edge_pulse(app: AppHandle, state: State<'_, EdgeRuntime>) -> Result<EdgeView, String> {
    let native = app
        .rampage_edge()
        .status()
        .map_err(|error| error.to_string())?;
    let native_telemetry = telemetry(&native);
    if !native_telemetry.eligible() {
        let _ = app.rampage_edge().set_donation(false);
        return Err(
            "native foreground, battery, low-power, or thermal policy paused contribution".into(),
        );
    }
    let runtime = state.0.clone();
    let session = tauri::async_runtime::spawn_blocking(move || {
        runtime
            .lock()
            .map_err(|_| "edge runtime lock is poisoned".to_string())?
            .as_mut()
            .ok_or_else(|| "edge session is not started".to_string())?
            .pulse(&native_telemetry)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(EdgeView {
        native,
        session: Some(session),
        message: "Foreground lease pulse is healthy.".into(),
    })
}

#[tauri::command]
async fn edge_stop(app: AppHandle, state: State<'_, EdgeRuntime>) -> Result<EdgeView, String> {
    let native = app
        .rampage_edge()
        .set_donation(false)
        .map_err(|error| error.to_string())?;
    let runtime = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(worker) = runtime
            .lock()
            .map_err(|_| "edge runtime lock is poisoned".to_string())?
            .take()
        {
            worker.shutdown();
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(EdgeView {
        native,
        session: None,
        message: "Donation stopped locally; the last short offer will expire without refresh."
            .into(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_rampage_edge::init())
        .manage(EdgeRuntime::default())
        .invoke_handler(tauri::generate_handler![
            edge_status,
            edge_start,
            edge_pulse,
            edge_stop
        ])
        .run(tauri::generate_context!())
        .expect("Rampage Edge runtime failed");
}
