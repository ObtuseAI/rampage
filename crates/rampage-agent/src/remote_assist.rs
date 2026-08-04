use chrono::Utc;
use rampage_protocol::{
    MAX_REMOTE_DESKTOP_FRAME_BYTES, RemoteDesktopActionV1, RemoteDesktopFrameV1,
    RemoteDesktopModeV1, RemoteDesktopRequestV1, RemoteDesktopResponseV1, RemoteInputEventV1,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use uuid::Uuid;

const POLICY_FILE: &str = "remote-assist-policy.json";
const ACTIVE_FILE: &str = "remote-assist-active.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteAssistPolicyV1 {
    schema: String,
    enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
struct RemoteAssistActiveV1<'a> {
    schema: &'static str,
    session_id: Uuid,
    mode: RemoteDesktopModeV1,
    controller_endpoint_id: &'a str,
    updated_at: chrono::DateTime<Utc>,
    expires_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct SessionState {
    lease_id: Uuid,
    last_input_sequence: u64,
}

#[derive(Default)]
pub struct SessionAuthority {
    sessions: Mutex<HashMap<Uuid, SessionState>>,
    frame_sequence: AtomicU64,
}

pub fn policy_path(data_dir: &Path) -> PathBuf {
    data_dir.join(POLICY_FILE)
}

pub fn active_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ACTIVE_FILE)
}

pub fn supported() -> bool {
    cfg!(target_os = "windows")
}

pub fn enabled(data_dir: &Path) -> bool {
    if !supported() {
        return false;
    }
    let Ok(bytes) = fs::read(policy_path(data_dir)) else {
        return false;
    };
    if bytes.len() > 4 * 1024 {
        return false;
    }
    serde_json::from_slice::<RemoteAssistPolicyV1>(&bytes)
        .is_ok_and(|policy| policy.schema == "rampage.remote-assist-policy.v1" && policy.enabled)
}

pub fn clear_active(data_dir: &Path) {
    match fs::remove_file(active_path(data_dir)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn write_active(data_dir: &Path, request: &RemoteDesktopRequestV1) -> anyhow::Result<()> {
    fs::create_dir_all(data_dir)?;
    let destination = active_path(data_dir);
    let temporary = destination.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let payload = serde_json::to_vec(&RemoteAssistActiveV1 {
        schema: "rampage.remote-assist-active.v1",
        session_id: request.lease.session_id,
        mode: request.lease.mode,
        controller_endpoint_id: &request.lease.controller_endpoint_id,
        updated_at: Utc::now(),
        expires_at: request.lease.expires_at,
    })?;
    let write_result = (|| -> anyhow::Result<()> {
        use std::io::Write as _;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&payload)?;
        file.sync_all()?;
        drop(file);
        if destination.exists() {
            let metadata = fs::symlink_metadata(&destination)?;
            anyhow::ensure!(
                !metadata.file_type().is_symlink() && metadata.is_file(),
                "Remote Assist activity path is not a regular non-symlink file"
            );
            fs::remove_file(&destination)?;
        }
        fs::rename(&temporary, &destination)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

pub fn handle_request(
    request: RemoteDesktopRequestV1,
    node_id: Uuid,
    controller_endpoint_id: &str,
    governor_public_key: &str,
    store: &rampage_storage::CasStore,
    data_dir: &Path,
    authority: &SessionAuthority,
) -> anyhow::Result<(RemoteDesktopResponseV1, Vec<u8>)> {
    anyhow::ensure!(
        !data_dir.join("KILL").is_file(),
        "local owner STOP is active"
    );
    anyhow::ensure!(
        enabled(data_dir),
        "Remote Assist is disabled on this worker"
    );
    rampage_policy::verify_remote_desktop_lease_with_key(governor_public_key, &request.lease)?;
    anyhow::ensure!(
        request
            .lease
            .is_active_at(Utc::now(), request.lease.fencing_epoch)
            && request.is_valid_for(node_id, controller_endpoint_id),
        "Remote Assist request is outside its signed authority"
    );

    {
        let mut sessions = authority
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Remote Assist session state lock poisoned"))?;
        let previous_sequence = sessions
            .get(&request.lease.session_id)
            .map_or(0, |state| state.last_input_sequence);
        let is_new_lease = sessions
            .get(&request.lease.session_id)
            .is_none_or(|state| state.lease_id != request.lease.lease_id);
        if is_new_lease {
            store.accept_authority(
                "governor.remote-assist",
                request.lease.fencing_epoch,
                &request.lease.nonce,
                request.lease.expires_at,
            )?;
            sessions.insert(
                request.lease.session_id,
                SessionState {
                    lease_id: request.lease.lease_id,
                    last_input_sequence: previous_sequence,
                },
            );
        }
    }

    let request_id = request.request_id;
    match request.action {
        RemoteDesktopActionV1::Frame => {
            let (width, height, payload) =
                platform::capture_jpeg(request.lease.max_width, request.lease.max_height)?;
            anyhow::ensure!(
                !payload.is_empty() && payload.len() as u64 <= MAX_REMOTE_DESKTOP_FRAME_BYTES,
                "captured desktop frame is outside the signed payload limit"
            );
            let sequence = authority.frame_sequence.fetch_add(1, Ordering::AcqRel) + 1;
            let frame = RemoteDesktopFrameV1 {
                sequence,
                captured_at: Utc::now(),
                width,
                height,
                media_type: "image/jpeg".into(),
                payload_size: payload.len() as u64,
                payload_digest: format!("sha256:{}", hex::encode(Sha256::digest(&payload))),
            };
            anyhow::ensure!(
                frame.is_valid_for(&request.lease),
                "captured frame is invalid"
            );
            write_active(data_dir, &request)?;
            Ok((success(request_id, Some(frame), 0), payload))
        }
        RemoteDesktopActionV1::Input => {
            anyhow::ensure!(
                request.lease.mode == RemoteDesktopModeV1::Control,
                "view-only Remote Assist session cannot inject input"
            );
            let sequence = request
                .input_sequence
                .ok_or_else(|| anyhow::anyhow!("input sequence is missing"))?;
            {
                let mut sessions = authority
                    .sessions
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Remote Assist session state lock poisoned"))?;
                let session = sessions
                    .get_mut(&request.lease.session_id)
                    .ok_or_else(|| anyhow::anyhow!("Remote Assist session is unavailable"))?;
                anyhow::ensure!(
                    sequence > session.last_input_sequence,
                    "Remote Assist input sequence was replayed or reordered"
                );
                platform::apply_input(&request.events)?;
                session.last_input_sequence = sequence;
            }
            write_active(data_dir, &request)?;
            Ok((
                success(request_id, None, request.events.len() as u16),
                Vec::new(),
            ))
        }
        RemoteDesktopActionV1::Heartbeat => {
            write_active(data_dir, &request)?;
            Ok((success(request_id, None, 0), Vec::new()))
        }
        RemoteDesktopActionV1::Close => {
            authority
                .sessions
                .lock()
                .map_err(|_| anyhow::anyhow!("Remote Assist session state lock poisoned"))?
                .remove(&request.lease.session_id);
            let active_matches_session = fs::read(active_path(data_dir))
                .ok()
                .filter(|bytes| bytes.len() <= 16 * 1024)
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| {
                    value
                        .get("session_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .is_some_and(|session_id| session_id == request.lease.session_id.to_string());
            if active_matches_session {
                clear_active(data_dir);
            }
            Ok((success(request_id, None, 0), Vec::new()))
        }
    }
}

fn success(
    request_id: Uuid,
    frame: Option<RemoteDesktopFrameV1>,
    applied_events: u16,
) -> RemoteDesktopResponseV1 {
    RemoteDesktopResponseV1 {
        schema: RemoteDesktopResponseV1::SCHEMA.into(),
        request_id,
        status: 200,
        frame,
        applied_events,
        error: None,
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
    use windows::Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CAPTUREBLT, CreateCompatibleBitmap,
            CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HALFTONE,
            HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject, SetStretchBltMode, StretchBlt,
        },
        System::StationsAndDesktops::{
            CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP, HDESK,
            OpenInputDesktop, SwitchDesktop,
        },
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
                MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
                MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
                MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
                MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY,
            },
            WindowsAndMessaging::{
                GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
                SM_YVIRTUALSCREEN,
            },
        },
    };

    struct DesktopGuard(HDESK);
    impl Drop for DesktopGuard {
        fn drop(&mut self) {
            let _ = unsafe { CloseDesktop(self.0) };
        }
    }

    struct ScreenDc(HDC);
    impl Drop for ScreenDc {
        fn drop(&mut self) {
            let _ = unsafe { ReleaseDC(None, self.0) };
        }
    }

    struct MemoryDc(HDC);
    impl Drop for MemoryDc {
        fn drop(&mut self) {
            let _ = unsafe { DeleteDC(self.0) };
        }
    }

    struct Bitmap(HBITMAP);
    impl Drop for Bitmap {
        fn drop(&mut self) {
            let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
        }
    }

    fn require_interactive_desktop() -> anyhow::Result<DesktopGuard> {
        let desktop = unsafe {
            OpenInputDesktop(
                Default::default(),
                false,
                DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0),
            )
        }
        .map_err(|_| anyhow::anyhow!("Windows secure or locked desktop is not available"))?;
        unsafe { SwitchDesktop(desktop) }
            .map_err(|_| anyhow::anyhow!("Windows secure or locked desktop is not available"))?;
        Ok(DesktopGuard(desktop))
    }

    pub fn capture_jpeg(max_width: u32, max_height: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        let _desktop = require_interactive_desktop()?;
        let source_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let source_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let source_width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let source_height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        anyhow::ensure!(
            source_width > 0 && source_height > 0,
            "interactive desktop dimensions are unavailable"
        );
        let scale = f64::min(
            max_width as f64 / source_width as f64,
            max_height as f64 / source_height as f64,
        )
        .min(1.0);
        let width = (source_width as f64 * scale).round().max(1.0) as i32;
        let height = (source_height as f64 * scale).round().max(1.0) as i32;

        let screen = ScreenDc(unsafe { GetDC(None) });
        anyhow::ensure!(
            screen.0 != HDC::default(),
            "could not open the desktop device context"
        );
        let memory = MemoryDc(unsafe { CreateCompatibleDC(Some(screen.0)) });
        anyhow::ensure!(
            memory.0 != HDC::default(),
            "could not create a capture device context"
        );
        let bitmap = Bitmap(unsafe { CreateCompatibleBitmap(screen.0, width, height) });
        anyhow::ensure!(
            bitmap.0 != HBITMAP::default(),
            "could not allocate the desktop capture bitmap"
        );
        let previous = unsafe { SelectObject(memory.0, HGDIOBJ(bitmap.0.0)) };
        anyhow::ensure!(
            previous != HGDIOBJ::default(),
            "could not select the desktop capture bitmap"
        );
        unsafe { SetStretchBltMode(memory.0, HALFTONE) };
        let copied = unsafe {
            StretchBlt(
                memory.0,
                0,
                0,
                width,
                height,
                Some(screen.0),
                source_x,
                source_y,
                source_width,
                source_height,
                SRCCOPY | CAPTUREBLT,
            )
        };
        if !copied.as_bool() {
            unsafe { SelectObject(memory.0, previous) };
            anyhow::bail!("Windows refused the desktop capture operation");
        }
        // GetDIBits requires the queried bitmap not to be selected into a device context.
        unsafe { SelectObject(memory.0, previous) };
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0_u8; width as usize * height as usize * 4];
        let lines = unsafe {
            GetDIBits(
                memory.0,
                bitmap.0,
                0,
                height as u32,
                Some(bgra.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        anyhow::ensure!(
            lines == height,
            "Windows returned an incomplete desktop frame"
        );

        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for pixel in bgra.chunks_exact(4) {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
        let mut jpeg = Vec::new();
        for quality in [72, 58, 44, 32] {
            jpeg.clear();
            JpegEncoder::new_with_quality(&mut jpeg, quality).encode(
                &rgb,
                width as u32,
                height as u32,
                ExtendedColorType::Rgb8,
            )?;
            if jpeg.len() as u64 <= MAX_REMOTE_DESKTOP_FRAME_BYTES {
                break;
            }
        }
        anyhow::ensure!(
            jpeg.len() as u64 <= MAX_REMOTE_DESKTOP_FRAME_BYTES,
            "desktop frame could not be encoded inside the signed payload limit"
        );
        Ok((width as u32, height as u32, jpeg))
    }

    pub fn apply_input(events: &[RemoteInputEventV1]) -> anyhow::Result<()> {
        let _desktop = require_interactive_desktop()?;
        let mut inputs = Vec::with_capacity(events.len());
        for event in events {
            let input = match event {
                RemoteInputEventV1::MouseMove { x, y } => INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: i32::from(*x),
                            dy: i32::from(*y),
                            dwFlags: MOUSEEVENTF_MOVE
                                | MOUSEEVENTF_ABSOLUTE
                                | MOUSEEVENTF_VIRTUALDESK,
                            ..Default::default()
                        },
                    },
                },
                RemoteInputEventV1::MouseButton { button, pressed } => {
                    let flags = match (button, pressed) {
                        (rampage_protocol::RemoteMouseButtonV1::Left, true) => MOUSEEVENTF_LEFTDOWN,
                        (rampage_protocol::RemoteMouseButtonV1::Left, false) => MOUSEEVENTF_LEFTUP,
                        (rampage_protocol::RemoteMouseButtonV1::Right, true) => {
                            MOUSEEVENTF_RIGHTDOWN
                        }
                        (rampage_protocol::RemoteMouseButtonV1::Right, false) => {
                            MOUSEEVENTF_RIGHTUP
                        }
                        (rampage_protocol::RemoteMouseButtonV1::Middle, true) => {
                            MOUSEEVENTF_MIDDLEDOWN
                        }
                        (rampage_protocol::RemoteMouseButtonV1::Middle, false) => {
                            MOUSEEVENTF_MIDDLEUP
                        }
                    };
                    INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dwFlags: flags,
                                ..Default::default()
                            },
                        },
                    }
                }
                RemoteInputEventV1::MouseWheel { delta } => INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            mouseData: i32::from(*delta) as u32,
                            dwFlags: MOUSEEVENTF_WHEEL,
                            ..Default::default()
                        },
                    },
                },
                RemoteInputEventV1::Key {
                    virtual_key,
                    pressed,
                } => INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(*virtual_key),
                            dwFlags: if *pressed {
                                Default::default()
                            } else {
                                KEYEVENTF_KEYUP
                            },
                            ..Default::default()
                        },
                    },
                },
            };
            inputs.push(input);
        }
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        anyhow::ensure!(
            sent as usize == inputs.len(),
            "Windows blocked Remote Assist input injection (UIPI or secure desktop)"
        );
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::*;

    pub fn capture_jpeg(_max_width: u32, _max_height: u32) -> anyhow::Result<(u32, u32, Vec<u8>)> {
        anyhow::bail!("Remote Assist capture is not qualified on this platform")
    }

    pub fn apply_input(_events: &[RemoteInputEventV1]) -> anyhow::Result<()> {
        anyhow::bail!("Remote Assist input is not qualified on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    use chrono::Duration;
    #[cfg(target_os = "windows")]
    use rampage_protocol::{
        AvailabilityV1, ExecutionPattern, MeshEndpointRecordV1, ResourceClass, ResourceOfferV1,
        WorkloadCapabilityStatus, WorkloadCapabilityV1, WorkloadDomain, WorkloadIsolation,
    };
    #[cfg(target_os = "windows")]
    use std::collections::BTreeSet;

    #[test]
    fn remote_assist_is_fail_closed_without_exact_policy() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!enabled(temp.path()));
        fs::write(policy_path(temp.path()), br#"{"enabled":true}"#).unwrap();
        assert!(!enabled(temp.path()));
        fs::write(
            policy_path(temp.path()),
            br#"{"schema":"rampage.remote-assist-policy.v1","enabled":true}"#,
        )
        .unwrap();
        assert_eq!(enabled(temp.path()), cfg!(target_os = "windows"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn signed_heartbeat_is_visible_and_durably_replay_fenced() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            policy_path(temp.path()),
            br#"{"schema":"rampage.remote-assist-policy.v1","enabled":true}"#,
        )
        .unwrap();
        let now = Utc::now();
        let node_id = Uuid::now_v7();
        let governor =
            rampage_policy::Governor::ephemeral(rampage_policy::GovernorConfig::default());
        let offer = ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id,
            observed_at: now,
            expires_at: now + Duration::minutes(1),
            resources: Vec::new(),
            availability: AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 80,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::from(["rampage.remote-assist.v1".into()]),
            workload_capabilities: vec![WorkloadCapabilityV1 {
                schema: WorkloadCapabilityV1::SCHEMA.into(),
                adapter: "rampage.remote-assist.v1".into(),
                domain: WorkloadDomain::EdgeUtility,
                operations: BTreeSet::from(["view".into(), "control".into()]),
                execution_patterns: BTreeSet::from([ExecutionPattern::StreamingService]),
                resource_classes: BTreeSet::from([
                    ResourceClass::CpuCompute,
                    ResourceClass::NetworkRelay,
                ]),
                isolation: WorkloadIsolation::DedicatedProcess,
                runtime_digest: "shipped-agent:remote-assist-v1".into(),
                checkpointable: false,
                preemptible: true,
                network_allowlist_required: false,
                status: WorkloadCapabilityStatus::Shipped,
                qualification_digest: None,
            }],
            model_runtimes: Vec::new(),
            link_benchmark: None,
            mesh_endpoint: Some(MeshEndpointRecordV1 {
                schema: MeshEndpointRecordV1::SCHEMA.into(),
                endpoint_id: "worker".into(),
                direct_addresses: vec!["127.0.0.1:1".into()],
                relay_urls: Vec::new(),
                issued_at: now,
                expires_at: now + Duration::minutes(1),
                signature: "signed".into(),
            }),
            signature: "signed".into(),
        };
        let controller = "controller";
        let lease = governor
            .authorize_remote_desktop_at_epoch(
                &offer,
                controller,
                Uuid::now_v7(),
                RemoteDesktopModeV1::Control,
                rampage_policy::RemoteDesktopLimits {
                    max_width: 1920,
                    max_height: 1080,
                    max_fps: 10,
                },
                6,
            )
            .unwrap();
        let request = RemoteDesktopRequestV1 {
            schema: RemoteDesktopRequestV1::SCHEMA.into(),
            request_id: Uuid::now_v7(),
            lease,
            action: RemoteDesktopActionV1::Heartbeat,
            input_sequence: None,
            events: Vec::new(),
        };
        let store = rampage_storage::CasStore::open(temp.path().join("cas"), [9_u8; 32]).unwrap();
        let authority = SessionAuthority::default();
        let governor_key = hex::encode(governor.verifying_key().to_bytes());
        handle_request(
            request.clone(),
            node_id,
            controller,
            &governor_key,
            &store,
            temp.path(),
            &authority,
        )
        .unwrap();
        assert!(active_path(temp.path()).is_file());
        assert!(
            handle_request(
                request,
                node_id,
                controller,
                &governor_key,
                &store,
                temp.path(),
                &SessionAuthority::default(),
            )
            .is_err()
        );
    }
}
