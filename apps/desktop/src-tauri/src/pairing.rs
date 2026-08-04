use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use hkdf::Hkdf;
use if_addrs::{IfAddr, Ifv4Addr};
use rand::{TryRng as _, rngs::SysRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket as StdUdpSocket},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::AppHandle;
use tokio::net::UdpSocket;
use x25519_dalek::{PublicKey, StaticSecret};

const PAIRING_SCHEMA: &str = "rampage.lan-pairing.v1";
const PAIRING_PORT: u16 = 47_839;
const PAIRING_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 73, 82);
const PAIRING_WINDOW_MS: u64 = 3 * 60 * 1_000;
const WORKER_WAIT_MS: u64 = 5 * 60 * 1_000;
const MAX_DATAGRAM_BYTES: usize = 8 * 1_024;
const MAX_INVITATION_BYTES: usize = 5 * 1_024;
const MAX_PENDING_REQUESTS: usize = 16;
const MAX_NEW_REQUESTS_PER_IP_PER_MINUTE: usize = 5;

#[derive(Clone, Default)]
pub(crate) struct PairingManager {
    inner: Arc<Mutex<PairingInner>>,
}

#[derive(Default)]
struct PairingInner {
    owner_socket: Option<Arc<UdpSocket>>,
    owner_name: String,
    owner_open_until_ms: u64,
    pending: HashMap<String, PendingPair>,
    attempts: HashMap<IpAddr, VecDeque<u64>>,
    worker_generation: u64,
    worker: WorkerPairingView,
}

#[derive(Debug, Clone)]
struct PendingPair {
    request_id: String,
    device_name: String,
    device_kind: String,
    peer_addr: SocketAddr,
    peer_public_key: String,
    verification_code: String,
    key: [u8; 32],
    aad: Vec<u8>,
    expires_at_ms: u64,
    challenge_payload: Vec<u8>,
    approval_payload: Option<Vec<u8>>,
    state: PairingRequestState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairingRequestState {
    AwaitingApproval,
    Approved,
    Completed,
}

impl PairingRequestState {
    fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingApproval => "awaiting_approval",
            Self::Approved => "approved",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PairingRequestView {
    pub request_id: String,
    pub device_name: String,
    pub device_kind: String,
    pub verification_code: String,
    pub expires_at_ms: u64,
    pub state: String,
}

impl From<&PendingPair> for PairingRequestView {
    fn from(value: &PendingPair) -> Self {
        Self {
            request_id: value.request_id.clone(),
            device_name: value.device_name.clone(),
            device_kind: value.device_kind.clone(),
            verification_code: value.verification_code.clone(),
            expires_at_ms: value.expires_at_ms,
            state: value.state.as_str().into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PairingWindowView {
    pub schema: &'static str,
    pub open: bool,
    pub open_until_ms: u64,
    pub requests: Vec<PairingRequestView>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum WorkerPairingView {
    #[default]
    Idle,
    Searching {
        request_id: String,
        expires_at_ms: u64,
    },
    WaitingApproval {
        request_id: String,
        owner_name: String,
        verification_code: String,
        expires_at_ms: u64,
    },
    Approved {
        request_id: String,
        owner_name: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PairingDatagram {
    Hello {
        schema: String,
        request_id: String,
        device_name: String,
        device_kind: String,
        ephemeral_public_key: String,
        issued_at_ms: u64,
        expires_at_ms: u64,
    },
    Challenge {
        schema: String,
        request_id: String,
        owner_name: String,
        ephemeral_public_key: String,
        expires_at_ms: u64,
    },
    Approval {
        schema: String,
        request_id: String,
        nonce: String,
        ciphertext: String,
    },
    Rejected {
        schema: String,
        request_id: String,
        reason: String,
    },
    EnrollmentComplete {
        schema: String,
        request_id: String,
        nonce: String,
        ciphertext: String,
    },
}

pub(crate) async fn open_owner_window(
    manager: &PairingManager,
    owner_name: String,
) -> Result<PairingWindowView, String> {
    let owner_name = bounded_label(&owner_name, "owner name")?;
    let existing_socket = manager
        .inner
        .lock()
        .map_err(|_| "pairing state lock poisoned".to_string())?
        .owner_socket
        .clone();
    if existing_socket.is_none() {
        let socket = bind_owner_socket()?;
        let mut inner = manager
            .inner
            .lock()
            .map_err(|_| "pairing state lock poisoned".to_string())?;
        if inner.owner_socket.is_none() {
            inner.owner_socket = Some(socket.clone());
            let background_manager = manager.clone();
            tauri::async_runtime::spawn(async move {
                owner_receive_loop(background_manager, socket).await;
            });
        }
    }
    let now = now_ms();
    {
        let mut inner = manager
            .inner
            .lock()
            .map_err(|_| "pairing state lock poisoned".to_string())?;
        inner.owner_name = owner_name;
        inner.owner_open_until_ms = now.saturating_add(PAIRING_WINDOW_MS);
        prune_owner_state(&mut inner, now);
    }
    owner_window(manager)
}

pub(crate) fn owner_window(manager: &PairingManager) -> Result<PairingWindowView, String> {
    let now = now_ms();
    let mut inner = manager
        .inner
        .lock()
        .map_err(|_| "pairing state lock poisoned".to_string())?;
    prune_owner_state(&mut inner, now);
    let mut requests = inner
        .pending
        .values()
        .map(PairingRequestView::from)
        .collect::<Vec<_>>();
    requests.sort_by(|left, right| left.request_id.cmp(&right.request_id));
    Ok(PairingWindowView {
        schema: "rampage.pairing-window.v1",
        open: inner.owner_open_until_ms > now,
        open_until_ms: inner.owner_open_until_ms,
        requests,
    })
}

pub(crate) async fn approve(
    manager: &PairingManager,
    request_id: &str,
    invitation: &str,
) -> Result<PairingRequestView, String> {
    let (socket, peer_addr, payload, view) = {
        let mut inner = manager
            .inner
            .lock()
            .map_err(|_| "pairing state lock poisoned".to_string())?;
        prune_owner_state(&mut inner, now_ms());
        let socket = inner
            .owner_socket
            .clone()
            .ok_or_else(|| "pairing window is not open".to_string())?;
        let pending = inner
            .pending
            .get_mut(request_id)
            .ok_or_else(|| "pairing request expired or is unknown".to_string())?;
        let payload = encrypted_approval(pending, invitation)?;
        pending.approval_payload = Some(payload.clone());
        pending.state = PairingRequestState::Approved;
        (
            socket,
            pending.peer_addr,
            payload,
            PairingRequestView::from(&*pending),
        )
    };
    socket
        .send_to(&payload, peer_addr)
        .await
        .map_err(|error| format!("could not deliver pairing approval: {error}"))?;
    Ok(view)
}

pub(crate) async fn reject(manager: &PairingManager, request_id: &str) -> Result<(), String> {
    let (socket, peer_addr, payload) = {
        let mut inner = manager
            .inner
            .lock()
            .map_err(|_| "pairing state lock poisoned".to_string())?;
        let socket = inner
            .owner_socket
            .clone()
            .ok_or_else(|| "pairing window is not open".to_string())?;
        let pending = inner
            .pending
            .remove(request_id)
            .ok_or_else(|| "pairing request expired or is unknown".to_string())?;
        let payload = serde_json::to_vec(&PairingDatagram::Rejected {
            schema: PAIRING_SCHEMA.into(),
            request_id: request_id.into(),
            reason: "The owner declined this device.".into(),
        })
        .map_err(|error| error.to_string())?;
        (socket, pending.peer_addr, payload)
    };
    socket
        .send_to(&payload, peer_addr)
        .await
        .map_err(|error| format!("could not deliver pairing rejection: {error}"))?;
    Ok(())
}

pub(crate) fn worker_status(manager: &PairingManager) -> Result<WorkerPairingView, String> {
    manager
        .inner
        .lock()
        .map(|inner| inner.worker.clone())
        .map_err(|_| "pairing state lock poisoned".to_string())
}

pub(crate) fn cancel_worker(manager: &PairingManager) -> Result<(), String> {
    let mut inner = manager
        .inner
        .lock()
        .map_err(|_| "pairing state lock poisoned".to_string())?;
    inner.worker_generation = inner.worker_generation.saturating_add(1);
    inner.worker = WorkerPairingView::Idle;
    Ok(())
}

pub(crate) fn begin_worker(
    app: AppHandle,
    manager: &PairingManager,
    device_name: String,
) -> Result<WorkerPairingView, String> {
    let device_name = bounded_label(&device_name, "device name")?;
    let generation = {
        let mut inner = manager
            .inner
            .lock()
            .map_err(|_| "pairing state lock poisoned".to_string())?;
        if matches!(
            inner.worker,
            WorkerPairingView::Searching { .. } | WorkerPairingView::WaitingApproval { .. }
        ) {
            return Ok(inner.worker.clone());
        }
        inner.worker_generation = inner.worker_generation.saturating_add(1);
        inner.worker_generation
    };
    let request_id = fresh_hex::<16>()?;
    let expires_at_ms = now_ms().saturating_add(WORKER_WAIT_MS);
    set_worker_status(
        manager,
        generation,
        WorkerPairingView::Searching {
            request_id: request_id.clone(),
            expires_at_ms,
        },
    )?;
    let background_manager = manager.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = worker_pairing_loop(
            app,
            background_manager.clone(),
            generation,
            request_id,
            device_name,
            expires_at_ms,
        )
        .await
        {
            let _ = set_worker_status(
                &background_manager,
                generation,
                WorkerPairingView::Failed { message: error },
            );
        }
    });
    worker_status(manager)
}

async fn owner_receive_loop(manager: PairingManager, socket: Arc<UdpSocket>) {
    let mut buffer = vec![0_u8; MAX_DATAGRAM_BYTES + 1];
    loop {
        let Ok((length, source)) = socket.recv_from(&mut buffer).await else {
            continue;
        };
        if length == 0 || length > MAX_DATAGRAM_BYTES {
            continue;
        }
        let Ok(message) = serde_json::from_slice::<PairingDatagram>(&buffer[..length]) else {
            continue;
        };
        let now = now_ms();
        if let PairingDatagram::EnrollmentComplete {
            schema,
            request_id,
            nonce,
            ciphertext,
        } = &message
        {
            if schema != PAIRING_SCHEMA || !valid_request_id(request_id) {
                continue;
            }
            let Ok(mut inner) = manager.inner.lock() else {
                continue;
            };
            prune_owner_state(&mut inner, now);
            let Some(pending) = inner.pending.get_mut(request_id) else {
                continue;
            };
            if pending.peer_addr != source || pending.state != PairingRequestState::Approved {
                continue;
            }
            if decrypt_payload(&pending.key, &pending.aad, nonce, ciphertext)
                .is_ok_and(|payload| payload == b"enrollment-complete")
            {
                pending.state = PairingRequestState::Completed;
            }
            continue;
        }
        let PairingDatagram::Hello {
            schema,
            request_id,
            device_name,
            device_kind,
            ephemeral_public_key,
            issued_at_ms: _,
            expires_at_ms: _,
        } = message
        else {
            continue;
        };
        if schema != PAIRING_SCHEMA
            || !valid_request_id(&request_id)
            || bounded_label(&device_name, "device name").is_err()
            || device_kind != "desktop"
        {
            continue;
        }
        let Ok(peer_public_bytes) = decode_32(&ephemeral_public_key) else {
            continue;
        };
        let payload = {
            let Ok(mut inner) = manager.inner.lock() else {
                continue;
            };
            prune_owner_state(&mut inner, now);
            if inner.owner_open_until_ms <= now {
                continue;
            }
            if let Some(existing) = inner.pending.get_mut(&request_id) {
                if existing.peer_public_key != ephemeral_public_key
                    || existing.peer_addr.ip() != source.ip()
                {
                    continue;
                }
                existing.peer_addr = source;
                existing
                    .approval_payload
                    .clone()
                    .unwrap_or_else(|| existing.challenge_payload.clone())
            } else {
                if inner.pending.len() >= MAX_PENDING_REQUESTS
                    || !admit_source_attempt(&mut inner.attempts, source.ip(), now)
                {
                    continue;
                }
                let Ok(owner_secret_bytes) = fresh_bytes::<32>() else {
                    continue;
                };
                let owner_secret = StaticSecret::from(owner_secret_bytes);
                let owner_public = PublicKey::from(&owner_secret).to_bytes();
                let shared = owner_secret
                    .diffie_hellman(&PublicKey::from(peer_public_bytes))
                    .to_bytes();
                let transcript = pairing_transcript(&request_id, &peer_public_bytes, &owner_public);
                let Ok((key, verification_code, aad)) = derive_material(&shared, &transcript)
                else {
                    continue;
                };
                // Remote wall clocks are not a trust boundary. Keep the request bounded by the
                // owner's local pairing window so clock drift cannot silently break discovery or
                // let a peer extend enrollment availability.
                let effective_expiry = now
                    .saturating_add(WORKER_WAIT_MS)
                    .min(inner.owner_open_until_ms);
                let challenge = PairingDatagram::Challenge {
                    schema: PAIRING_SCHEMA.into(),
                    request_id: request_id.clone(),
                    owner_name: inner.owner_name.clone(),
                    ephemeral_public_key: BASE64.encode(owner_public),
                    expires_at_ms: effective_expiry,
                };
                let Ok(challenge_payload) = serde_json::to_vec(&challenge) else {
                    continue;
                };
                inner.pending.insert(
                    request_id.clone(),
                    PendingPair {
                        request_id,
                        device_name,
                        device_kind,
                        peer_addr: source,
                        peer_public_key: ephemeral_public_key,
                        verification_code,
                        key,
                        aad,
                        expires_at_ms: effective_expiry,
                        challenge_payload: challenge_payload.clone(),
                        approval_payload: None,
                        state: PairingRequestState::AwaitingApproval,
                    },
                );
                challenge_payload
            }
        };
        let _ = socket.send_to(&payload, source).await;
    }
}

async fn worker_pairing_loop(
    app: AppHandle,
    manager: PairingManager,
    generation: u64,
    request_id: String,
    device_name: String,
    expires_at_ms: u64,
) -> Result<(), String> {
    let socket = bind_worker_socket()?;
    let secret = StaticSecret::from(fresh_bytes::<32>()?);
    let public = PublicKey::from(&secret).to_bytes();
    let hello = serde_json::to_vec(&PairingDatagram::Hello {
        schema: PAIRING_SCHEMA.into(),
        request_id: request_id.clone(),
        device_name,
        device_kind: "desktop".into(),
        ephemeral_public_key: BASE64.encode(public),
        issued_at_ms: now_ms(),
        expires_at_ms,
    })
    .map_err(|error| error.to_string())?;
    let destinations = pairing_destinations();
    let mut interval = tokio::time::interval(Duration::from_millis(750));
    let mut buffer = vec![0_u8; MAX_DATAGRAM_BYTES + 1];
    let mut selected_owner: Option<(SocketAddr, [u8; 32], Vec<u8>, String)> = None;
    loop {
        if !generation_is_current(&manager, generation)? {
            return Ok(());
        }
        if now_ms() >= expires_at_ms {
            return Err(
                "No owner approved this laptop within five minutes. Try again beside the owner PC."
                    .into(),
            );
        }
        tokio::select! {
            _ = interval.tick() => {
                for destination in &destinations {
                    let _ = socket.send_to(&hello, destination).await;
                }
            }
            received = socket.recv_from(&mut buffer) => {
                let Ok((length, source)) = received else { continue; };
                if length == 0 || length > MAX_DATAGRAM_BYTES { continue; }
                let Ok(message) = serde_json::from_slice::<PairingDatagram>(&buffer[..length]) else { continue; };
                match message {
                    PairingDatagram::Challenge { schema, request_id: response_id, owner_name, ephemeral_public_key, expires_at_ms: _ }
                        if schema == PAIRING_SCHEMA && response_id == request_id => {
                        let Ok(owner_public) = decode_32(&ephemeral_public_key) else { continue; };
                        if let Some((selected_addr, _, _, _)) = &selected_owner
                            && (*selected_addr != source)
                        {
                            continue;
                        }
                        let shared = secret.diffie_hellman(&PublicKey::from(owner_public)).to_bytes();
                        let transcript = pairing_transcript(&request_id, &public, &owner_public);
                        let (key, verification_code, aad) = derive_material(&shared, &transcript)?;
                        let owner_name = bounded_label(&owner_name, "owner name")?;
                        selected_owner = Some((source, key, aad, owner_name.clone()));
                        set_worker_status(&manager, generation, WorkerPairingView::WaitingApproval {
                            request_id: request_id.clone(),
                            owner_name,
                            verification_code,
                            // Present a countdown based on this device's monotonic pairing
                            // lifetime rather than assuming both Windows clocks agree.
                            expires_at_ms,
                        })?;
                    }
                    PairingDatagram::Approval { schema, request_id: response_id, nonce, ciphertext }
                        if schema == PAIRING_SCHEMA && response_id == request_id => {
                        let Some((owner_addr, key, aad, owner_name)) = &selected_owner else { continue; };
                        if *owner_addr != source { continue; }
                        let invitation = decrypt_approval(key, aad, &nonce, &ciphertext)?;
                        super::persist_remote_invite(&app, &invitation)?;
                        let completion = encrypted_completion(&request_id, key, aad)?;
                        for _ in 0..3 {
                            socket.send_to(&completion, source).await
                                .map_err(|error| format!("could not confirm secure enrollment: {error}"))?;
                            tokio::time::sleep(Duration::from_millis(80)).await;
                        }
                        set_worker_status(&manager, generation, WorkerPairingView::Approved {
                            request_id: request_id.clone(),
                            owner_name: owner_name.clone(),
                        })?;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        app.restart();
                    }
                    PairingDatagram::Rejected { schema, request_id: response_id, reason }
                        if schema == PAIRING_SCHEMA && response_id == request_id => {
                        let Some((owner_addr, _, _, _)) = &selected_owner else { continue; };
                        if *owner_addr != source { continue; }
                        return Err(reason);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn encrypted_approval(pending: &PendingPair, invitation: &str) -> Result<Vec<u8>, String> {
    if invitation.len() > MAX_INVITATION_BYTES {
        return Err("invite exceeds the pairing payload limit".into());
    }
    let cipher = Aes256Gcm::new_from_slice(&pending.key)
        .map_err(|_| "could not initialize pairing encryption".to_string())?;
    let nonce = fresh_bytes::<12>()?;
    let ciphertext = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: invitation.as_bytes(),
                aad: &pending.aad,
            },
        )
        .map_err(|_| "could not encrypt the enrollment invitation".to_string())?;
    let payload = serde_json::to_vec(&PairingDatagram::Approval {
        schema: PAIRING_SCHEMA.into(),
        request_id: pending.request_id.clone(),
        nonce: BASE64.encode(nonce),
        ciphertext: BASE64.encode(ciphertext),
    })
    .map_err(|error| error.to_string())?;
    if payload.len() > MAX_DATAGRAM_BYTES {
        return Err("encrypted invite exceeds the pairing datagram limit".into());
    }
    Ok(payload)
}

fn decrypt_approval(
    key: &[u8; 32],
    aad: &[u8],
    nonce: &str,
    ciphertext: &str,
) -> Result<String, String> {
    let plaintext = decrypt_payload(key, aad, nonce, ciphertext)?;
    String::from_utf8(plaintext).map_err(|_| "pairing approval is not UTF-8".to_string())
}

fn encrypted_completion(request_id: &str, key: &[u8; 32], aad: &[u8]) -> Result<Vec<u8>, String> {
    let (nonce, ciphertext) = encrypt_payload(key, aad, b"enrollment-complete")?;
    serde_json::to_vec(&PairingDatagram::EnrollmentComplete {
        schema: PAIRING_SCHEMA.into(),
        request_id: request_id.into(),
        nonce,
        ciphertext,
    })
    .map_err(|error| error.to_string())
}

fn encrypt_payload(
    key: &[u8; 32],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<(String, String), String> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "could not initialize pairing encryption".to_string())?;
    let nonce = fresh_bytes::<12>()?;
    let ciphertext = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| "could not encrypt pairing payload".to_string())?;
    Ok((BASE64.encode(nonce), BASE64.encode(ciphertext)))
}

fn decrypt_payload(
    key: &[u8; 32],
    aad: &[u8],
    nonce: &str,
    ciphertext: &str,
) -> Result<Vec<u8>, String> {
    let nonce = BASE64
        .decode(nonce)
        .map_err(|_| "pairing approval nonce is invalid".to_string())?;
    let nonce: [u8; 12] = nonce
        .try_into()
        .map_err(|_| "pairing approval nonce has the wrong size".to_string())?;
    let ciphertext = BASE64
        .decode(ciphertext)
        .map_err(|_| "pairing approval ciphertext is invalid".to_string())?;
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "could not initialize pairing decryption".to_string())?;
    cipher
        .decrypt(
            (&nonce).into(),
            Payload {
                msg: &ciphertext,
                aad,
            },
        )
        .map_err(|_| "pairing payload could not be authenticated".to_string())
}

fn derive_material(
    shared: &[u8; 32],
    transcript: &[u8],
) -> Result<([u8; 32], String, Vec<u8>), String> {
    let hkdf = Hkdf::<Sha256>::new(Some(b"rampage-lan-pairing-v1"), shared);
    let mut encryption_key = [0_u8; 32];
    let mut key_info = b"invite-key\0".to_vec();
    key_info.extend_from_slice(transcript);
    hkdf.expand(&key_info, &mut encryption_key)
        .map_err(|_| "could not derive pairing encryption key".to_string())?;
    let mut code_bytes = [0_u8; 4];
    let mut code_info = b"verification-code\0".to_vec();
    code_info.extend_from_slice(transcript);
    hkdf.expand(&code_info, &mut code_bytes)
        .map_err(|_| "could not derive pairing verification code".to_string())?;
    let verification_code = format!("{:04}", u32::from_be_bytes(code_bytes) % 10_000);
    let aad = Sha256::digest(transcript).to_vec();
    Ok((encryption_key, verification_code, aad))
}

fn pairing_transcript(
    request_id: &str,
    worker_public: &[u8; 32],
    owner_public: &[u8; 32],
) -> Vec<u8> {
    let mut transcript = b"rampage.lan-pairing.transcript.v1\0".to_vec();
    transcript.extend_from_slice(request_id.as_bytes());
    transcript.push(0);
    transcript.extend_from_slice(worker_public);
    transcript.extend_from_slice(owner_public);
    transcript
}

fn bind_owner_socket() -> Result<Arc<UdpSocket>, String> {
    let socket = StdUdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PAIRING_PORT))
        .map_err(|error| format!("could not open the local pairing listener: {error}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    socket
        .set_broadcast(true)
        .map_err(|error| error.to_string())?;
    let mut joined = false;
    for interface in active_ipv4_interfaces() {
        if socket
            .join_multicast_v4(&PAIRING_MULTICAST, &interface.ip)
            .is_ok()
        {
            joined = true;
        }
    }
    if !joined {
        let _ = socket.join_multicast_v4(&PAIRING_MULTICAST, &Ipv4Addr::UNSPECIFIED);
    }
    UdpSocket::from_std(socket)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

fn bind_worker_socket() -> Result<UdpSocket, String> {
    let socket = StdUdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|error| format!("could not open a local pairing socket: {error}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    socket
        .set_broadcast(true)
        .map_err(|error| error.to_string())?;
    UdpSocket::from_std(socket).map_err(|error| error.to_string())
}

fn active_ipv4_interfaces() -> Vec<Ifv4Addr> {
    let mut interfaces = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| {
            interface.is_oper_up()
                && !interface.is_loopback()
                && !interface.is_p2p()
                && !interface.is_link_local()
        })
        .filter_map(|interface| match interface.addr {
            IfAddr::V4(address) if !address.ip.is_unspecified() => Some(address),
            _ => None,
        })
        .collect::<Vec<_>>();
    interfaces.sort_by_key(|interface| interface.ip);
    interfaces.dedup_by_key(|interface| interface.ip);
    interfaces
}

fn pairing_destinations() -> Vec<SocketAddr> {
    pairing_destinations_for(active_ipv4_interfaces())
}

fn pairing_destinations_for(interfaces: impl IntoIterator<Item = Ifv4Addr>) -> Vec<SocketAddr> {
    let mut addresses = vec![Ipv4Addr::BROADCAST, PAIRING_MULTICAST];
    addresses.extend(
        interfaces
            .into_iter()
            .filter_map(|interface| interface.broadcast),
    );
    addresses.sort_unstable();
    addresses.dedup();
    addresses
        .into_iter()
        .map(|address| SocketAddr::V4(SocketAddrV4::new(address, PAIRING_PORT)))
        .collect()
}

fn prune_owner_state(inner: &mut PairingInner, now: u64) {
    inner
        .pending
        .retain(|_, pending| pending.expires_at_ms > now);
    for attempts in inner.attempts.values_mut() {
        while attempts
            .front()
            .is_some_and(|timestamp| timestamp.saturating_add(60_000) <= now)
        {
            attempts.pop_front();
        }
    }
    inner.attempts.retain(|_, attempts| !attempts.is_empty());
    if inner.owner_open_until_ms <= now {
        inner.pending.clear();
    }
}

fn admit_source_attempt(
    attempts: &mut HashMap<IpAddr, VecDeque<u64>>,
    source: IpAddr,
    now: u64,
) -> bool {
    let attempts = attempts.entry(source).or_default();
    while attempts
        .front()
        .is_some_and(|timestamp| timestamp.saturating_add(60_000) <= now)
    {
        attempts.pop_front();
    }
    if attempts.len() >= MAX_NEW_REQUESTS_PER_IP_PER_MINUTE {
        return false;
    }
    attempts.push_back(now);
    true
}

fn set_worker_status(
    manager: &PairingManager,
    generation: u64,
    status: WorkerPairingView,
) -> Result<(), String> {
    let mut inner = manager
        .inner
        .lock()
        .map_err(|_| "pairing state lock poisoned".to_string())?;
    if inner.worker_generation == generation {
        inner.worker = status;
    }
    Ok(())
}

fn generation_is_current(manager: &PairingManager, generation: u64) -> Result<bool, String> {
    manager
        .inner
        .lock()
        .map(|inner| inner.worker_generation == generation)
        .map_err(|_| "pairing state lock poisoned".to_string())
}

fn valid_request_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn bounded_label(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 64 || trimmed.chars().any(char::is_control) {
        return Err(format!("{label} must contain 1 to 64 visible characters"));
    }
    Ok(trimmed.to_string())
}

fn decode_32(value: &str) -> Result<[u8; 32], String> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| "pairing public key is not valid base64".to_string())?;
    decoded
        .try_into()
        .map_err(|_| "pairing public key has the wrong size".to_string())
}

fn fresh_hex<const N: usize>() -> Result<String, String> {
    fresh_bytes::<N>().map(hex::encode)
}

fn fresh_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0_u8; N];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| "system randomness is unavailable".to_string())?;
    Ok(bytes)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_fixture() -> PendingPair {
        let request_id = "01".repeat(16);
        let worker_secret = StaticSecret::from([7_u8; 32]);
        let owner_secret = StaticSecret::from([9_u8; 32]);
        let worker_public = PublicKey::from(&worker_secret).to_bytes();
        let owner_public = PublicKey::from(&owner_secret).to_bytes();
        let owner_shared = owner_secret
            .diffie_hellman(&PublicKey::from(worker_public))
            .to_bytes();
        let transcript = pairing_transcript(&request_id, &worker_public, &owner_public);
        let (key, verification_code, aad) = derive_material(&owner_shared, &transcript).unwrap();
        PendingPair {
            request_id,
            device_name: "Laptop".into(),
            device_kind: "desktop".into(),
            peer_addr: "127.0.0.1:40000".parse().unwrap(),
            peer_public_key: BASE64.encode(worker_public),
            verification_code,
            key,
            aad,
            expires_at_ms: now_ms() + 60_000,
            challenge_payload: Vec::new(),
            approval_payload: None,
            state: PairingRequestState::AwaitingApproval,
        }
    }

    #[test]
    fn both_devices_derive_the_same_key_and_four_digit_code() {
        let request_id = "ab".repeat(16);
        let worker_secret = StaticSecret::from([11_u8; 32]);
        let owner_secret = StaticSecret::from([19_u8; 32]);
        let worker_public = PublicKey::from(&worker_secret).to_bytes();
        let owner_public = PublicKey::from(&owner_secret).to_bytes();
        let transcript = pairing_transcript(&request_id, &worker_public, &owner_public);
        let owner_material = derive_material(
            &owner_secret
                .diffie_hellman(&PublicKey::from(worker_public))
                .to_bytes(),
            &transcript,
        )
        .unwrap();
        let worker_material = derive_material(
            &worker_secret
                .diffie_hellman(&PublicKey::from(owner_public))
                .to_bytes(),
            &transcript,
        )
        .unwrap();
        assert_eq!(owner_material, worker_material);
        assert_eq!(owner_material.1.len(), 4);
        assert!(owner_material.1.bytes().all(|byte| byte.is_ascii_digit()));
    }

    #[test]
    fn approval_hides_and_authenticates_the_real_invite() {
        let pending = pending_fixture();
        let invitation = r#"{"schema":"rampage.enrollment-invite.v1","secret":"never-plaintext"}"#;
        let encoded = encrypted_approval(&pending, invitation).unwrap();
        let wire = String::from_utf8(encoded).unwrap();
        assert!(!wire.contains("never-plaintext"));
        let PairingDatagram::Approval {
            nonce, ciphertext, ..
        } = serde_json::from_str(&wire).unwrap()
        else {
            panic!("expected approval datagram")
        };
        assert_eq!(
            decrypt_approval(&pending.key, &pending.aad, &nonce, &ciphertext).unwrap(),
            invitation
        );
        let mut wrong_aad = pending.aad.clone();
        wrong_aad[0] ^= 1;
        assert!(decrypt_approval(&pending.key, &wrong_aad, &nonce, &ciphertext).is_err());
    }

    #[test]
    fn completion_receipt_is_authenticated_and_contains_no_invite() {
        let pending = pending_fixture();
        let encoded =
            encrypted_completion(&pending.request_id, &pending.key, &pending.aad).unwrap();
        let wire = String::from_utf8(encoded).unwrap();
        assert!(!wire.contains("enrollment-complete"));
        let PairingDatagram::EnrollmentComplete {
            nonce, ciphertext, ..
        } = serde_json::from_str(&wire).unwrap()
        else {
            panic!("expected completion datagram")
        };
        assert_eq!(
            decrypt_payload(&pending.key, &pending.aad, &nonce, &ciphertext).unwrap(),
            b"enrollment-complete"
        );
        let wrong_key = [99_u8; 32];
        assert!(decrypt_payload(&wrong_key, &pending.aad, &nonce, &ciphertext).is_err());
    }

    #[test]
    fn approval_payload_is_bounded_to_one_pairing_datagram() {
        let pending = pending_fixture();
        let largest_allowed = "x".repeat(MAX_INVITATION_BYTES);
        let encoded = encrypted_approval(&pending, &largest_allowed).unwrap();
        assert!(encoded.len() <= MAX_DATAGRAM_BYTES);
        assert!(encrypted_approval(&pending, &"x".repeat(MAX_INVITATION_BYTES + 1)).is_err());
    }

    #[tokio::test]
    async fn loopback_discovery_approval_and_completion_work_end_to_end() {
        let manager = PairingManager::default();
        let owner_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let owner_addr = owner_socket.local_addr().unwrap();
        {
            let mut inner = manager.inner.lock().unwrap();
            inner.owner_socket = Some(owner_socket.clone());
            inner.owner_name = "MAIN-PC".into();
            inner.owner_open_until_ms = now_ms() + 60_000;
        }
        let receiver = tokio::spawn(owner_receive_loop(manager.clone(), owner_socket));
        let worker_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let request_id = "cd".repeat(16);
        let worker_secret = StaticSecret::from([27_u8; 32]);
        let worker_public = PublicKey::from(&worker_secret).to_bytes();
        let hello = serde_json::to_vec(&PairingDatagram::Hello {
            schema: PAIRING_SCHEMA.into(),
            request_id: request_id.clone(),
            device_name: "Studio Laptop".into(),
            device_kind: "desktop".into(),
            ephemeral_public_key: BASE64.encode(worker_public),
            // Pairing must remain available even when the laptop's wall clock is wrong. The
            // owner bounds the request with its own three-minute enrollment window.
            issued_at_ms: u64::MAX,
            expires_at_ms: 0,
        })
        .unwrap();
        worker_socket.send_to(&hello, owner_addr).await.unwrap();

        let mut buffer = vec![0_u8; MAX_DATAGRAM_BYTES];
        let (challenge_length, challenge_source) =
            tokio::time::timeout(Duration::from_secs(2), worker_socket.recv_from(&mut buffer))
                .await
                .expect("owner challenge timed out")
                .unwrap();
        assert_eq!(challenge_source, owner_addr);
        let PairingDatagram::Challenge {
            ephemeral_public_key,
            ..
        } = serde_json::from_slice(&buffer[..challenge_length]).unwrap()
        else {
            panic!("expected owner challenge")
        };
        let owner_public = decode_32(&ephemeral_public_key).unwrap();
        let shared = worker_secret
            .diffie_hellman(&PublicKey::from(owner_public))
            .to_bytes();
        let transcript = pairing_transcript(&request_id, &worker_public, &owner_public);
        let (key, verification_code, aad) = derive_material(&shared, &transcript).unwrap();
        assert_eq!(
            owner_window(&manager).unwrap().requests[0].verification_code,
            verification_code
        );

        let invitation = r#"{"schema":"rampage.enrollment-invite.v1","secret":"encrypted"}"#;
        approve(&manager, &request_id, invitation).await.unwrap();
        let (approval_length, approval_source) =
            tokio::time::timeout(Duration::from_secs(2), worker_socket.recv_from(&mut buffer))
                .await
                .expect("encrypted approval timed out")
                .unwrap();
        assert_eq!(approval_source, owner_addr);
        let PairingDatagram::Approval {
            nonce, ciphertext, ..
        } = serde_json::from_slice(&buffer[..approval_length]).unwrap()
        else {
            panic!("expected encrypted approval")
        };
        assert_eq!(
            decrypt_approval(&key, &aad, &nonce, &ciphertext).unwrap(),
            invitation
        );

        let completion = encrypted_completion(&request_id, &key, &aad).unwrap();
        worker_socket
            .send_to(&completion, owner_addr)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if owner_window(&manager).unwrap().requests[0].state == "completed" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("completion receipt timed out");
        receiver.abort();
    }

    #[test]
    fn pairing_labels_and_request_ids_are_bounded() {
        assert!(valid_request_id(&"00".repeat(16)));
        assert!(!valid_request_id("1234"));
        assert!(bounded_label("Laptop", "device").is_ok());
        assert!(bounded_label("Laptop\nInjected", "device").is_err());
        assert!(bounded_label(&"x".repeat(65), "device").is_err());
    }

    #[test]
    fn discovery_targets_every_active_lan_broadcast_once() {
        let destinations = pairing_destinations_for([
            Ifv4Addr {
                ip: "192.168.86.32".parse().unwrap(),
                netmask: "255.255.255.0".parse().unwrap(),
                prefixlen: 24,
                broadcast: Some("192.168.86.255".parse().unwrap()),
            },
            Ifv4Addr {
                ip: "192.168.86.44".parse().unwrap(),
                netmask: "255.255.255.0".parse().unwrap(),
                prefixlen: 24,
                broadcast: Some("192.168.86.255".parse().unwrap()),
            },
            Ifv4Addr {
                ip: "10.42.0.8".parse().unwrap(),
                netmask: "255.255.0.0".parse().unwrap(),
                prefixlen: 16,
                broadcast: Some("10.42.255.255".parse().unwrap()),
            },
        ]);
        assert_eq!(
            destinations,
            vec![
                "10.42.255.255:47839".parse().unwrap(),
                "192.168.86.255:47839".parse().unwrap(),
                "239.255.73.82:47839".parse().unwrap(),
                "255.255.255.255:47839".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn new_request_rate_limit_is_per_source() {
        let source: IpAddr = "192.168.1.20".parse().unwrap();
        let mut attempts = HashMap::new();
        let now = now_ms();
        for _ in 0..MAX_NEW_REQUESTS_PER_IP_PER_MINUTE {
            assert!(admit_source_attempt(&mut attempts, source, now));
        }
        assert!(!admit_source_attempt(&mut attempts, source, now));
        assert!(admit_source_attempt(&mut attempts, source, now + 60_001));
    }
}
