mod discovery;
mod remote_assist;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{Duration, Utc};
use clap::Parser;
use ed25519_dalek::SigningKey;
use rampage_protocol::{
    ArtifactRefV1, ArtifactReplicaReceiptV1, ArtifactTransferActionV2, ArtifactTransferProgressV1,
    ArtifactTransferRequestV2, ArtifactTransferResponseV2, AvailabilityV1, DeviceKind,
    EnrollmentInviteV1, EnrollmentRequestV1, ExecutionReceiptV1, JobState,
    LINK_BENCHMARK_TRANSFER_BYTES, LinkBenchmarkV1, MAX_MODEL_OUTPUT_BYTES, MeshControlRequestV1,
    MeshEndpointRecordV1, ModelExecutionReceiptV1, ModelInvocationFrameKind,
    ModelInvocationFrameV1, ModelInvocationRequestV1, ModelUsageV1, NodeIdentityV1,
    RemoteDesktopResponseV1, ResourceOfferV1, StorageClass, WorkClaimV1,
};
use rand::{TryRng as _, rngs::SysRng};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    time::Instant,
};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedControllerV1 {
    schema: String,
    endpoint: MeshEndpointRecordV1,
    governor_public_key: String,
    enrolled_at: chrono::DateTime<Utc>,
}

#[derive(Parser)]
#[command(name = "rampage-agent", about = "Rampage machine resource agent")]
struct Args {
    #[arg(long)]
    node_id: Option<Uuid>,
    #[arg(long, default_value = "desktop")]
    device_kind: String,
    #[arg(long, default_value = "This Device")]
    display_name: String,
    #[arg(long)]
    owner_id: Option<Uuid>,
    #[arg(long, default_value = ".rampage/agent.key")]
    key_file: PathBuf,
    #[arg(long)]
    enrollment_code: Option<String>,
    /// Join bundle exported by the owner. Uses Rampage QUIC instead of a controller TCP port.
    #[arg(long)]
    invite_file: Option<PathBuf>,
    #[arg(long, default_value = "http://127.0.0.1:47831")]
    controller: String,
    #[arg(long)]
    register: bool,
    /// Claim and execute one controller-assigned native task, then submit its signed receipt.
    #[arg(long)]
    work_once: bool,
    /// Continuously refresh the offer and execute admitted work until owner STOP.
    #[arg(long)]
    serve: bool,
}

const OFFER_LIFETIME: Duration = Duration::seconds(45);
const OFFER_HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const CAPABILITY_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const LINK_PROBE_REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
const WORK_CONTROL_REQUEST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_TRUSTED_CLOCK_OFFSET_SECONDS: i64 = 24 * 60 * 60;

fn ollama_loopback_base_url() -> anyhow::Result<String> {
    let configured =
        std::env::var("RAMPAGE_OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let parsed = reqwest::Url::parse(&configured)?;
    anyhow::ensure!(
        parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("127.0.0.1" | "::1"))
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
            && matches!(parsed.path(), "" | "/"),
        "RAMPAGE_OLLAMA_URL must be a plain HTTP loopback IP origin without credentials or a path"
    );
    Ok(configured.trim_end_matches('/').to_string())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let data_dir = std::env::var_os("RAMPAGE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".rampage/runtime"));
    if data_dir.join("KILL").is_file() {
        anyhow::bail!("owner kill latch is active; remove it only through an explicit resume flow");
    }
    let device_kind = parse_device_kind(&args.device_kind)?;
    let signing_key = load_or_create_key(&args.key_file)?;
    let identity_file = args.key_file.with_extension("identity.json");
    let (node_id, owner_id) =
        load_or_create_identity_ids(&identity_file, args.node_id, args.owner_id)?;
    let ollama_base_url = ollama_loopback_base_url()?;
    let invitation = if let Some(path) = &args.invite_file {
        Some(read_json_file_bounded::<EnrollmentInviteV1>(
            path,
            256 * 1024,
        )?)
    } else {
        None
    };
    let enrollment_marker = args.key_file.with_extension("enrolled");
    let controller_pin_file = args.key_file.with_extension("controller-pin.json");
    let stored_pin = if controller_pin_file.is_file() {
        Some(read_json_file_bounded::<PinnedControllerV1>(
            &controller_pin_file,
            64 * 1024,
        )?)
    } else {
        None
    };
    let invitation_endpoint_id = invitation
        .as_ref()
        .and_then(|invite| invite.controller_mesh.as_ref())
        .map(|endpoint| endpoint.endpoint_id.as_str());
    let already_enrolled = stored_pin.is_some()
        || invitation_endpoint_id.is_some_and(|endpoint_id| {
            fs::read_to_string(&enrollment_marker).is_ok_and(|saved| saved.trim() == endpoint_id)
        });
    let migrated_pin = if stored_pin.is_none() && already_enrolled {
        Some(pin_from_invitation(invitation.as_ref().ok_or_else(
            || anyhow::anyhow!("stored enrollment is missing its controller route"),
        )?)?)
    } else {
        None
    };
    let controller_pin = stored_pin.as_ref().or(migrated_pin.as_ref());
    let remote_controller = controller_pin
        .map(RemoteController::Pinned)
        .or_else(|| invitation.as_ref().map(RemoteController::Invitation));
    let identity = NodeIdentityV1 {
        schema: "rampage.node-identity.v1".into(),
        node_id,
        owner_id,
        display_name: args.display_name.clone(),
        device_kind,
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        public_key: hex::encode(signing_key.verifying_key().to_bytes()),
        enrolled_at: Utc::now(),
        fencing_epoch: 0,
    };
    let mut base_labels = BTreeMap::new();
    base_labels.insert("device_kind".into(), args.device_kind.clone());
    base_labels.insert("device_name".into(), args.display_name.clone());
    base_labels.insert(
        "os".into(),
        sysinfo::System::name().unwrap_or_else(|| std::env::consts::OS.into()),
    );
    base_labels.insert("arch".into(), std::env::consts::ARCH.into());
    base_labels.insert("node_id".into(), node_id.to_string());
    let discovered = discovery::discover(base_labels, &data_dir);
    let storage_capacity = |class| {
        discovered
            .resources
            .iter()
            .find(|resource| resource.class == class)
            .map_or(0, |resource| resource.available)
    };
    let artifact_store = std::sync::Arc::new(rampage_storage::CasStore::open_with_limits(
        data_dir.join("cas"),
        load_or_create_secret(&data_dir.join("storage.key"))?,
        Some(rampage_storage::StorageLimits {
            cache_bytes: storage_capacity(rampage_protocol::ResourceClass::StorageCache),
            scratch_bytes: storage_capacity(rampage_protocol::ResourceClass::StorageScratch),
            protected_bytes: storage_capacity(rampage_protocol::ResourceClass::ProtectedStore),
        }),
    )?);
    let transport = ControllerTransport::new(
        &args.controller,
        remote_controller,
        &signing_key,
        &data_dir,
        node_id,
        artifact_store.clone(),
        &ollama_base_url,
    )?;
    let now = transport.controller_now();
    let mut adapters = BTreeSet::from([
        "rampage.echo.v1".into(),
        "rampage.hash.v1".into(),
        "rampage.eval-shard.v1".into(),
        "rampage.artifact-hash.v1".into(),
        "rampage.benchmark.v1".into(),
    ]);
    let has_ollama = discovery::ollama_available(&ollama_base_url);
    if has_ollama {
        adapters.insert("rampage.ollama.v1".into());
    }
    if remote_assist::enabled(&data_dir) {
        adapters.insert("rampage.remote-assist.v1".into());
    }
    let model_runtimes = match discovery::discover_model_runtimes(
        &discovered.resources,
        has_ollama,
        &ollama_base_url,
    ) {
        Ok(profiles) => profiles,
        Err(error) => {
            eprintln!("model runtime profiles rejected; continuing fail-closed: {error}");
            Vec::new()
        }
    };
    adapters.extend(model_runtimes.iter().map(|profile| profile.adapter.clone()));
    let workload_capabilities =
        discovery::discover_workload_capabilities(&adapters, &model_runtimes);
    let offer = ResourceOfferV1 {
        schema: "rampage.resource-offer.v1".into(),
        offer_id: Uuid::now_v7(),
        node_id,
        observed_at: now,
        expires_at: now + OFFER_LIFETIME,
        resources: discovered.resources,
        availability: AvailabilityV1 {
            on_ac_power: discovered.on_ac_power,
            battery_percent: discovered.battery_percent,
            thermal_headroom_percent: discovered.thermal_headroom_percent,
            foreground_allowed: if matches!(
                device_kind,
                DeviceKind::Phone | DeviceKind::Tablet | DeviceKind::Console
            ) {
                std::env::var("RAMPAGE_EDGE_FOREGROUND")
                    .is_ok_and(|value| value.eq_ignore_ascii_case("true"))
            } else {
                true
            },
            owner_idle: discovered.owner_idle,
        },
        adapters,
        workload_capabilities,
        model_runtimes,
        link_benchmark: None,
        mesh_endpoint: transport.signed_worker_endpoint(&signing_key, now, now + OFFER_LIFETIME),
        signature: String::new(),
    };
    let mut offer = offer;
    rampage_policy::sign_offer(&signing_key, &mut offer);
    let enrollment_code = args.enrollment_code.as_deref().or_else(|| {
        invitation
            .as_ref()
            .filter(|_| !already_enrolled)
            .map(|invite| invite.enrollment_code.as_str())
    });
    if let Some(code) = enrollment_code {
        let (invite_id, secret) = parse_enrollment_code(code)?;
        let mut request = EnrollmentRequestV1 {
            schema: "rampage.enrollment-request.v1".into(),
            invite_id,
            enrollment_code: secret,
            identity: identity.clone(),
            signature: String::new(),
        };
        rampage_policy::sign_enrollment(&signing_key, &mut request);
        transport.post_json("/v1/nodes/enroll", &request)?;
        if let Some(endpoint_id) = invitation_endpoint_id {
            let temporary = enrollment_marker.with_extension("enrolled.tmp");
            fs::write(&temporary, endpoint_id)?;
            fs::rename(temporary, &enrollment_marker)?;
        }
    }
    if stored_pin.is_none() && invitation.is_some() {
        let pin =
            if let Some(pin) = migrated_pin {
                pin
            } else {
                pin_from_invitation(invitation.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("remote enrollment is missing its invitation")
                })?)?
            };
        write_json_atomic(&controller_pin_file, &pin)?;
        if let Some(path) = &args.invite_file {
            fs::remove_file(path)?;
        }
    }
    if args.serve {
        let (execution_tx, execution_rx) =
            std::sync::mpsc::channel::<anyhow::Result<ExecutionReceiptV1>>();
        let receipt_outbox = args.key_file.with_extension("receipt.json");
        let mut work_in_flight = false;
        let mut ready_announced = false;
        let mut reconnect_delay = std::time::Duration::from_secs(1);
        let mut next_offer_at = Instant::now();
        let mut next_work_poll_at = Instant::now();
        let mut next_link_probe_at = Instant::now();
        let mut next_capability_refresh_at = Instant::now() + CAPABILITY_REFRESH_INTERVAL;
        loop {
            if data_dir.join("KILL").is_file() {
                return Ok(());
            }
            if Instant::now() >= next_capability_refresh_at {
                refresh_dynamic_capabilities(&mut offer, &ollama_base_url, &data_dir);
                next_capability_refresh_at = Instant::now() + CAPABILITY_REFRESH_INTERVAL;
            }
            if Instant::now() >= next_offer_at {
                let now = transport.controller_now();
                offer.offer_id = Uuid::now_v7();
                offer.observed_at = now;
                offer.expires_at = now + OFFER_LIFETIME;
                if offer
                    .link_benchmark
                    .as_ref()
                    .is_some_and(|benchmark| benchmark.expires_at < offer.expires_at)
                {
                    offer.link_benchmark = None;
                }
                offer.mesh_endpoint =
                    transport.signed_worker_endpoint(&signing_key, now, offer.expires_at);
                rampage_policy::sign_offer(&signing_key, &mut offer);
                if let Err(error) = transport.post_json("/v1/offers", &offer) {
                    eprintln!(
                        "owner PC unavailable; retrying the signed fabric connection in {} second(s): {error}",
                        reconnect_delay.as_secs()
                    );
                    std::thread::sleep(reconnect_delay);
                    reconnect_delay = (reconnect_delay * 2).min(std::time::Duration::from_secs(10));
                    next_offer_at = Instant::now();
                    continue;
                }
                reconnect_delay = std::time::Duration::from_secs(1);
                next_offer_at = Instant::now() + OFFER_HEARTBEAT_INTERVAL;
                if !ready_announced {
                    println!(
                        "{}",
                        serde_json::to_string(&serde_json::json!({
                            "schema": "rampage.worker-ready.v1",
                            "node_id": node_id,
                            "offer_id": offer.offer_id,
                            "offer_expires_at": offer.expires_at,
                        }))?
                    );
                    std::io::stdout().flush()?;
                    ready_announced = true;
                }
            }

            if receipt_outbox.is_file() {
                let pending: ExecutionReceiptV1 =
                    serde_json::from_slice(&fs::read(&receipt_outbox)?)?;
                if let Err(error) = transport.post_json_with_deadline(
                    "/v1/receipts",
                    &pending,
                    WORK_CONTROL_REQUEST_DEADLINE,
                ) {
                    eprintln!("signed receipt delivery unavailable; retaining outbox: {error}");
                } else {
                    fs::remove_file(&receipt_outbox)?;
                    println!("{}", serde_json::to_string_pretty(&pending)?);
                }
            }
            match execution_rx.try_recv() {
                Ok(Ok(receipt)) => {
                    let temporary = receipt_outbox.with_extension("receipt.tmp");
                    fs::write(&temporary, serde_json::to_vec_pretty(&receipt)?)?;
                    fs::rename(&temporary, &receipt_outbox)?;
                    work_in_flight = false;
                }
                Ok(Err(error)) => {
                    eprintln!("admitted work failed locally: {error}");
                    work_in_flight = false;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) if work_in_flight => {
                    anyhow::bail!("worker execution channel disconnected")
                }
                Err(_) => {}
            }

            let benchmark_expires_at = offer
                .link_benchmark
                .as_ref()
                .map(|benchmark| benchmark.expires_at);
            let link_probe_is_due = link_probe_due(
                ready_announced,
                Instant::now(),
                next_link_probe_at,
                benchmark_expires_at,
                offer.expires_at,
            );
            if link_probe_is_due {
                let observed_at = transport.controller_now();
                match transport.measure_link(node_id, observed_at) {
                    Ok(benchmark) => {
                        offer.link_benchmark = benchmark;
                        next_link_probe_at = Instant::now() + std::time::Duration::from_secs(60);
                    }
                    Err(error) => {
                        eprintln!(
                            "link benchmark unavailable; placement will stay conservative: {error}"
                        );
                        offer.link_benchmark = None;
                        next_link_probe_at = Instant::now() + std::time::Duration::from_secs(30);
                    }
                }
            }
            if !work_in_flight && Instant::now() >= next_work_poll_at {
                match transport.get_json_with_deadline::<Option<WorkClaimV1>>(
                    &format!("/v1/work/claim?node_id={}", identity.node_id),
                    WORK_CONTROL_REQUEST_DEADLINE,
                ) {
                    Ok(Some(claim)) => {
                        let execution_tx = execution_tx.clone();
                        let signing_key = signing_key.clone();
                        let artifact_store = artifact_store.clone();
                        let worker_node_id = identity.node_id;
                        std::thread::Builder::new()
                            .name("rampage-work-executor".into())
                            .spawn(move || {
                                let result = rampage_agent::execute_claim_with_store(
                                    &claim,
                                    worker_node_id,
                                    claim.lease.fencing_epoch,
                                    &signing_key,
                                    artifact_store.as_ref(),
                                )
                                .map_err(anyhow::Error::from);
                                let _ = execution_tx.send(result);
                            })?;
                        work_in_flight = true;
                        next_work_poll_at = Instant::now();
                    }
                    Ok(None) => {
                        next_work_poll_at = Instant::now() + std::time::Duration::from_secs(2);
                    }
                    Err(error) => {
                        eprintln!(
                            "fabric work channel unavailable; retrying without dropping enrollment: {error}"
                        );
                        next_work_poll_at = Instant::now() + std::time::Duration::from_secs(2);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    if args.register {
        transport.post_json("/v1/offers", &offer)?;
    }
    if args.work_once {
        if !execute_one_work_item(
            &args,
            &transport,
            &identity,
            &signing_key,
            artifact_store.as_ref(),
        )? {
            println!("No admissible work is waiting for this node.");
        }
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&offer)?);
    Ok(())
}

fn link_probe_due(
    ready_announced: bool,
    now: Instant,
    next_probe_at: Instant,
    benchmark_expires_at: Option<chrono::DateTime<Utc>>,
    offer_expires_at: chrono::DateTime<Utc>,
) -> bool {
    ready_announced
        && now >= next_probe_at
        && benchmark_expires_at.is_none_or(|expires_at| expires_at < offer_expires_at)
}

fn refresh_dynamic_capabilities(
    offer: &mut ResourceOfferV1,
    ollama_base_url: &str,
    data_dir: &Path,
) {
    let has_ollama = discovery::ollama_available(ollama_base_url);
    if has_ollama {
        offer.adapters.insert("rampage.ollama.v1".into());
    } else {
        offer.adapters.remove("rampage.ollama.v1");
    }
    if remote_assist::enabled(data_dir) {
        offer.adapters.insert("rampage.remote-assist.v1".into());
    } else {
        offer.adapters.remove("rampage.remote-assist.v1");
        remote_assist::clear_active(data_dir);
    }
    offer.model_runtimes =
        match discovery::discover_model_runtimes(&offer.resources, has_ollama, ollama_base_url) {
            Ok(profiles) => profiles,
            Err(error) => {
                eprintln!("model runtime refresh rejected; continuing fail-closed: {error}");
                Vec::new()
            }
        };
    offer.workload_capabilities =
        discovery::discover_workload_capabilities(&offer.adapters, &offer.model_runtimes);
}

fn execute_one_work_item(
    args: &Args,
    transport: &ControllerTransport,
    identity: &NodeIdentityV1,
    signing_key: &SigningKey,
    artifact_store: &rampage_storage::CasStore,
) -> anyhow::Result<bool> {
    let receipt_outbox = args.key_file.with_extension("receipt.json");
    if receipt_outbox.is_file() {
        let pending: ExecutionReceiptV1 = serde_json::from_slice(&fs::read(&receipt_outbox)?)?;
        transport.post_json("/v1/receipts", &pending)?;
        fs::remove_file(&receipt_outbox)?;
    }
    let claim: Option<WorkClaimV1> =
        transport.get_json(&format!("/v1/work/claim?node_id={}", identity.node_id))?;
    let Some(claim) = claim else {
        return Ok(false);
    };
    let receipt = rampage_agent::execute_claim_with_store(
        &claim,
        identity.node_id,
        claim.lease.fencing_epoch,
        signing_key,
        artifact_store,
    )?;
    let temporary = receipt_outbox.with_extension("receipt.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&receipt)?)?;
    fs::rename(&temporary, &receipt_outbox)?;
    transport.post_json("/v1/receipts", &receipt)?;
    fs::remove_file(&receipt_outbox)?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(true)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct IdentityIds {
    node_id: Uuid,
    owner_id: Uuid,
}

fn load_or_create_identity_ids(
    path: &PathBuf,
    requested_node_id: Option<Uuid>,
    requested_owner_id: Option<Uuid>,
) -> anyhow::Result<(Uuid, Uuid)> {
    if path.is_file() {
        let existing: IdentityIds = read_json_file_bounded(path, 16 * 1024)?;
        if requested_node_id.is_some_and(|value| value != existing.node_id)
            || requested_owner_id.is_some_and(|value| value != existing.owner_id)
        {
            anyhow::bail!("requested identity conflicts with the persistent agent identity");
        }
        return Ok((existing.node_id, existing.owner_id));
    }
    let identity = IdentityIds {
        node_id: requested_node_id.unwrap_or_else(Uuid::now_v7),
        owner_id: requested_owner_id.unwrap_or_else(Uuid::now_v7),
    };
    fs::write(path, serde_json::to_vec_pretty(&identity)?)?;
    Ok((identity.node_id, identity.owner_id))
}

fn read_json_file_bounded<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    max_bytes: u64,
) -> anyhow::Result<T> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(metadata.is_file(), "JSON input must be a regular file");
    anyhow::ensure!(
        metadata.len() <= max_bytes,
        "JSON input exceeds the {max_bytes} byte limit"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= max_bytes,
        "JSON input exceeds the {max_bytes} byte limit"
    );
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    Ok(serde_json::from_slice(bytes)?)
}

fn load_or_create_key(path: &Path) -> anyhow::Result<SigningKey> {
    Ok(SigningKey::from_bytes(&load_or_create_secret_bytes(
        path,
        "agent signing key",
    )?))
}

enum ControllerTransport {
    Http { base: String, token: String },
    Mesh(MeshController),
}

struct MeshController {
    runtime: tokio::runtime::Runtime,
    endpoint: iroh::Endpoint,
    controller_endpoint_id: String,
    destination: iroh::EndpointAddr,
    controller_clock_offset: std::sync::Mutex<Duration>,
}

enum RemoteController<'a> {
    Invitation(&'a EnrollmentInviteV1),
    Pinned(&'a PinnedControllerV1),
}

fn mesh_config_for_controller(
    record: &MeshEndpointRecordV1,
) -> anyhow::Result<rampage_mesh::MeshConfig> {
    let mode = if record.relay_urls.is_empty() {
        rampage_mesh::MeshMode::LocalOnly
    } else {
        rampage_mesh::MeshMode::PrivateRelay {
            urls: record.relay_urls.clone(),
        }
    };
    let config = rampage_mesh::MeshConfig {
        schema: "rampage.mesh-config.v1".into(),
        mode,
        allowed_peer_keys: BTreeSet::from([record.endpoint_id.clone()]),
    };
    config.validate()?;
    Ok(config)
}

impl ControllerTransport {
    fn new(
        controller: &str,
        remote: Option<RemoteController<'_>>,
        signing_key: &SigningKey,
        data_dir: &std::path::Path,
        node_id: Uuid,
        artifact_store: std::sync::Arc<rampage_storage::CasStore>,
        ollama_base_url: &str,
    ) -> anyhow::Result<Self> {
        let Some(remote) = remote else {
            anyhow::ensure!(
                controller.starts_with("http://127.0.0.1:")
                    || controller.starts_with("http://[::1]:")
                    || controller.starts_with("http://localhost:"),
                "plain HTTP controller transport is restricted to loopback; use an invite file for remote enrollment"
            );
            let token = std::env::var("RAMPAGE_TOKEN")
                .ok()
                .or_else(|| fs::read_to_string(data_dir.join("controller.token")).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_default();
            return Ok(Self::Http {
                base: controller.trim_end_matches('/').to_string(),
                token,
            });
        };
        let (mesh_record, governor_key, aggregate, candidates) = match remote {
            RemoteController::Invitation(invitation) => {
                anyhow::ensure!(
                    invitation.schema == "rampage.enrollment-invite.v1"
                        && invitation.expires_at > Utc::now(),
                    "invite is expired or has an unsupported schema"
                );
                let mesh_record = invitation
                    .controller_mesh
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("invite does not contain a mesh endpoint"))?;
                let governor_key = invitation.governor_public_key.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("invite does not contain a Governor public key")
                })?;
                rampage_policy::verify_mesh_endpoint_with_key(governor_key, mesh_record)
                    .map_err(|_| anyhow::anyhow!("invite mesh endpoint signature is invalid"))?;
                (
                    mesh_record,
                    governor_key,
                    rampage_mesh::endpoint_addr_from_record(mesh_record)?,
                    rampage_mesh::endpoint_addr_candidates_from_record(mesh_record)?,
                )
            }
            RemoteController::Pinned(pin) => {
                validate_controller_pin(pin)?;
                (
                    &pin.endpoint,
                    pin.governor_public_key.as_str(),
                    rampage_mesh::endpoint_addr_from_pinned_record(&pin.endpoint)?,
                    rampage_mesh::endpoint_addr_candidates_from_pinned_record(&pin.endpoint)?,
                )
            }
        };
        let destination = select_controller_destination(aggregate, candidates);
        let mesh_config = mesh_config_for_controller(mesh_record)?;
        let runtime = tokio::runtime::Runtime::new()?;
        let endpoint = runtime.block_on(rampage_mesh::bind_endpoint(
            signing_key.to_bytes(),
            &mesh_config,
        ))?;
        runtime.spawn(serve_worker_gateway(
            endpoint.clone(),
            WorkerGatewayConfig {
                controller_endpoint_id: mesh_record.endpoint_id.clone(),
                node_id,
                governor_public_key: governor_key.to_string(),
                store: artifact_store,
                signing_key: SigningKey::from_bytes(&signing_key.to_bytes()),
                ollama_base_url: ollama_base_url.to_string(),
                data_dir: data_dir.to_path_buf(),
                remote_authority: std::sync::Arc::new(remote_assist::SessionAuthority::default()),
            },
        ));
        let mesh = MeshController {
            runtime,
            endpoint,
            controller_endpoint_id: mesh_record.endpoint_id.clone(),
            destination,
            controller_clock_offset: std::sync::Mutex::new(Duration::zero()),
        };
        if let Err(error) = mesh.request("GET", "/health", None) {
            eprintln!(
                "authenticated controller clock alignment unavailable; retrying with the local clock: {error}"
            );
        }
        Ok(Self::Mesh(mesh))
    }

    fn controller_now(&self) -> chrono::DateTime<Utc> {
        match self {
            Self::Http { .. } => Utc::now(),
            Self::Mesh(mesh) => mesh.controller_now(),
        }
    }

    fn signed_worker_endpoint(
        &self,
        signing_key: &SigningKey,
        issued_at: chrono::DateTime<Utc>,
        expires_at: chrono::DateTime<Utc>,
    ) -> Option<MeshEndpointRecordV1> {
        let Self::Mesh(mesh) = self else {
            return None;
        };
        let address = mesh.endpoint.addr();
        let mut record = MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: address.id.to_string(),
            direct_addresses: address.ip_addrs().map(ToString::to_string).collect(),
            relay_urls: address.relay_urls().map(ToString::to_string).collect(),
            issued_at,
            expires_at,
            signature: String::new(),
        };
        rampage_policy::sign_mesh_endpoint(signing_key, &mut record);
        Some(record)
    }

    fn measure_link(
        &self,
        node_id: Uuid,
        observed_at: chrono::DateTime<Utc>,
    ) -> anyhow::Result<Option<LinkBenchmarkV1>> {
        let Self::Mesh(mesh) = self else {
            return Ok(None);
        };
        let mut rtt_samples = Vec::with_capacity(3);
        for _ in 0..3 {
            let started = Instant::now();
            let response =
                mesh.request_with_deadline("GET", "/health", None, LINK_PROBE_REQUEST_DEADLINE)?;
            anyhow::ensure!(
                response.status == 200,
                "controller health probe was rejected"
            );
            rtt_samples.push(started.elapsed().as_micros() as u64);
        }
        rtt_samples.sort_unstable();
        let rtt_micros_p50 = rtt_samples[1].max(1);

        let probe = vec![0x3C_u8; LINK_BENCHMARK_TRANSFER_BYTES as usize];
        let probe_digest = hex::encode(Sha256::digest(&probe));
        let upload_nonce = Uuid::now_v7();
        let upload_started = Instant::now();
        let upload_response = mesh.request_with_deadline(
            "POST",
            "/v1/benchmarks/link",
            Some(serde_json::json!({
                "node_id": node_id,
                "nonce": upload_nonce,
                "upload_base64": BASE64.encode(&probe),
                "download_bytes": 0
            })),
            LINK_PROBE_REQUEST_DEADLINE,
        )?;
        let upload_elapsed = upload_started.elapsed().as_micros() as u64;
        validate_probe_response(
            &upload_response,
            node_id,
            upload_nonce,
            LINK_BENCHMARK_TRANSFER_BYTES,
            &probe_digest,
            0,
        )?;

        let download_nonce = Uuid::now_v7();
        let download_started = Instant::now();
        let download_response = mesh.request_with_deadline(
            "POST",
            "/v1/benchmarks/link",
            Some(serde_json::json!({
                "node_id": node_id,
                "nonce": download_nonce,
                "upload_base64": "",
                "download_bytes": LINK_BENCHMARK_TRANSFER_BYTES
            })),
            LINK_PROBE_REQUEST_DEADLINE,
        )?;
        let download_elapsed = download_started.elapsed().as_micros() as u64;
        validate_probe_response(
            &download_response,
            node_id,
            download_nonce,
            0,
            &hex::encode(Sha256::digest([])),
            LINK_BENCHMARK_TRANSFER_BYTES,
        )?;

        Ok(Some(LinkBenchmarkV1 {
            schema: LinkBenchmarkV1::SCHEMA.into(),
            controller_endpoint_id: mesh.controller_endpoint_id.clone(),
            observed_at,
            expires_at: observed_at + Duration::minutes(2),
            rtt_micros_p50,
            uplink_bps: effective_bps(
                LINK_BENCHMARK_TRANSFER_BYTES,
                upload_elapsed,
                rtt_micros_p50,
            ),
            downlink_bps: effective_bps(
                LINK_BENCHMARK_TRANSFER_BYTES,
                download_elapsed,
                rtt_micros_p50,
            ),
            transfer_bytes: LINK_BENCHMARK_TRANSFER_BYTES,
            samples: 3,
            transport: "authenticated_quic".into(),
        }))
    }

    fn post_json<T: serde::Serialize>(&self, path: &str, body: &T) -> anyhow::Result<()> {
        self.post_json_with_deadline(path, body, rampage_mesh::CONTROL_REQUEST_DEADLINE)
    }

    fn post_json_with_deadline<T: serde::Serialize>(
        &self,
        path: &str,
        body: &T,
        deadline: std::time::Duration,
    ) -> anyhow::Result<()> {
        match self {
            Self::Http { base, token } => {
                let response = reqwest::blocking::Client::builder()
                    .timeout(deadline)
                    .build()?
                    .post(format!("{base}{path}"))
                    .header("x-rampage-token", token)
                    .json(body)
                    .send()?;
                if !response.status().is_success() {
                    anyhow::bail!(
                        "controller rejected request: {} {}",
                        response.status(),
                        response.text()?
                    );
                }
                Ok(())
            }
            Self::Mesh(mesh) => {
                let response = mesh.request_with_deadline(
                    "POST",
                    path,
                    Some(serde_json::to_value(body)?),
                    deadline,
                )?;
                anyhow::ensure!(
                    (200..300).contains(&response.status),
                    "mesh controller rejected request: {} {}",
                    response.status,
                    response.body
                );
                Ok(())
            }
        }
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        self.get_json_with_deadline(path, rampage_mesh::CONTROL_REQUEST_DEADLINE)
    }

    fn get_json_with_deadline<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        deadline: std::time::Duration,
    ) -> anyhow::Result<T> {
        match self {
            Self::Http { base, token } => Ok(reqwest::blocking::Client::builder()
                .timeout(deadline)
                .build()?
                .get(format!("{base}{path}"))
                .header("x-rampage-token", token)
                .send()?
                .error_for_status()?
                .json()?),
            Self::Mesh(mesh) => {
                let response = mesh.request_with_deadline("GET", path, None, deadline)?;
                anyhow::ensure!(
                    (200..300).contains(&response.status),
                    "mesh controller rejected request: {} {}",
                    response.status,
                    response.body
                );
                Ok(serde_json::from_value(response.body)?)
            }
        }
    }
}

fn effective_bps(bytes: u64, elapsed_micros: u64, rtt_micros: u64) -> u64 {
    let transfer_micros = elapsed_micros.saturating_sub(rtt_micros).max(1_000);
    bytes
        .saturating_mul(8)
        .saturating_mul(1_000_000)
        .checked_div(transfer_micros)
        .unwrap_or(1)
        .max(1)
}

fn validate_probe_response(
    response: &rampage_protocol::MeshControlResponseV1,
    node_id: Uuid,
    nonce: Uuid,
    expected_upload_bytes: u64,
    expected_upload_digest: &str,
    expected_download_bytes: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(response.status == 200, "link probe was rejected");
    anyhow::ensure!(
        response
            .body
            .get("schema")
            .and_then(serde_json::Value::as_str)
            == Some("rampage.link-probe-response.v1")
            && response
                .body
                .get("node_id")
                .and_then(serde_json::Value::as_str)
                == Some(node_id.to_string().as_str())
            && response
                .body
                .get("nonce")
                .and_then(serde_json::Value::as_str)
                == Some(nonce.to_string().as_str())
            && response
                .body
                .get("upload_bytes")
                .and_then(serde_json::Value::as_u64)
                == Some(expected_upload_bytes)
            && response
                .body
                .get("upload_sha256")
                .and_then(serde_json::Value::as_str)
                == Some(expected_upload_digest),
        "link probe response did not match its request"
    );
    let encoded = response
        .body
        .get("download_base64")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("link probe response omitted download payload"))?;
    let downloaded = BASE64.decode(encoded.as_bytes())?;
    anyhow::ensure!(
        downloaded.len() as u64 == expected_download_bytes
            && response
                .body
                .get("download_sha256")
                .and_then(serde_json::Value::as_str)
                == Some(hex::encode(Sha256::digest(&downloaded)).as_str()),
        "link probe download payload failed integrity validation"
    );
    Ok(())
}

struct ArtifactOperationResult {
    request_id: Uuid,
    artifact: Option<ArtifactRefV1>,
    progress: Option<ArtifactTransferProgressV1>,
    chunk_index: Option<u32>,
    chunk_digest: Option<String>,
    replica_receipt: Option<ArtifactReplicaReceiptV1>,
    payload: Vec<u8>,
}

fn resumable_spec(request: &ArtifactTransferRequestV2) -> rampage_storage::ResumablePutSpec {
    rampage_storage::ResumablePutSpec {
        session_id: request.session_id.simple().to_string(),
        lease_id: request.lease.lease_id.simple().to_string(),
        authority_scope: "governor".into(),
        fencing_epoch: request.lease.fencing_epoch,
        authority_nonce: request.lease.nonce.clone(),
        expires_at: request.lease.expires_at,
        digest: request.lease.digest.clone(),
        size_bytes: request.lease.size_bytes,
        media_type: request.media_type.clone(),
        storage_class: request.lease.storage_class,
        required_replicas: if request.lease.storage_class == StorageClass::Protected {
            2
        } else {
            1
        },
        chunk_size: request.chunk_size,
    }
}

fn transfer_progress(
    session_id: Uuid,
    status: rampage_storage::ResumablePutStatus,
) -> ArtifactTransferProgressV1 {
    ArtifactTransferProgressV1 {
        schema: ArtifactTransferProgressV1::SCHEMA.into(),
        session_id,
        digest: status.digest,
        size_bytes: status.size_bytes,
        chunk_size: status.chunk_size,
        chunk_count: status.chunk_count,
        received_chunks: status.received_chunks,
        missing_chunks: status.missing_chunks,
        complete: status.complete,
    }
}

fn signed_replica_receipt(
    request: &ArtifactTransferRequestV2,
    node_id: Uuid,
    signing_key: &SigningKey,
) -> ArtifactReplicaReceiptV1 {
    let verified_at = Utc::now();
    let mut receipt = ArtifactReplicaReceiptV1 {
        schema: ArtifactReplicaReceiptV1::SCHEMA.into(),
        receipt_id: Uuid::now_v7(),
        session_id: request.session_id,
        lease_id: request.lease.lease_id,
        node_id,
        digest: request.lease.digest.clone(),
        size_bytes: request.lease.size_bytes,
        storage_class: request.lease.storage_class,
        challenge_nonce: request.challenge_nonce.clone(),
        verified_at,
        expires_at: verified_at + Duration::minutes(10),
        fencing_epoch: request.lease.fencing_epoch,
        signature: String::new(),
    };
    rampage_policy::sign_artifact_replica_receipt(signing_key, &mut receipt);
    receipt
}

#[derive(Clone)]
struct WorkerGatewayConfig {
    controller_endpoint_id: String,
    node_id: Uuid,
    governor_public_key: String,
    store: std::sync::Arc<rampage_storage::CasStore>,
    signing_key: SigningKey,
    ollama_base_url: String,
    data_dir: PathBuf,
    remote_authority: std::sync::Arc<remote_assist::SessionAuthority>,
}

async fn serve_worker_gateway(endpoint: iroh::Endpoint, config: WorkerGatewayConfig) {
    while let Some(incoming) = endpoint.accept().await {
        let store = config.store.clone();
        let controller_endpoint_id = config.controller_endpoint_id.clone();
        let node_id = config.node_id;
        let governor_public_key = config.governor_public_key.clone();
        let signing_key = config.signing_key.clone();
        let ollama_base_url = config.ollama_base_url.clone();
        let data_dir = config.data_dir.clone();
        let remote_authority = config.remote_authority.clone();
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else {
                return;
            };
            let peer = connection.remote_id().to_string();
            if peer != controller_endpoint_id {
                connection.close(1_u8.into(), b"worker peer denied");
                return;
            }
            if connection.alpn() == rampage_mesh::MODEL_ALPN {
                serve_model_connection(
                    connection,
                    &peer,
                    node_id,
                    &governor_public_key,
                    store,
                    signing_key,
                    ollama_base_url,
                )
                .await;
                return;
            }
            if connection.alpn() == rampage_mesh::REMOTE_DESKTOP_ALPN {
                serve_remote_desktop_connection(
                    connection,
                    &peer,
                    node_id,
                    &governor_public_key,
                    store,
                    data_dir,
                    remote_authority,
                )
                .await;
                return;
            }
            if connection.alpn() != rampage_mesh::ARTIFACT_ALPN {
                connection.close(1_u8.into(), b"worker protocol denied");
                return;
            }
            while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                let store = store.clone();
                let governor_public_key = governor_public_key.clone();
                let signing_key = signing_key.clone();
                tokio::spawn(async move {
                    let parsed = rampage_mesh::read_artifact_request(&mut receive).await;
                    let response_request_id = parsed
                        .as_ref()
                        .map(|(request, _)| request.request_id)
                        .unwrap_or_else(|_| Uuid::nil());
                    let result = async {
                        let (request, payload) = parsed?;
                        rampage_policy::verify_storage_lease_with_key(
                            &governor_public_key,
                            &request.lease,
                        )?;
                        anyhow::ensure!(
                            request.lease.node_id == node_id
                                && request
                                    .lease
                                    .is_active_at(Utc::now(), request.lease.fencing_epoch),
                            "storage lease is not active for this node"
                        );
                        match request.action {
                            ArtifactTransferActionV2::Begin | ArtifactTransferActionV2::Status => {
                                let status =
                                    store.begin_resumable_put(&resumable_spec(&request))?;
                                Ok(ArtifactOperationResult {
                                    request_id: request.request_id,
                                    artifact: None,
                                    progress: Some(transfer_progress(request.session_id, status)),
                                    chunk_index: None,
                                    chunk_digest: None,
                                    replica_receipt: None,
                                    payload: Vec::new(),
                                })
                            }
                            ArtifactTransferActionV2::PutChunk => {
                                store.begin_resumable_put(&resumable_spec(&request))?;
                                let index = request.chunk_index.expect("validated chunk index");
                                let digest = request
                                    .chunk_digest
                                    .as_deref()
                                    .expect("validated chunk digest");
                                let status = store.put_resumable_chunk(
                                    &request.session_id.simple().to_string(),
                                    index,
                                    digest,
                                    &payload,
                                )?;
                                Ok(ArtifactOperationResult {
                                    request_id: request.request_id,
                                    artifact: None,
                                    progress: Some(transfer_progress(request.session_id, status)),
                                    chunk_index: Some(index),
                                    chunk_digest: Some(digest.into()),
                                    replica_receipt: None,
                                    payload: Vec::new(),
                                })
                            }
                            ArtifactTransferActionV2::Commit => {
                                store.begin_resumable_put(&resumable_spec(&request))?;
                                let artifact = store.commit_resumable_put(
                                    &request.session_id.simple().to_string(),
                                )?;
                                anyhow::ensure!(
                                    artifact.digest == request.lease.digest
                                        && artifact.size_bytes == request.lease.size_bytes,
                                    "stored artifact did not match its lease"
                                );
                                let receipt =
                                    signed_replica_receipt(&request, node_id, &signing_key);
                                Ok(ArtifactOperationResult {
                                    request_id: request.request_id,
                                    artifact: Some(artifact),
                                    progress: Some(transfer_progress(
                                        request.session_id,
                                        store.resumable_put_status(
                                            &request.session_id.simple().to_string(),
                                        )?,
                                    )),
                                    chunk_index: None,
                                    chunk_digest: None,
                                    replica_receipt: Some(receipt),
                                    payload: Vec::new(),
                                })
                            }
                            ArtifactTransferActionV2::GetChunk => {
                                store.accept_authority(
                                    "governor",
                                    request.lease.fencing_epoch,
                                    &request.lease.nonce,
                                    request.lease.expires_at,
                                )?;
                                let stored = store.head(&request.lease.digest)?;
                                anyhow::ensure!(
                                    stored.storage_class == request.lease.storage_class
                                        && stored.size_bytes == request.lease.size_bytes,
                                    "stored artifact metadata did not match its lease"
                                );
                                let index = request.chunk_index.expect("validated chunk index");
                                let chunk = store.get_chunk(&request.lease.digest, index)?;
                                let digest =
                                    format!("sha256:{}", hex::encode(Sha256::digest(&chunk)));
                                Ok(ArtifactOperationResult {
                                    request_id: request.request_id,
                                    artifact: Some(stored),
                                    progress: None,
                                    chunk_index: Some(index),
                                    chunk_digest: Some(digest),
                                    replica_receipt: None,
                                    payload: chunk,
                                })
                            }
                            ArtifactTransferActionV2::Head => {
                                store.accept_authority(
                                    "governor",
                                    request.lease.fencing_epoch,
                                    &request.lease.nonce,
                                    request.lease.expires_at,
                                )?;
                                let stored = store.verify(&request.lease.digest)?;
                                anyhow::ensure!(
                                    stored.storage_class == request.lease.storage_class
                                        && stored.size_bytes == request.lease.size_bytes,
                                    "stored artifact metadata did not match its lease"
                                );
                                let receipt =
                                    signed_replica_receipt(&request, node_id, &signing_key);
                                Ok(ArtifactOperationResult {
                                    request_id: request.request_id,
                                    artifact: Some(stored),
                                    progress: None,
                                    chunk_index: None,
                                    chunk_digest: None,
                                    replica_receipt: Some(receipt),
                                    payload: Vec::new(),
                                })
                            }
                        }
                    }
                    .await;
                    let (response, payload) = match result {
                        Ok(result) => {
                            let payload = result.payload;
                            (
                                ArtifactTransferResponseV2 {
                                    schema: ArtifactTransferResponseV2::SCHEMA.into(),
                                    request_id: result.request_id,
                                    status: 200,
                                    artifact: result.artifact,
                                    progress: result.progress,
                                    chunk_index: result.chunk_index,
                                    chunk_digest: result.chunk_digest,
                                    payload_size: payload.len() as u64,
                                    replica_receipt: result.replica_receipt,
                                    error: None,
                                },
                                payload,
                            )
                        }
                        Err(error) => (
                            ArtifactTransferResponseV2 {
                                schema: ArtifactTransferResponseV2::SCHEMA.into(),
                                request_id: response_request_id,
                                status: 400,
                                artifact: None,
                                progress: None,
                                chunk_index: None,
                                chunk_digest: None,
                                payload_size: 0,
                                replica_receipt: None,
                                error: Some(error.to_string()),
                            },
                            Vec::new(),
                        ),
                    };
                    let _ =
                        rampage_mesh::write_artifact_response(&mut send, &response, &payload).await;
                });
            }
        });
    }
}

async fn serve_remote_desktop_connection(
    connection: iroh::endpoint::Connection,
    controller_endpoint_id: &str,
    node_id: Uuid,
    governor_public_key: &str,
    store: std::sync::Arc<rampage_storage::CasStore>,
    data_dir: PathBuf,
    authority: std::sync::Arc<remote_assist::SessionAuthority>,
) {
    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
        let controller_endpoint_id = controller_endpoint_id.to_string();
        let governor_public_key = governor_public_key.to_string();
        let store = store.clone();
        let data_dir = data_dir.clone();
        let authority = authority.clone();
        tokio::spawn(async move {
            let parsed = rampage_mesh::read_remote_desktop_request(&mut receive).await;
            let request_id = parsed
                .as_ref()
                .map(|request| request.request_id)
                .unwrap_or_else(|_| Uuid::nil());
            let result = match parsed {
                Ok(request) => tokio::task::spawn_blocking(move || {
                    remote_assist::handle_request(
                        request,
                        node_id,
                        &controller_endpoint_id,
                        &governor_public_key,
                        store.as_ref(),
                        &data_dir,
                        authority.as_ref(),
                    )
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result),
                Err(error) => Err(error),
            };
            let (response, payload) = match result {
                Ok(result) => result,
                Err(error) => (
                    RemoteDesktopResponseV1 {
                        schema: RemoteDesktopResponseV1::SCHEMA.into(),
                        request_id,
                        status: 403,
                        frame: None,
                        applied_events: 0,
                        error: Some(bounded_error(&error.to_string())),
                    },
                    Vec::new(),
                ),
            };
            let _ =
                rampage_mesh::write_remote_desktop_response(&mut send, &response, &payload).await;
        });
    }
}

#[derive(Debug, serde::Deserialize)]
struct OllamaChatChunk {
    model: Option<String>,
    message: Option<OllamaChatMessage>,
    #[serde(default)]
    done: bool,
    done_reason: Option<String>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OllamaChatMessage {
    #[serde(default)]
    content: String,
}

struct OllamaCompletion {
    finish_reason: String,
    usage: Option<ModelUsageV1>,
}

async fn serve_model_connection(
    connection: iroh::endpoint::Connection,
    controller_endpoint_id: &str,
    node_id: Uuid,
    governor_public_key: &str,
    store: std::sync::Arc<rampage_storage::CasStore>,
    signing_key: SigningKey,
    ollama_base_url: String,
) {
    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
        let controller_endpoint_id = controller_endpoint_id.to_string();
        let governor_public_key = governor_public_key.to_string();
        let store = store.clone();
        let signing_key = signing_key.clone();
        let ollama_base_url = ollama_base_url.clone();
        tokio::spawn(async move {
            let parsed = rampage_mesh::read_model_request(&mut receive).await;
            let request_id = parsed
                .as_ref()
                .map(|request| request.request_id)
                .unwrap_or_else(|_| Uuid::nil());
            let result = async {
                let request = parsed?;
                rampage_policy::verify_model_session_lease_with_key(
                    &governor_public_key,
                    &request.lease,
                )?;
                anyhow::ensure!(
                    request
                        .lease
                        .is_active_at(Utc::now(), request.lease.fencing_epoch)
                        && request.is_valid_for(node_id, &controller_endpoint_id),
                    "model invocation is outside its signed authority"
                );
                verify_local_ollama_model(&request, &ollama_base_url).await?;
                store.accept_authority(
                    "governor",
                    request.lease.fencing_epoch,
                    &request.lease.nonce,
                    request.lease.expires_at,
                )?;
                run_ollama_invocation(&mut send, &request, &signing_key, &ollama_base_url).await
            }
            .await;
            if let Err(error) = result {
                let message = bounded_error(&error.to_string());
                let frame = ModelInvocationFrameV1 {
                    schema: ModelInvocationFrameV1::SCHEMA.into(),
                    request_id,
                    sequence: 0,
                    kind: ModelInvocationFrameKind::Error,
                    content: String::new(),
                    finish_reason: None,
                    error: Some(message),
                    receipt: None,
                };
                let _ = rampage_mesh::write_model_frame(&mut send, &frame).await;
            }
            let _ = send.finish();
        });
    }
}

async fn verify_local_ollama_model(
    request: &ModelInvocationRequestV1,
    ollama_base_url: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let response = client
        .get(format!("{ollama_base_url}/api/tags"))
        .send()
        .await?
        .error_for_status()?;
    let payload = bounded_json(response, 1024 * 1024).await?;
    let exact = payload
        .get("models")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| {
            models.iter().any(|model| {
                let identifier = model
                    .get("model")
                    .or_else(|| model.get("name"))
                    .and_then(serde_json::Value::as_str);
                let digest = model
                    .get("digest")
                    .and_then(serde_json::Value::as_str)
                    .map(|value| {
                        if value.starts_with("sha256:") {
                            value.to_ascii_lowercase()
                        } else {
                            format!("sha256:{}", value.to_ascii_lowercase())
                        }
                    });
                let size = model.get("size").and_then(serde_json::Value::as_u64);
                identifier == Some(request.lease.model_id.as_str())
                    && digest.as_deref() == Some(request.lease.model_digest.as_str())
                    && size.is_some_and(|value| value > 0)
            })
        });
    anyhow::ensure!(
        exact,
        "the leased Ollama model is not installed with the advertised digest"
    );
    Ok(())
}

async fn run_ollama_invocation(
    send: &mut iroh::endpoint::SendStream,
    request: &ModelInvocationRequestV1,
    signing_key: &SigningKey,
    ollama_base_url: &str,
) -> anyhow::Result<()> {
    let started_at = Utc::now();
    let remaining = (request.lease.expires_at - started_at)
        .to_std()
        .unwrap_or_else(|_| std::time::Duration::from_secs(1));
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(remaining)
        .build()?;
    let mut options = serde_json::Map::from_iter([(
        "num_predict".into(),
        serde_json::json!(request.max_output_tokens),
    )]);
    if let Some(value) = request.temperature {
        options.insert("temperature".into(), serde_json::json!(value));
    }
    if let Some(value) = request.top_p {
        options.insert("top_p".into(), serde_json::json!(value));
    }
    let response = client
        .post(format!("{ollama_base_url}/api/chat"))
        .json(&serde_json::json!({
            "model": request.lease.model_id,
            "messages": request.messages,
            "stream": true,
            // Keep reasoning-capable runtimes in their structured mode. Ollama then places the
            // reasoning trace in `message.thinking`, which Rampage intentionally does not forward,
            // while ordinary answer text remains in `message.content`.
            "think": true,
            "options": options
        }))
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let reason = bounded_http_error(response).await;
        anyhow::bail!("local Ollama rejected the model session ({status}): {reason}");
    }

    let mut response = response;
    let mut buffered = Vec::new();
    let mut output = Vec::new();
    let mut sequence = 0_u64;
    let mut completion = None;
    let execution = async {
        while let Some(chunk) = response.chunk().await? {
            buffered.extend_from_slice(&chunk);
            anyhow::ensure!(
                buffered.len() <= 256 * 1024,
                "Ollama emitted an oversized frame"
            );
            while let Some(newline) = buffered.iter().position(|byte| *byte == b'\n') {
                let line = buffered.drain(..=newline).collect::<Vec<_>>();
                if line[..line.len().saturating_sub(1)]
                    .iter()
                    .all(u8::is_ascii_whitespace)
                {
                    continue;
                }
                completion = process_ollama_line(
                    send,
                    request,
                    &line[..line.len().saturating_sub(1)],
                    &mut output,
                    &mut sequence,
                )
                .await?;
                if completion.is_some() {
                    break;
                }
            }
            if completion.is_some() {
                break;
            }
        }
        if completion.is_none() && !buffered.iter().all(u8::is_ascii_whitespace) {
            completion =
                process_ollama_line(send, request, &buffered, &mut output, &mut sequence).await?;
        }
        completion.ok_or_else(|| anyhow::anyhow!("Ollama stream ended without a terminal frame"))
    }
    .await;

    let finished_at = Utc::now();
    let (state, finish_reason, usage, error) = match execution {
        Ok(completion) => (
            JobState::Succeeded,
            Some(completion.finish_reason),
            completion.usage,
            None,
        ),
        Err(error) => (
            JobState::Failed,
            None,
            None,
            Some(bounded_error(&error.to_string())),
        ),
    };
    let mut receipt = ModelExecutionReceiptV1 {
        schema: ModelExecutionReceiptV1::SCHEMA.into(),
        receipt_id: Uuid::now_v7(),
        lease_id: request.lease.lease_id,
        session_id: request.lease.session_id,
        request_id: request.request_id,
        node_id: request.lease.node_id,
        state,
        started_at,
        finished_at,
        output_digest: format!("sha256:{}", hex::encode(Sha256::digest(&output))),
        output_bytes: output.len() as u64,
        usage,
        error: error.clone(),
        signature: String::new(),
    };
    rampage_policy::sign_model_receipt(signing_key, &mut receipt);
    let frame = ModelInvocationFrameV1 {
        schema: ModelInvocationFrameV1::SCHEMA.into(),
        request_id: request.request_id,
        sequence,
        kind: if state == JobState::Succeeded {
            ModelInvocationFrameKind::Complete
        } else {
            ModelInvocationFrameKind::Error
        },
        content: String::new(),
        finish_reason,
        error,
        receipt: Some(receipt),
    };
    rampage_mesh::write_model_frame(send, &frame).await
}

async fn process_ollama_line(
    send: &mut iroh::endpoint::SendStream,
    request: &ModelInvocationRequestV1,
    line: &[u8],
    output: &mut Vec<u8>,
    sequence: &mut u64,
) -> anyhow::Result<Option<OllamaCompletion>> {
    let chunk: OllamaChatChunk = serde_json::from_slice(line)?;
    if let Some(error) = chunk.error {
        anyhow::bail!("Ollama stream failed: {}", bounded_error(&error));
    }
    if let Some(model) = chunk.model {
        anyhow::ensure!(
            model == request.lease.model_id,
            "Ollama response model differs from its lease"
        );
    }
    if let Some(content) = chunk.message.map(|message| message.content)
        && !content.is_empty()
    {
        anyhow::ensure!(
            output.len().saturating_add(content.len()) <= MAX_MODEL_OUTPUT_BYTES as usize,
            "model output exceeded the bounded response size"
        );
        output.extend_from_slice(content.as_bytes());
        for content in utf8_chunks(&content, 16 * 1024) {
            let frame = ModelInvocationFrameV1 {
                schema: ModelInvocationFrameV1::SCHEMA.into(),
                request_id: request.request_id,
                sequence: *sequence,
                kind: ModelInvocationFrameKind::Delta,
                content: content.to_string(),
                finish_reason: None,
                error: None,
                receipt: None,
            };
            rampage_mesh::write_model_frame(send, &frame).await?;
            *sequence = sequence.saturating_add(1);
        }
    }
    if !chunk.done {
        return Ok(None);
    }
    Ok(Some(OllamaCompletion {
        finish_reason: chunk.done_reason.unwrap_or_else(|| "stop".into()),
        usage: chunk.prompt_eval_count.zip(chunk.eval_count).map(
            |(prompt_tokens, completion_tokens)| ModelUsageV1 {
                prompt_tokens,
                completion_tokens,
            },
        ),
    }))
}

async fn bounded_http_error(mut response: reqwest::Response) -> String {
    let mut bytes = Vec::new();
    while bytes.len() < 4_096 {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = 4_096 - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            _ => break,
        }
    }
    bounded_error(&String::from_utf8_lossy(&bytes))
}

async fn bounded_json(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= max_bytes,
            "loopback service response exceeded its bounded size"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn bounded_error(error: &str) -> String {
    error.chars().take(512).collect()
}

fn utf8_chunks(value: &str, max_bytes: usize) -> Vec<&str> {
    if value.len() <= max_bytes {
        return vec![value];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let mut end = (start + max_bytes).min(value.len());
        while end > start && !value.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = value[start..]
                .char_indices()
                .nth(1)
                .map_or(value.len(), |(offset, _)| start + offset);
        }
        chunks.push(&value[start..end]);
        start = end;
    }
    chunks
}

fn load_or_create_secret(path: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    load_or_create_secret_bytes(path, "agent storage secret")
}

fn load_or_create_secret_bytes(path: &std::path::Path, label: &str) -> anyhow::Result<[u8; 32]> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(_) => return read_secret_file(path, label),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut bytes = [0_u8; 32];
    SysRng.try_fill_bytes(&mut bytes)?;
    let encoded = hex::encode(bytes);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
            sync_parent_directory(path)?;
            Ok(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_secret_file(path, label)
        }
        Err(error) => Err(error.into()),
    }
}

fn read_secret_file(path: &std::path::Path, label: &str) -> anyhow::Result<[u8; 32]> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink() && metadata.is_file(),
        "{label} must be a regular non-symlink file"
    );
    anyhow::ensure!(metadata.len() <= 128, "{label} exceeds its size limit");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        if metadata.mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    let bytes = hex::decode(fs::read_to_string(path)?.trim())?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{label} must contain exactly 32 bytes"))
}

#[cfg(unix)]
fn sync_parent_directory(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

impl MeshController {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> anyhow::Result<rampage_protocol::MeshControlResponseV1> {
        self.request_with_deadline(method, path, body, rampage_mesh::CONTROL_REQUEST_DEADLINE)
    }

    fn request_with_deadline(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        deadline: std::time::Duration,
    ) -> anyhow::Result<rampage_protocol::MeshControlResponseV1> {
        let request = MeshControlRequestV1 {
            schema: MeshControlRequestV1::SCHEMA.into(),
            request_id: Uuid::now_v7(),
            method: method.into(),
            path: path.into(),
            body,
        };
        let local_sent_at = Utc::now();
        let started = Instant::now();
        let response = self
            .runtime
            .block_on(rampage_mesh::control_request_with_deadline(
                &self.endpoint,
                self.destination.clone(),
                &request,
                deadline,
            ))?;
        if let Some(controller_time) = response_controller_time(&response.body)
            && let Ok(offset) =
                estimated_controller_clock_offset(local_sent_at, started.elapsed(), controller_time)
            && offset.num_seconds().unsigned_abs() <= MAX_TRUSTED_CLOCK_OFFSET_SECONDS as u64
            && let Ok(mut current) = self.controller_clock_offset.lock()
        {
            *current = offset;
        }
        Ok(response)
    }

    fn controller_now(&self) -> chrono::DateTime<Utc> {
        let offset = self
            .controller_clock_offset
            .lock()
            .map(|value| *value)
            .unwrap_or_else(|_| Duration::zero());
        Utc::now() + offset
    }
}

fn response_controller_time(body: &serde_json::Value) -> Option<chrono::DateTime<Utc>> {
    body.get("controller_time")
        .or_else(|| body.get("generated_at"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn estimated_controller_clock_offset(
    local_sent_at: chrono::DateTime<Utc>,
    round_trip: std::time::Duration,
    controller_time: chrono::DateTime<Utc>,
) -> anyhow::Result<Duration> {
    let half_round_trip = Duration::from_std(round_trip / 2)?;
    Ok(controller_time - (local_sent_at + half_round_trip))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalIpv4Network {
    ip: Ipv4Addr,
    netmask: Ipv4Addr,
}

fn active_ipv4_networks() -> Vec<LocalIpv4Network> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| {
            interface.is_oper_up()
                && !interface.is_loopback()
                && !interface.is_p2p()
                && !interface.is_link_local()
        })
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(address)
                if !address.ip.is_unspecified() && address.netmask != Ipv4Addr::UNSPECIFIED =>
            {
                Some(LocalIpv4Network {
                    ip: address.ip,
                    netmask: address.netmask,
                })
            }
            _ => None,
        })
        .collect()
}

fn select_controller_destination(
    aggregate: iroh::EndpointAddr,
    candidates: Vec<iroh::EndpointAddr>,
) -> iroh::EndpointAddr {
    select_controller_destination_for(aggregate, candidates, &active_ipv4_networks())
}

fn select_controller_destination_for(
    aggregate: iroh::EndpointAddr,
    candidates: Vec<iroh::EndpointAddr>,
    local_networks: &[LocalIpv4Network],
) -> iroh::EndpointAddr {
    candidates
        .into_iter()
        .find(|candidate| {
            candidate.ip_addrs().any(|address| match address.ip() {
                IpAddr::V4(remote) => local_networks
                    .iter()
                    .any(|local| ipv4_is_same_subnet(local.ip, remote, local.netmask)),
                IpAddr::V6(_) => false,
            })
        })
        .unwrap_or(aggregate)
}

fn ipv4_is_same_subnet(local: Ipv4Addr, remote: Ipv4Addr, netmask: Ipv4Addr) -> bool {
    let mask = u32::from(netmask);
    if mask == 0 {
        return false;
    }
    u32::from(local) & mask == u32::from(remote) & mask
}

fn pin_from_invitation(invitation: &EnrollmentInviteV1) -> anyhow::Result<PinnedControllerV1> {
    let endpoint = invitation
        .controller_mesh
        .clone()
        .ok_or_else(|| anyhow::anyhow!("invitation omitted its controller mesh endpoint"))?;
    let governor_public_key = invitation
        .governor_public_key
        .clone()
        .ok_or_else(|| anyhow::anyhow!("invitation omitted its Governor public key"))?;
    let pin = PinnedControllerV1 {
        schema: "rampage.pinned-controller.v1".into(),
        endpoint,
        governor_public_key,
        enrolled_at: Utc::now(),
    };
    validate_controller_pin(&pin)?;
    Ok(pin)
}

fn validate_controller_pin(pin: &PinnedControllerV1) -> anyhow::Result<()> {
    anyhow::ensure!(
        pin.schema == "rampage.pinned-controller.v1",
        "unsupported controller pin schema"
    );
    anyhow::ensure!(
        pin.endpoint.endpoint_id.len() == 64
            && pin
                .endpoint
                .endpoint_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && pin.governor_public_key.len() == 64
            && pin
                .governor_public_key
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "controller pin contains a malformed identity"
    );
    rampage_policy::verify_pinned_mesh_endpoint_with_key(&pin.governor_public_key, &pin.endpoint)
        .map_err(|_| anyhow::anyhow!("stored controller route signature is invalid"))?;
    rampage_mesh::endpoint_addr_from_pinned_record(&pin.endpoint)?;
    Ok(())
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", Uuid::now_v7().simple()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        sync_parent_directory(path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn parse_enrollment_code(code: &str) -> anyhow::Result<(Uuid, String)> {
    let (invite_id, secret) = code
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("enrollment code has an invalid shape"))?;
    Ok((Uuid::parse_str(invite_id)?, secret.to_string()))
}

fn parse_device_kind(value: &str) -> anyhow::Result<DeviceKind> {
    let kind = match value {
        "desktop" => DeviceKind::Desktop,
        "laptop" => DeviceKind::Laptop,
        "server" => DeviceKind::Server,
        "steam_deck" => DeviceKind::SteamDeck,
        "phone" => DeviceKind::Phone,
        "tablet" => DeviceKind::Tablet,
        "console" => DeviceKind::Console,
        _ => anyhow::bail!("unsupported device kind {value}"),
    };
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Body,
        response::Response,
        routing::{get, post},
    };
    use rampage_protocol::{
        ARTIFACT_TRANSFER_CHUNK_BYTES, ArtifactTransferOperation, InstalledModelV1, ModelBackend,
        ModelChatMessageV1, ModelMemoryKind, ModelParallelism, ModelRuntimeOfferV1,
        ModelRuntimeStatus, ModelSessionLeaseV1, ResourceClass, ResourceQuantityV1,
    };

    async fn fake_tags() -> Json<serde_json::Value> {
        Json(serde_json::json!({"models": [{
            "name": "test:latest",
            "model": "test:latest",
            "size": 1024_u64,
            "digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }]}))
    }

    async fn fake_chat(Json(request): Json<serde_json::Value>) -> Response {
        assert_eq!(request.get("think"), Some(&serde_json::Value::Bool(true)));
        Response::builder()
            .header("content-type", "application/x-ndjson")
            .body(Body::from(concat!(
                "{\"model\":\"test:latest\",\"message\":{\"role\":\"assistant\",\"content\":\"hello \"},\"done\":false}\n",
                "{\"model\":\"test:latest\",\"message\":{\"role\":\"assistant\",\"content\":\"world\"},\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":2,\"eval_count\":2}\n"
            )))
            .unwrap()
    }

    #[test]
    fn signed_controller_relays_are_the_only_worker_relay_candidates() {
        let now = Utc::now();
        let mut record = MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: "ab".repeat(32),
            direct_addresses: Vec::new(),
            relay_urls: vec!["https://relay.example.test".into()],
            issued_at: now,
            expires_at: now + Duration::minutes(2),
            signature: "verified-before-helper".into(),
        };
        let config = mesh_config_for_controller(&record).unwrap();
        assert_eq!(
            config.mode,
            rampage_mesh::MeshMode::PrivateRelay {
                urls: vec!["https://relay.example.test".into()]
            }
        );
        assert_eq!(
            config.allowed_peer_keys,
            BTreeSet::from([record.endpoint_id.clone()])
        );

        record.relay_urls = vec!["http://relay.example.test".into()];
        assert!(mesh_config_for_controller(&record).is_err());
    }

    #[test]
    fn consumed_invitation_migrates_to_a_verified_pin_after_expiry() {
        let governor = SigningKey::from_bytes(&[91_u8; 32]);
        let controller = SigningKey::from_bytes(&[92_u8; 32]);
        let now = Utc::now();
        let mut endpoint = MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: hex::encode(controller.verifying_key().to_bytes()),
            direct_addresses: vec!["127.0.0.1:47838".into()],
            relay_urls: Vec::new(),
            issued_at: now - Duration::minutes(11),
            expires_at: now - Duration::minutes(1),
            signature: String::new(),
        };
        rampage_policy::sign_mesh_endpoint(&governor, &mut endpoint);
        let invitation = EnrollmentInviteV1 {
            schema: "rampage.enrollment-invite.v1".into(),
            invite_id: Uuid::now_v7(),
            enrollment_code: format!("{}.{}", Uuid::now_v7(), "a".repeat(32)),
            expires_at: now - Duration::minutes(1),
            controller_mesh: Some(endpoint),
            governor_public_key: Some(hex::encode(governor.verifying_key().to_bytes())),
        };
        assert!(
            rampage_policy::verify_mesh_endpoint_with_key(
                invitation.governor_public_key.as_deref().unwrap(),
                invitation.controller_mesh.as_ref().unwrap(),
            )
            .is_err()
        );
        let pin = pin_from_invitation(&invitation).unwrap();
        assert_eq!(pin.schema, "rampage.pinned-controller.v1");
        assert!(validate_controller_pin(&pin).is_ok());
    }

    fn model_offer(
        node_id: Uuid,
        worker: &iroh::Endpoint,
        model: InstalledModelV1,
    ) -> (ResourceOfferV1, ModelRuntimeOfferV1) {
        let now = Utc::now();
        let runtime = ModelRuntimeOfferV1 {
            schema: ModelRuntimeOfferV1::SCHEMA.into(),
            adapter: "rampage.ollama.v1".into(),
            backend: ModelBackend::LocalOllama,
            runtime_version: "test".into(),
            runtime_digest: "shipped-local:test".into(),
            compatibility_key: "ollama-test".into(),
            memory_kind: ModelMemoryKind::Host,
            available_model_bytes: 4096,
            supported_parallelism: BTreeSet::from([ModelParallelism::WholeModel]),
            status: ModelRuntimeStatus::ShippedLocal,
            installed_models: vec![model],
            certification_digest: None,
        };
        let address = worker.addr();
        let offer = ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id,
            observed_at: now,
            expires_at: now + Duration::minutes(2),
            resources: vec![ResourceQuantityV1 {
                class: ResourceClass::RamWorkingSet,
                capacity: 4096,
                available: 4096,
                unit: "byte".into(),
                labels: BTreeMap::new(),
            }],
            availability: AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 80,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::from(["rampage.ollama.v1".into()]),
            workload_capabilities: Vec::new(),
            model_runtimes: vec![runtime.clone()],
            link_benchmark: None,
            mesh_endpoint: Some(MeshEndpointRecordV1 {
                schema: MeshEndpointRecordV1::SCHEMA.into(),
                endpoint_id: address.id.to_string(),
                direct_addresses: address.ip_addrs().map(ToString::to_string).collect(),
                relay_urls: vec![],
                issued_at: now,
                expires_at: now + Duration::minutes(2),
                signature: "transport-test".into(),
            }),
            signature: "offer-test".into(),
        };
        (offer, runtime)
    }

    fn invocation(lease: ModelSessionLeaseV1) -> ModelInvocationRequestV1 {
        ModelInvocationRequestV1 {
            schema: ModelInvocationRequestV1::SCHEMA.into(),
            request_id: Uuid::now_v7(),
            lease,
            messages: vec![ModelChatMessageV1 {
                role: "user".into(),
                content: "hello".into(),
            }],
            max_output_tokens: 16,
            stream: true,
            temperature: None,
            top_p: None,
        }
    }

    async fn collect_frames(
        controller: &iroh::Endpoint,
        worker: &iroh::Endpoint,
        request: &ModelInvocationRequestV1,
    ) -> Vec<ModelInvocationFrameV1> {
        let mut stream = rampage_mesh::invoke_model(controller, worker.addr(), request)
            .await
            .unwrap();
        let mut frames = Vec::new();
        loop {
            let frame = stream.next_frame().await.unwrap();
            let terminal = matches!(
                frame.kind,
                ModelInvocationFrameKind::Complete | ModelInvocationFrameKind::Error
            );
            frames.push(frame);
            if terminal {
                return frames;
            }
        }
    }

    #[tokio::test]
    async fn model_gateway_streams_signed_receipt_and_rejects_replay_and_stale_epoch() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ollama_url = format!("http://{}", listener.local_addr().unwrap());
        let ollama_app = Router::new()
            .route("/api/tags", get(fake_tags))
            .route("/api/chat", post(fake_chat));
        let ollama = tokio::spawn(async move {
            axum::serve(listener, ollama_app).await.unwrap();
        });

        let governor =
            rampage_policy::Governor::ephemeral(rampage_policy::GovernorConfig::default());
        let governor_key = hex::encode(governor.verifying_key().to_bytes());
        let controller =
            rampage_mesh::bind_endpoint([71_u8; 32], &rampage_mesh::MeshConfig::default())
                .await
                .unwrap();
        let agent_key = SigningKey::from_bytes(&[72_u8; 32]);
        let worker =
            rampage_mesh::bind_endpoint(agent_key.to_bytes(), &rampage_mesh::MeshConfig::default())
                .await
                .unwrap();
        let node_id = Uuid::now_v7();
        let temp = tempfile::tempdir().unwrap();
        let store =
            std::sync::Arc::new(rampage_storage::CasStore::open(temp.path(), [73_u8; 32]).unwrap());
        let gateway = tokio::spawn(serve_worker_gateway(
            worker.clone(),
            WorkerGatewayConfig {
                controller_endpoint_id: controller.id().to_string(),
                node_id,
                governor_public_key: governor_key.clone(),
                store,
                signing_key: agent_key.clone(),
                ollama_base_url: ollama_url,
                data_dir: temp.path().to_path_buf(),
                remote_authority: std::sync::Arc::new(remote_assist::SessionAuthority::default()),
            },
        ));
        let model = InstalledModelV1 {
            schema: InstalledModelV1::SCHEMA.into(),
            model_id: "test:latest".into(),
            artifact_digest: format!("sha256:{}", "a".repeat(64)),
            artifact_size_bytes: 1024,
        };
        let (offer, runtime) = model_offer(node_id, &worker, model.clone());
        let lease = governor
            .authorize_model_session_at_epoch(
                &offer,
                &runtime,
                &model,
                &controller.id().to_string(),
                rampage_policy::ModelSessionLimits {
                    max_prompt_bytes: 1024,
                    max_output_tokens: 16,
                },
                5,
            )
            .unwrap();
        let request = invocation(lease);
        let frames = collect_frames(&controller, &worker, &request).await;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].content, "hello ");
        assert_eq!(frames[1].content, "world");
        assert_eq!(frames[2].kind, ModelInvocationFrameKind::Complete);
        let receipt = frames[2].receipt.as_ref().unwrap();
        let identity = NodeIdentityV1 {
            schema: "rampage.node-identity.v1".into(),
            node_id,
            owner_id: Uuid::now_v7(),
            display_name: "worker".into(),
            device_kind: DeviceKind::Desktop,
            platform: "test".into(),
            public_key: hex::encode(agent_key.verifying_key().to_bytes()),
            enrolled_at: Utc::now(),
            fencing_epoch: 0,
        };
        assert!(rampage_policy::verify_model_receipt(&identity, receipt).is_ok());
        assert_eq!(receipt.output_bytes, 11);
        assert_eq!(receipt.usage.as_ref().unwrap().completion_tokens, 2);

        let replay = collect_frames(&controller, &worker, &request).await;
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].kind, ModelInvocationFrameKind::Error);
        assert!(replay[0].receipt.is_none());

        let epoch_six = invocation(
            governor
                .authorize_model_session_at_epoch(
                    &offer,
                    &runtime,
                    &model,
                    &controller.id().to_string(),
                    rampage_policy::ModelSessionLimits {
                        max_prompt_bytes: 1024,
                        max_output_tokens: 16,
                    },
                    6,
                )
                .unwrap(),
        );
        assert_eq!(
            collect_frames(&controller, &worker, &epoch_six)
                .await
                .last()
                .unwrap()
                .kind,
            ModelInvocationFrameKind::Complete
        );
        let stale = invocation(
            governor
                .authorize_model_session_at_epoch(
                    &offer,
                    &runtime,
                    &model,
                    &controller.id().to_string(),
                    rampage_policy::ModelSessionLimits {
                        max_prompt_bytes: 1024,
                        max_output_tokens: 16,
                    },
                    5,
                )
                .unwrap(),
        );
        let stale_frames = collect_frames(&controller, &worker, &stale).await;
        assert_eq!(stale_frames.len(), 1);
        assert_eq!(stale_frames[0].kind, ModelInvocationFrameKind::Error);

        controller.close().await;
        worker.close().await;
        gateway.abort();
        ollama.abort();
    }

    #[tokio::test]
    async fn encrypted_mesh_put_resumes_after_worker_gateway_and_store_restart() {
        let governor =
            rampage_policy::Governor::ephemeral(rampage_policy::GovernorConfig::default());
        let governor_key = hex::encode(governor.verifying_key().to_bytes());
        let controller =
            rampage_mesh::bind_endpoint([81_u8; 32], &rampage_mesh::MeshConfig::default())
                .await
                .unwrap();
        let agent_key = SigningKey::from_bytes(&[82_u8; 32]);
        let worker =
            rampage_mesh::bind_endpoint(agent_key.to_bytes(), &rampage_mesh::MeshConfig::default())
                .await
                .unwrap();
        let node_id = Uuid::now_v7();
        let temp = tempfile::tempdir().unwrap();
        let store =
            std::sync::Arc::new(rampage_storage::CasStore::open(temp.path(), [83_u8; 32]).unwrap());
        let gateway = tokio::spawn(serve_worker_gateway(
            worker.clone(),
            WorkerGatewayConfig {
                controller_endpoint_id: controller.id().to_string(),
                node_id,
                governor_public_key: governor_key.clone(),
                store: store.clone(),
                signing_key: agent_key.clone(),
                ollama_base_url: "http://127.0.0.1:11434".into(),
                data_dir: temp.path().to_path_buf(),
                remote_authority: std::sync::Arc::new(remote_assist::SessionAuthority::default()),
            },
        ));
        let now = Utc::now();
        let offer = ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id,
            observed_at: now,
            expires_at: now + Duration::minutes(2),
            resources: vec![ResourceQuantityV1 {
                class: ResourceClass::ProtectedStore,
                capacity: 16 * 1024 * 1024,
                available: 16 * 1024 * 1024,
                unit: "byte".into(),
                labels: BTreeMap::new(),
            }],
            availability: AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 80,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::new(),
            workload_capabilities: Vec::new(),
            model_runtimes: Vec::new(),
            link_benchmark: None,
            mesh_endpoint: None,
            signature: "test".into(),
        };
        let payload = vec![57_u8; ARTIFACT_TRANSFER_CHUNK_BYTES as usize + 31];
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&payload)));
        let lease = governor
            .authorize_storage_at_epoch(
                &offer,
                &digest,
                payload.len() as u64,
                StorageClass::Protected,
                ArtifactTransferOperation::Put,
                7,
            )
            .unwrap();
        let session_id = Uuid::now_v7();
        let context = |destination: iroh::EndpointAddr, challenge_nonce: &str| {
            rampage_mesh::ArtifactTransferContext {
                destination,
                lease: lease.clone(),
                media_type: "application/octet-stream".into(),
                session_id,
                challenge_nonce: challenge_nonce.into(),
            }
        };
        let started =
            rampage_mesh::artifact_put(&controller, context(worker.addr(), "start-before-restart"))
                .await
                .unwrap();
        assert_eq!(started.missing_chunks, vec![0, 1]);
        let first = &payload[..ARTIFACT_TRANSFER_CHUNK_BYTES as usize];
        let after_first = rampage_mesh::artifact_put_chunk(
            &controller,
            context(worker.addr(), "chunk-before-restart"),
            0,
            format!("sha256:{}", hex::encode(Sha256::digest(first))),
            first,
        )
        .await
        .unwrap();
        assert_eq!(after_first.received_chunks, vec![0]);

        gateway.abort();
        worker.close().await;
        drop(store);

        let reopened =
            std::sync::Arc::new(rampage_storage::CasStore::open(temp.path(), [83_u8; 32]).unwrap());
        let restarted_worker =
            rampage_mesh::bind_endpoint(agent_key.to_bytes(), &rampage_mesh::MeshConfig::default())
                .await
                .unwrap();
        let restarted_gateway = tokio::spawn(serve_worker_gateway(
            restarted_worker.clone(),
            WorkerGatewayConfig {
                controller_endpoint_id: controller.id().to_string(),
                node_id,
                governor_public_key: governor_key,
                store: reopened.clone(),
                signing_key: agent_key.clone(),
                ollama_base_url: "http://127.0.0.1:11434".into(),
                data_dir: temp.path().to_path_buf(),
                remote_authority: std::sync::Arc::new(remote_assist::SessionAuthority::default()),
            },
        ));
        let resumed = rampage_mesh::artifact_put(
            &controller,
            context(restarted_worker.addr(), "resume-after-restart"),
        )
        .await
        .unwrap();
        assert_eq!(resumed.received_chunks, vec![0]);
        assert_eq!(resumed.missing_chunks, vec![1]);
        let second = &payload[ARTIFACT_TRANSFER_CHUNK_BYTES as usize..];
        rampage_mesh::artifact_put_chunk(
            &controller,
            context(restarted_worker.addr(), "chunk-after-restart"),
            1,
            format!("sha256:{}", hex::encode(Sha256::digest(second))),
            second,
        )
        .await
        .unwrap();
        let challenge = "commit-after-restart";
        let (artifact, receipt) =
            rampage_mesh::artifact_commit(&controller, context(restarted_worker.addr(), challenge))
                .await
                .unwrap();
        assert_eq!(artifact.digest, digest);
        assert_eq!(receipt.challenge_nonce, challenge);
        let identity = NodeIdentityV1 {
            schema: "rampage.node-identity.v1".into(),
            node_id,
            owner_id: Uuid::now_v7(),
            display_name: "restarted-worker".into(),
            device_kind: DeviceKind::Desktop,
            platform: "test".into(),
            public_key: hex::encode(agent_key.verifying_key().to_bytes()),
            enrolled_at: now,
            fencing_epoch: 7,
        };
        assert!(rampage_policy::verify_artifact_replica_receipt(&identity, &receipt).is_ok());
        assert_eq!(reopened.get(&digest).unwrap(), payload);

        restarted_gateway.abort();
        restarted_worker.close().await;
        controller.close().await;
    }

    #[test]
    fn bounded_json_loader_accepts_windows_utf8_bom_and_rejects_oversize() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("identity.json");
        let expected = IdentityIds {
            node_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
        };
        let mut encoded = vec![0xEF, 0xBB, 0xBF];
        encoded.extend(serde_json::to_vec(&expected).unwrap());
        fs::write(&path, encoded).unwrap();
        let parsed: IdentityIds = read_json_file_bounded(&path, 1024).unwrap();
        assert_eq!(parsed.node_id, expected.node_id);
        assert_eq!(parsed.owner_id, expected.owner_id);
        assert!(read_json_file_bounded::<IdentityIds>(&path, 8).is_err());
    }

    #[test]
    fn first_offer_is_never_blocked_by_the_link_probe() {
        let now = Instant::now();
        let offer_expires_at = Utc::now() + OFFER_LIFETIME;
        assert!(LINK_PROBE_REQUEST_DEADLINE * 5 < OFFER_LIFETIME.to_std().unwrap());
        assert!(!link_probe_due(false, now, now, None, offer_expires_at));
        assert!(link_probe_due(true, now, now, None, offer_expires_at));
        assert!(!link_probe_due(
            true,
            now,
            now + std::time::Duration::from_secs(1),
            None,
            offer_expires_at
        ));
        assert!(!link_probe_due(
            true,
            now,
            now,
            Some(offer_expires_at + Duration::seconds(1)),
            offer_expires_at
        ));
    }

    #[test]
    fn authenticated_controller_time_absorbs_ordinary_device_clock_skew() {
        let local_sent_at = chrono::DateTime::parse_from_rfc3339("2026-08-06T18:52:11Z")
            .unwrap()
            .with_timezone(&Utc);
        let controller_time = local_sent_at + Duration::seconds(38) + Duration::milliseconds(50);
        let offset = estimated_controller_clock_offset(
            local_sent_at,
            std::time::Duration::from_millis(100),
            controller_time,
        )
        .unwrap();
        assert_eq!(offset, Duration::seconds(38));

        let response = serde_json::json!({"controller_time": controller_time});
        assert_eq!(response_controller_time(&response), Some(controller_time));
    }

    #[test]
    fn signed_route_selection_prefers_the_best_same_lan_address() {
        let endpoint_id = iroh::SecretKey::from_bytes(&[83_u8; 32]).public();
        let make = |address: &str| {
            iroh::EndpointAddr::from_parts(
                endpoint_id,
                vec![iroh::TransportAddr::Ip(address.parse().unwrap())],
            )
        };
        let aggregate = iroh::EndpointAddr::from_parts(
            endpoint_id,
            vec![
                iroh::TransportAddr::Ip("100.98.141.113:58899".parse().unwrap()),
                iroh::TransportAddr::Ip("172.28.48.1:58899".parse().unwrap()),
                iroh::TransportAddr::Ip("192.168.86.32:58899".parse().unwrap()),
            ],
        );
        let selected = select_controller_destination_for(
            aggregate,
            vec![
                make("192.168.86.32:58899"),
                make("172.28.48.1:58899"),
                make("100.98.141.113:58899"),
            ],
            &[
                LocalIpv4Network {
                    ip: "192.168.86.47".parse().unwrap(),
                    netmask: "255.255.255.0".parse().unwrap(),
                },
                LocalIpv4Network {
                    ip: "100.98.141.114".parse().unwrap(),
                    netmask: "255.255.255.255".parse().unwrap(),
                },
            ],
        );
        assert_eq!(
            selected.ip_addrs().next().unwrap().to_string(),
            "192.168.86.32:58899"
        );
    }

    #[test]
    fn signed_route_selection_preserves_the_complete_fallback_off_lan() {
        let endpoint_id = iroh::SecretKey::from_bytes(&[84_u8; 32]).public();
        let aggregate = iroh::EndpointAddr::from_parts(
            endpoint_id,
            vec![
                iroh::TransportAddr::Ip("192.168.86.32:58899".parse().unwrap()),
                iroh::TransportAddr::Ip("203.0.113.20:58899".parse().unwrap()),
            ],
        );
        let selected = select_controller_destination_for(
            aggregate,
            vec![iroh::EndpointAddr::from_parts(
                endpoint_id,
                vec![iroh::TransportAddr::Ip(
                    "192.168.86.32:58899".parse().unwrap(),
                )],
            )],
            &[LocalIpv4Network {
                ip: "10.40.0.12".parse().unwrap(),
                netmask: "255.255.255.0".parse().unwrap(),
            }],
        );
        assert_eq!(selected.ip_addrs().count(), 2);
        assert_eq!(selected.id, endpoint_id);
    }

    #[test]
    fn signed_same_lan_selection_reaches_authenticated_address() {
        let dead_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let dead_port = dead_socket.local_addr().unwrap().port();
        let server_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_port = server_socket.local_addr().unwrap().port();
        drop(server_socket);
        assert_ne!(dead_port, server_port);

        let server_runtime = tokio::runtime::Runtime::new().unwrap();
        let server = server_runtime
            .block_on(rampage_mesh::bind_endpoint_on_port(
                [91_u8; 32],
                &rampage_mesh::MeshConfig::default(),
                server_port,
            ))
            .unwrap();
        let server_task_endpoint = server.clone();
        let server_task = server_runtime.spawn(async move {
            let connection = server_task_endpoint.accept().await.unwrap().await.unwrap();
            let (mut send, mut receive) = connection.accept_bi().await.unwrap();
            let bytes = receive.read_to_end(1024 * 1024).await.unwrap();
            let request: MeshControlRequestV1 = serde_json::from_slice(&bytes).unwrap();
            let response = rampage_protocol::MeshControlResponseV1 {
                schema: rampage_protocol::MeshControlResponseV1::SCHEMA.into(),
                request_id: request.request_id,
                status: 200,
                body: serde_json::json!({"status": "ready"}),
            };
            send.write_all(&serde_json::to_vec(&response).unwrap())
                .await
                .unwrap();
            send.finish().unwrap();
            let _ = send.stopped().await;
        });

        let client_runtime = tokio::runtime::Runtime::new().unwrap();
        let client = client_runtime
            .block_on(rampage_mesh::bind_endpoint(
                [92_u8; 32],
                &rampage_mesh::MeshConfig::default(),
            ))
            .unwrap();
        let candidates = vec![
            iroh::EndpointAddr::from_parts(
                server.id(),
                vec![iroh::TransportAddr::Ip(
                    format!("127.0.1.1:{dead_port}").parse().unwrap(),
                )],
            ),
            iroh::EndpointAddr::from_parts(
                server.id(),
                vec![iroh::TransportAddr::Ip(
                    format!("127.0.0.1:{server_port}").parse().unwrap(),
                )],
            ),
        ];
        let aggregate = iroh::EndpointAddr::from_parts(
            server.id(),
            vec![
                iroh::TransportAddr::Ip(format!("127.0.1.1:{dead_port}").parse().unwrap()),
                iroh::TransportAddr::Ip(format!("127.0.0.1:{server_port}").parse().unwrap()),
            ],
        );
        let destination = select_controller_destination_for(
            aggregate,
            candidates,
            &[LocalIpv4Network {
                ip: "127.0.0.2".parse().unwrap(),
                netmask: "255.255.255.0".parse().unwrap(),
            }],
        );
        let controller = MeshController {
            runtime: client_runtime,
            endpoint: client,
            controller_endpoint_id: server.id().to_string(),
            destination,
            controller_clock_offset: std::sync::Mutex::new(Duration::zero()),
        };
        let response = controller.request("GET", "/health", None).unwrap();
        assert_eq!(response.status, 200);

        drop(dead_socket);
        server_task.abort();
        server_runtime.block_on(server.close());
    }
}
