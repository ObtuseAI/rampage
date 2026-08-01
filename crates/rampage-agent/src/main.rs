mod discovery;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{Duration, Utc};
use clap::Parser;
use ed25519_dalek::SigningKey;
use rampage_protocol::{
    ArtifactTransferOperation, ArtifactTransferResponseV1, AvailabilityV1, DeviceKind,
    EnrollmentInviteV1, EnrollmentRequestV1, ExecutionReceiptV1, LINK_BENCHMARK_TRANSFER_BYTES,
    LinkBenchmarkV1, MeshControlRequestV1, MeshEndpointRecordV1, NodeIdentityV1, ResourceOfferV1,
    StorageClass, WorkClaimV1,
};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    time::Instant,
};
use uuid::Uuid;

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
    let artifact_store = std::sync::Arc::new(rampage_storage::CasStore::open(
        data_dir.join("cas"),
        load_or_create_secret(&data_dir.join("storage.key"))?,
    )?);
    let identity_file = args.key_file.with_extension("identity.json");
    let (node_id, owner_id) =
        load_or_create_identity_ids(&identity_file, args.node_id, args.owner_id)?;
    let invitation = if let Some(path) = &args.invite_file {
        Some(serde_json::from_slice::<EnrollmentInviteV1>(&fs::read(
            path,
        )?)?)
    } else {
        None
    };
    let transport = ControllerTransport::new(
        &args.controller,
        invitation.as_ref(),
        &signing_key,
        &data_dir,
        node_id,
        artifact_store.clone(),
    )?;
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
    let discovered = discovery::discover(base_labels, &data_dir);
    let now = Utc::now();
    let mut adapters = BTreeSet::from([
        "rampage.echo.v1".into(),
        "rampage.hash.v1".into(),
        "rampage.eval-shard.v1".into(),
        "rampage.artifact-hash.v1".into(),
    ]);
    let has_ollama = discovery::ollama_available();
    if has_ollama {
        adapters.insert("rampage.ollama.v1".into());
    }
    let model_runtimes = match discovery::discover_model_runtimes(&discovered.resources, has_ollama)
    {
        Ok(profiles) => profiles,
        Err(error) => {
            eprintln!("model runtime profiles rejected; continuing fail-closed: {error}");
            Vec::new()
        }
    };
    adapters.extend(model_runtimes.iter().map(|profile| profile.adapter.clone()));
    let offer = ResourceOfferV1 {
        schema: "rampage.resource-offer.v1".into(),
        offer_id: Uuid::now_v7(),
        node_id,
        observed_at: now,
        expires_at: now + Duration::seconds(45),
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
        model_runtimes,
        link_benchmark: None,
        mesh_endpoint: transport.signed_worker_endpoint(
            &signing_key,
            now,
            now + Duration::seconds(45),
        ),
        signature: String::new(),
    };
    let mut offer = offer;
    rampage_policy::sign_offer(&signing_key, &mut offer);
    let enrollment_marker = args.key_file.with_extension("enrolled");
    let invitation_endpoint_id = invitation
        .as_ref()
        .and_then(|invite| invite.controller_mesh.as_ref())
        .map(|endpoint| endpoint.endpoint_id.as_str());
    let already_enrolled = invitation_endpoint_id.is_some_and(|endpoint_id| {
        fs::read_to_string(&enrollment_marker).is_ok_and(|saved| saved.trim() == endpoint_id)
    });
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
    if args.serve {
        loop {
            if data_dir.join("KILL").is_file() {
                return Ok(());
            }
            let now = Utc::now();
            offer.offer_id = Uuid::now_v7();
            offer.observed_at = now;
            offer.expires_at = now + Duration::seconds(45);
            if offer
                .link_benchmark
                .as_ref()
                .is_none_or(|benchmark| benchmark.expires_at < offer.expires_at)
            {
                match transport.measure_link(node_id, now) {
                    Ok(benchmark) => offer.link_benchmark = benchmark,
                    Err(error) => {
                        eprintln!(
                            "link benchmark unavailable; placement will stay conservative: {error}"
                        );
                        offer.link_benchmark = None;
                    }
                }
            }
            offer.mesh_endpoint =
                transport.signed_worker_endpoint(&signing_key, now, offer.expires_at);
            rampage_policy::sign_offer(&signing_key, &mut offer);
            transport.post_json("/v1/offers", &offer)?;
            let did_work = execute_one_work_item(
                &args,
                &transport,
                &identity,
                &signing_key,
                artifact_store.as_ref(),
            )?;
            // Drain an admitted queue promptly, but back off while idle so a donated machine does
            // not burn CPU polling. Offer freshness is maintained independently on every loop.
            std::thread::sleep(if did_work {
                std::time::Duration::from_millis(250)
            } else {
                std::time::Duration::from_secs(2)
            });
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
        let existing: IdentityIds = serde_json::from_slice(&fs::read(path)?)?;
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

fn load_or_create_key(path: &PathBuf) -> anyhow::Result<SigningKey> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.is_file() {
        let bytes = hex::decode(fs::read_to_string(path)?.trim())?;
        let key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("agent key must contain exactly 32 bytes"))?;
        return Ok(SigningKey::from_bytes(&key));
    }
    let mut key = [0_u8; 32];
    OsRng.fill_bytes(&mut key);
    let temporary = path.with_extension("key.tmp");
    fs::write(&temporary, hex::encode(key))?;
    fs::rename(temporary, path)?;
    Ok(SigningKey::from_bytes(&key))
}

enum ControllerTransport {
    Http { base: String, token: String },
    Mesh(MeshController),
}

struct MeshController {
    runtime: tokio::runtime::Runtime,
    endpoint: iroh::Endpoint,
    destination: iroh::EndpointAddr,
}

impl ControllerTransport {
    fn new(
        controller: &str,
        invitation: Option<&EnrollmentInviteV1>,
        signing_key: &SigningKey,
        data_dir: &std::path::Path,
        node_id: Uuid,
        artifact_store: std::sync::Arc<rampage_storage::CasStore>,
    ) -> anyhow::Result<Self> {
        let Some(invitation) = invitation else {
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
        anyhow::ensure!(
            invitation.schema == "rampage.enrollment-invite.v1"
                && invitation.expires_at > Utc::now(),
            "invite is expired or has an unsupported schema"
        );
        let mesh_record = invitation
            .controller_mesh
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("invite does not contain a mesh endpoint"))?;
        let governor_key = invitation
            .governor_public_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("invite does not contain a Governor public key"))?;
        rampage_policy::verify_mesh_endpoint_with_key(governor_key, mesh_record)
            .map_err(|_| anyhow::anyhow!("invite mesh endpoint signature is invalid"))?;
        let destination = rampage_mesh::endpoint_addr_from_record(mesh_record)?;
        let runtime = tokio::runtime::Runtime::new()?;
        let endpoint = runtime.block_on(rampage_mesh::bind_endpoint(
            signing_key.to_bytes(),
            &rampage_mesh::MeshConfig::default(),
        ))?;
        runtime.spawn(serve_artifact_gateway(
            endpoint.clone(),
            mesh_record.endpoint_id.clone(),
            node_id,
            governor_key.to_string(),
            artifact_store,
        ));
        Ok(Self::Mesh(MeshController {
            runtime,
            endpoint,
            destination,
        }))
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
            let response = mesh.request("GET", "/health", None)?;
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
        let upload_response = mesh.request(
            "POST",
            "/v1/benchmarks/link",
            Some(serde_json::json!({
                "node_id": node_id,
                "nonce": upload_nonce,
                "upload_base64": BASE64.encode(&probe),
                "download_bytes": 0
            })),
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
        let download_response = mesh.request(
            "POST",
            "/v1/benchmarks/link",
            Some(serde_json::json!({
                "node_id": node_id,
                "nonce": download_nonce,
                "upload_base64": "",
                "download_bytes": LINK_BENCHMARK_TRANSFER_BYTES
            })),
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
            controller_endpoint_id: mesh.destination.id.to_string(),
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
        match self {
            Self::Http { base, token } => {
                let response = reqwest::blocking::Client::new()
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
                let response = mesh.request("POST", path, Some(serde_json::to_value(body)?))?;
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
        match self {
            Self::Http { base, token } => Ok(reqwest::blocking::Client::new()
                .get(format!("{base}{path}"))
                .header("x-rampage-token", token)
                .send()?
                .error_for_status()?
                .json()?),
            Self::Mesh(mesh) => {
                let response = mesh.request("GET", path, None)?;
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

async fn serve_artifact_gateway(
    endpoint: iroh::Endpoint,
    controller_endpoint_id: String,
    node_id: Uuid,
    governor_public_key: String,
    store: std::sync::Arc<rampage_storage::CasStore>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let store = store.clone();
        let controller_endpoint_id = controller_endpoint_id.clone();
        let governor_public_key = governor_public_key.clone();
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else {
                return;
            };
            if connection.alpn() != rampage_mesh::ARTIFACT_ALPN
                || connection.remote_id().to_string() != controller_endpoint_id
            {
                connection.close(1_u8.into(), b"artifact peer denied");
                return;
            }
            while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                let store = store.clone();
                let governor_public_key = governor_public_key.clone();
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
                        store.accept_authority(
                            "governor",
                            request.lease.fencing_epoch,
                            &request.lease.nonce,
                            request.lease.expires_at,
                        )?;
                        match request.lease.operation {
                            ArtifactTransferOperation::Put => {
                                let required_replicas =
                                    if request.lease.storage_class == StorageClass::Protected {
                                        2
                                    } else {
                                        1
                                    };
                                let artifact = store.put(
                                    &payload,
                                    rampage_storage::PutOptions {
                                        media_type: request.media_type,
                                        storage_class: request.lease.storage_class,
                                        required_replicas,
                                    },
                                )?;
                                anyhow::ensure!(
                                    artifact.digest == request.lease.digest
                                        && artifact.size_bytes == request.lease.size_bytes,
                                    "stored artifact did not match its lease"
                                );
                                Ok((request.request_id, artifact, Vec::new()))
                            }
                            ArtifactTransferOperation::Get => {
                                let stored = store.head(&request.lease.digest)?;
                                anyhow::ensure!(
                                    stored.storage_class == request.lease.storage_class
                                        && stored.size_bytes == request.lease.size_bytes,
                                    "stored artifact metadata did not match its lease"
                                );
                                let payload = store.get(&request.lease.digest)?;
                                anyhow::ensure!(
                                    payload.len() as u64 == request.lease.size_bytes,
                                    "stored artifact size did not match its lease"
                                );
                                Ok((request.request_id, stored, payload))
                            }
                        }
                    }
                    .await;
                    let (response, payload) = match result {
                        Ok((request_id, artifact, payload)) => (
                            ArtifactTransferResponseV1 {
                                schema: ArtifactTransferResponseV1::SCHEMA.into(),
                                request_id,
                                status: 200,
                                artifact: Some(artifact),
                                payload_size: payload.len() as u64,
                                error: None,
                            },
                            payload,
                        ),
                        Err(error) => (
                            ArtifactTransferResponseV1 {
                                schema: ArtifactTransferResponseV1::SCHEMA.into(),
                                request_id: response_request_id,
                                status: 400,
                                artifact: None,
                                payload_size: 0,
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

fn load_or_create_secret(path: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.is_file() {
        let bytes = hex::decode(fs::read_to_string(path)?.trim())?;
        return bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret must contain exactly 32 bytes"));
    }
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, hex::encode(bytes))?;
    fs::rename(temporary, path)?;
    Ok(bytes)
}

impl MeshController {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> anyhow::Result<rampage_protocol::MeshControlResponseV1> {
        let request = MeshControlRequestV1 {
            schema: MeshControlRequestV1::SCHEMA.into(),
            request_id: Uuid::now_v7(),
            method: method.into(),
            path: path.into(),
            body,
        };
        self.runtime.block_on(rampage_mesh::control_request(
            &self.endpoint,
            self.destination.clone(),
            &request,
        ))
    }
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
