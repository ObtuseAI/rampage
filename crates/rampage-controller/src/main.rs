use anyhow::Context;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderName, Method, Request, StatusCode, header::CONTENT_TYPE},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rampage_controller::{
    AdmissionPolicy, ResourceReservation, choose_offer_with_topology, plan_model_session,
    plan_shard_set, score_offer_with_topology,
};
use rampage_ledger::{Ledger, LedgerEvent};
use rampage_mesh::{MeshConfig, MeshMode, MeshNode};
use rampage_policy::{
    Governor, GovernorConfig, verify_enrollment, verify_mesh_endpoint_with_key, verify_offer,
};
use rampage_protocol::{
    ArtifactRefV1, ArtifactTransferOperation, CapabilityLeaseV1, EnrollmentInviteV1,
    EnrollmentRequestV1, ExecutionReceiptV1, JobSpecV1, JobState, LINK_BENCHMARK_TRANSFER_BYTES,
    MAX_ARTIFACT_TRANSFER_BYTES, MeshControlRequestV1, MeshControlResponseV1, MeshEndpointRecordV1,
    ModelSessionRequestV1, NodeIdentityV1, ResourceOfferV1, ShardSetV1, StorageClass, WorkClaimV1,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;
use uuid::Uuid;

type ShardInputLocality = HashMap<(Uuid, Uuid), BTreeSet<String>>;

#[derive(Clone)]
struct AppState {
    ledger: Arc<Ledger>,
    governor: Arc<Governor>,
    offers: Arc<RwLock<HashMap<Uuid, ResourceOfferV1>>>,
    nodes: Arc<RwLock<HashMap<Uuid, NodeIdentityV1>>>,
    invites: Arc<RwLock<HashMap<Uuid, InviteRecord>>>,
    assignments: Arc<RwLock<HashMap<Uuid, Assignment>>>,
    idempotency: Arc<RwLock<HashMap<String, Uuid>>>,
    local_enrollment_enabled: bool,
    kill_latch_path: Arc<PathBuf>,
    mesh: Arc<MeshNode>,
    reservations: Arc<RwLock<Vec<ResourceReservation>>>,
    admission_policy: Arc<AdmissionPolicy>,
    completed_receipts: Arc<RwLock<HashMap<Uuid, Uuid>>>,
    shard_sets: Arc<RwLock<HashMap<Uuid, ShardSetRecord>>>,
    artifact_replicas: Arc<RwLock<HashMap<(String, Uuid), ArtifactRefV1>>>,
    artifact_store: Arc<rampage_storage::CasStore>,
    local_api_token: Arc<String>,
    admission_gate: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
struct InviteRecord {
    secret_hash: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone)]
struct Assignment {
    job: JobSpecV1,
    lease: CapabilityLeaseV1,
    claimed: bool,
}

#[derive(Clone)]
struct ShardSetRecord {
    spec: ShardSetV1,
    leases: Vec<CapabilityLeaseV1>,
}

#[derive(Debug, Serialize)]
struct Health {
    service: &'static str,
    version: &'static str,
    status: &'static str,
    authority: &'static str,
    kill_latch: bool,
    mesh_mode: &'static str,
    mesh_endpoint_id: String,
    mesh_sockets: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct EventQuery {
    after: Option<u64>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ClaimQuery {
    node_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ReceiptQuery {
    job_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactPutRequest {
    data_base64: String,
    media_type: String,
    storage_class: StorageClass,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactGetQuery {
    digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactReplicateRequest {
    digest: String,
    node_id: Uuid,
    media_type: String,
    storage_class: StorageClass,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactRetrieveRequest {
    digest: String,
    node_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkProbeRequest {
    node_id: Uuid,
    nonce: Uuid,
    upload_base64: String,
    download_bytes: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let data_dir = std::env::var_os("RAMPAGE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".rampage/runtime"));
    std::fs::create_dir_all(&data_dir)?;
    let ledger = Arc::new(Ledger::open(data_dir.join("controller.db"))?);
    ledger
        .verify()
        .context("refusing to start with an invalid evidence ledger")?;
    let address: SocketAddr = std::env::var("RAMPAGE_BIND")
        .unwrap_or_else(|_| "127.0.0.1:47831".into())
        .parse()?;
    anyhow::ensure!(
        address.ip().is_loopback(),
        "the controller API must remain loopback-only; use the authenticated mesh transport for remote nodes"
    );
    let governor = Arc::new(load_or_create_governor(
        &data_dir.join("governor.key"),
        GovernorConfig::default(),
    )?);
    let local_api_token = Arc::new(hex::encode(load_or_create_secret(
        &data_dir.join("controller.token"),
    )?));
    let (
        nodes,
        offers,
        invites,
        assignments,
        idempotency,
        reservations,
        completed_receipts,
        shard_sets,
        artifact_replicas,
    ) = restore_state(&ledger)?;
    let mesh_config = mesh_config_from_env(&nodes)?;
    let mesh = Arc::new(
        rampage_mesh::bind_node(
            load_or_create_secret(&data_dir.join("mesh.key"))?,
            &mesh_config,
        )
        .await?,
    );
    ledger.append(
        "mesh.started",
        &mesh.endpoint_id(),
        &json!({"mode": mesh.mode(), "sockets": mesh.bound_sockets()}),
    )?;
    let state = AppState {
        ledger,
        governor,
        offers: Arc::new(RwLock::new(offers)),
        nodes: Arc::new(RwLock::new(nodes)),
        invites: Arc::new(RwLock::new(invites)),
        assignments: Arc::new(RwLock::new(assignments)),
        idempotency: Arc::new(RwLock::new(idempotency)),
        local_enrollment_enabled: address.ip().is_loopback(),
        kill_latch_path: Arc::new(data_dir.join("KILL")),
        mesh,
        reservations: Arc::new(RwLock::new(reservations)),
        admission_policy: Arc::new(AdmissionPolicy::default()),
        completed_receipts: Arc::new(RwLock::new(completed_receipts)),
        shard_sets: Arc::new(RwLock::new(shard_sets)),
        artifact_replicas: Arc::new(RwLock::new(artifact_replicas)),
        artifact_store: Arc::new(rampage_storage::CasStore::open(
            data_dir.join("cas"),
            load_or_create_secret(&data_dir.join("storage.key"))?,
        )?),
        local_api_token: local_api_token.clone(),
        admission_gate: Arc::new(tokio::sync::Mutex::new(())),
    };
    let mesh_state = state.clone();
    let protected = Router::new()
        .route("/v1/stop", post(local_stop))
        .route("/v1/resume", post(local_resume))
        .route("/v1/enrollment/invites", post(create_invite))
        .route("/v1/nodes", get(list_nodes))
        .route("/v1/nodes/enroll", post(enroll_node))
        .route("/v1/offers", get(list_offers).post(register_offer))
        .route("/v1/jobs", post(submit_job))
        .route("/v1/jobs/plan", post(plan_job))
        .route("/v1/model-sessions/plan", post(plan_model_session_request))
        .route("/v1/shard-sets", post(submit_shard_set))
        .route("/v1/shard-sets/plan", post(plan_shard_set_request))
        .route("/v1/shard-sets/{set_id}", get(shard_set_status))
        .route("/v1/work/claim", get(claim_work))
        .route("/v1/receipts", get(list_receipts).post(submit_receipt))
        .route(
            "/v1/artifacts/put",
            post(put_artifact).layer(DefaultBodyLimit::max(
                (MAX_ARTIFACT_TRANSFER_BYTES as usize * 4 / 3) + 1024 * 1024,
            )),
        )
        .route("/v1/artifacts/get", get(get_artifact))
        .route("/v1/artifacts/replicate", post(replicate_artifact))
        .route("/v1/artifacts/retrieve", post(retrieve_artifact))
        .route("/v1/benchmarks/link", post(link_probe))
        .route("/v1/governor/key", get(governor_key))
        .route("/v1/projects/discover", post(discover_project))
        .route("/v1/events", get(events))
        .route_layer(middleware::from_fn_with_state(
            local_api_token,
            require_local_token,
        ));
    let app = Router::new()
        .route("/health", get(health))
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list([
                    "http://localhost:1420".parse().expect("static origin"),
                    "http://127.0.0.1:1420".parse().expect("static origin"),
                    "tauri://localhost".parse().expect("static origin"),
                    "http://tauri.localhost".parse().expect("static origin"),
                ]))
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([CONTENT_TYPE, HeaderName::from_static("x-rampage-token")]),
        )
        .with_state(state);
    info!(%address, "Rampage controller listening");
    let listener = tokio::net::TcpListener::bind(address).await?;
    tokio::spawn(serve_mesh_gateway(listener.local_addr()?, mesh_state));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn create_invite(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<EnrollmentInviteV1>), (StatusCode, Json<Value>)> {
    if !state.local_enrollment_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "local enrollment is disabled on non-loopback binds"})),
        ));
    }
    let invite_id = Uuid::now_v7();
    let secret = Uuid::new_v4().simple().to_string();
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(10);
    state.invites.write().map_err(lock_error)?.insert(
        invite_id,
        InviteRecord {
            secret_hash: hash_secret(&secret),
            expires_at,
        },
    );
    state
        .ledger
        .append(
            "enrollment.invite.created",
            &invite_id.to_string(),
            &json!({"expires_at": expires_at, "secret_hash": hash_secret(&secret)}),
        )
        .map_err(internal_error)?;
    let endpoint_address = state.mesh.endpoint_addr();
    let mut controller_mesh = MeshEndpointRecordV1 {
        schema: MeshEndpointRecordV1::SCHEMA.into(),
        endpoint_id: endpoint_address.id.to_string(),
        direct_addresses: endpoint_address
            .ip_addrs()
            .map(ToString::to_string)
            .collect(),
        relay_urls: endpoint_address
            .relay_urls()
            .map(ToString::to_string)
            .collect(),
        issued_at: chrono::Utc::now(),
        expires_at,
        signature: String::new(),
    };
    state.governor.sign_mesh_endpoint(&mut controller_mesh);
    Ok((
        StatusCode::CREATED,
        Json(EnrollmentInviteV1 {
            schema: "rampage.enrollment-invite.v1".into(),
            invite_id,
            enrollment_code: format!("{invite_id}.{secret}"),
            expires_at,
            controller_mesh: Some(controller_mesh),
            governor_public_key: Some(hex::encode(state.governor.verifying_key().to_bytes())),
        }),
    ))
}

async fn enroll_node(
    State(state): State<AppState>,
    Json(request): Json<EnrollmentRequestV1>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if request.schema != "rampage.enrollment-request.v1" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported enrollment schema"})),
        ));
    }
    verify_enrollment(&request).map_err(|error| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let record = state
        .invites
        .write()
        .map_err(lock_error)?
        .remove(&request.invite_id)
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unknown or already consumed invite"})),
            )
        })?;
    if record.expires_at <= chrono::Utc::now()
        || record.secret_hash != hash_secret(&request.enrollment_code)
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "expired or invalid enrollment code"})),
        ));
    }
    let identity = request.identity;
    state
        .ledger
        .append(
            "enrollment.invite.consumed",
            &request.invite_id.to_string(),
            &json!({"node_id": identity.node_id}),
        )
        .map_err(internal_error)?;
    state
        .nodes
        .write()
        .map_err(lock_error)?
        .insert(identity.node_id, identity.clone());
    state
        .ledger
        .append("node.enrolled", &identity.node_id.to_string(), &identity)
        .map_err(internal_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"node_id": identity.node_id, "fencing_epoch": identity.fencing_epoch})),
    ))
}

async fn list_nodes(
    State(state): State<AppState>,
) -> Result<Json<Vec<NodeIdentityV1>>, (StatusCode, Json<Value>)> {
    Ok(Json(
        state
            .nodes
            .read()
            .map_err(lock_error)?
            .values()
            .cloned()
            .collect(),
    ))
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    let killed = state.kill_latch_path.is_file();
    Json(Health {
        service: "rampage-controller",
        version: env!("CARGO_PKG_VERSION"),
        status: if killed { "stopped" } else { "ready" },
        authority: "non-agentic-governor",
        kill_latch: killed,
        mesh_mode: state.mesh.mode(),
        mesh_endpoint_id: state.mesh.endpoint_id(),
        mesh_sockets: state
            .mesh
            .bound_sockets()
            .into_iter()
            .map(|socket| socket.to_string())
            .collect(),
    })
}

async fn require_local_token(
    State(expected): State<Arc<String>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let supplied = request
        .headers()
        .get("x-rampage-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let expected_digest = Sha256::digest(expected.as_bytes());
    let supplied_digest = Sha256::digest(supplied.as_bytes());
    if expected_digest != supplied_digest {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "valid local Rampage token required"})),
        )
            .into_response();
    }
    next.run(request).await
}

async fn local_stop(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    std::fs::write(state.kill_latch_path.as_ref(), b"owner-stop-v1\n").map_err(internal_error)?;
    state
        .ledger
        .append(
            "fabric.owner_stop",
            "local-fabric",
            &json!({"source": "local-api"}),
        )
        .map_err(internal_error)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"stopped": true}))))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeRequest {
    confirmation: String,
}

async fn local_resume(
    State(state): State<AppState>,
    Json(request): Json<ResumeRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if request.confirmation != "OWNER_RESUME" {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "exact owner resume confirmation is required"})),
        ));
    }
    if state.kill_latch_path.is_file() {
        std::fs::remove_file(state.kill_latch_path.as_ref()).map_err(internal_error)?;
    }
    state
        .ledger
        .append(
            "fabric.owner_resume",
            "local-fabric",
            &json!({"source": "local-api", "explicit_confirmation": true}),
        )
        .map_err(internal_error)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"stopped": false}))))
}

async fn register_offer(
    State(state): State<AppState>,
    Json(offer): Json<ResourceOfferV1>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if offer.signature.is_empty() || offer.expires_at <= chrono::Utc::now() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid or expired offer"})),
        ));
    }
    let identity = state
        .nodes
        .read()
        .map_err(lock_error)?
        .get(&offer.node_id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "node is not enrolled"})),
            )
        })?;
    verify_offer(&identity, &offer).map_err(|error| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    if let Some(benchmark) = &offer.link_benchmark
        && !benchmark.is_valid_for(
            &state.mesh.endpoint_id(),
            offer.observed_at,
            offer.expires_at,
        )
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error": "link benchmark is stale, malformed, or targets another controller"}),
            ),
        ));
    }
    if let Some(endpoint) = &offer.mesh_endpoint {
        if endpoint.endpoint_id != identity.public_key {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "worker endpoint and enrolled identity differ"})),
            ));
        }
        verify_mesh_endpoint_with_key(&identity.public_key, endpoint).map_err(|error| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": error.to_string()})),
            )
        })?;
        if endpoint.expires_at != offer.expires_at {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "worker endpoint and resource offer expiries differ"})),
            ));
        }
    }
    if let Some(benchmark) = &offer.link_benchmark {
        let is_new_observation = state
            .offers
            .read()
            .map_err(lock_error)?
            .get(&offer.node_id)
            .and_then(|previous| previous.link_benchmark.as_ref())
            .is_none_or(|previous| previous.observed_at != benchmark.observed_at);
        if is_new_observation {
            state
                .ledger
                .append(
                    "network.benchmark.recorded",
                    &offer.node_id.to_string(),
                    benchmark,
                )
                .map_err(internal_error)?;
        }
    }
    state
        .ledger
        .append(
            "resource.offer.registered",
            &offer.node_id.to_string(),
            &offer,
        )
        .map_err(internal_error)?;
    state
        .offers
        .write()
        .map_err(lock_error)?
        .insert(offer.node_id, offer.clone());
    Ok((
        StatusCode::CREATED,
        Json(json!({"offer_id": offer.offer_id})),
    ))
}

async fn link_probe(
    State(_state): State<AppState>,
    Json(request): Json<LinkProbeRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let upload = BASE64
        .decode(request.upload_base64.as_bytes())
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "link probe upload is not valid base64"})),
            )
        })?;
    if upload.len() as u64 > LINK_BENCHMARK_TRANSFER_BYTES
        || request.download_bytes > LINK_BENCHMARK_TRANSFER_BYTES
    {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "link probe exceeds the bounded transfer size"})),
        ));
    }
    let download = vec![0xA5_u8; request.download_bytes as usize];
    Ok(Json(json!({
        "schema": "rampage.link-probe-response.v1",
        "node_id": request.node_id,
        "nonce": request.nonce,
        "upload_bytes": upload.len(),
        "upload_sha256": hex::encode(Sha256::digest(&upload)),
        "download_base64": BASE64.encode(&download),
        "download_sha256": hex::encode(Sha256::digest(&download))
    })))
}

async fn list_offers(
    State(state): State<AppState>,
) -> Result<Json<Vec<ResourceOfferV1>>, (StatusCode, Json<Value>)> {
    let now = chrono::Utc::now();
    let offers = state
        .offers
        .read()
        .map_err(lock_error)?
        .values()
        .filter(|offer| offer.expires_at > now)
        .cloned()
        .collect();
    Ok(Json(offers))
}

async fn submit_job(
    State(state): State<AppState>,
    Json(job): Json<JobSpecV1>,
) -> Result<(StatusCode, Json<CapabilityLeaseV1>), (StatusCode, Json<Value>)> {
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
        ));
    }
    job.validate_at(chrono::Utc::now()).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let _admission_guard = state.admission_gate.lock().await;
    if let Some(existing_job_id) = state
        .idempotency
        .read()
        .map_err(lock_error)?
        .get(&job.idempotency_key)
        .copied()
        && let Some(existing) = state
            .assignments
            .read()
            .map_err(lock_error)?
            .get(&existing_job_id)
    {
        if serde_json::to_value(&existing.job).map_err(internal_error)?
            != serde_json::to_value(&job).map_err(internal_error)?
        {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error": "job idempotency key is already bound to other data"})),
            ));
        }
        return Ok((StatusCode::OK, Json(existing.lease.clone())));
    }
    state
        .ledger
        .append("job.proposed", &job.job_id.to_string(), &job)
        .map_err(internal_error)?;
    let offers: Vec<_> = state
        .offers
        .read()
        .map_err(lock_error)?
        .values()
        .cloned()
        .collect();
    let reservations = state.reservations.read().map_err(lock_error)?.clone();
    let now = chrono::Utc::now();
    let local_inputs_by_node = input_locality(&state, &offers, &job)?;
    let Some((offer, score)) = choose_offer_with_topology(
        &job,
        &offers,
        &reservations,
        state.admission_policy.as_ref(),
        now,
        &local_inputs_by_node,
    ) else {
        state
            .ledger
            .append(
                "job.blocked",
                &job.job_id.to_string(),
                &json!({"reason": "no admissible offer"}),
            )
            .map_err(internal_error)?;
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "no admissible offer"})),
        ));
    };
    let lease = state
        .governor
        .authorize_job(&job, offer, offer.node_id)
        .map_err(|error| {
            (
                StatusCode::FORBIDDEN,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    stage_job_inputs(&state, offer, &job).await?;
    state
        .ledger
        .append(
            "lease.issued",
            &job.job_id.to_string(),
            &json!({"lease": lease, "placement": score}),
        )
        .map_err(internal_error)?;
    state
        .idempotency
        .write()
        .map_err(lock_error)?
        .insert(job.idempotency_key.clone(), job.job_id);
    state.assignments.write().map_err(lock_error)?.insert(
        job.job_id,
        Assignment {
            job: job.clone(),
            lease: lease.clone(),
            claimed: false,
        },
    );
    let mut reservation_book = state.reservations.write().map_err(lock_error)?;
    reservation_book.retain(|reservation| reservation.expires_at > chrono::Utc::now());
    reservation_book.extend(job.requests.iter().map(|request| ResourceReservation {
        job_id: job.job_id,
        node_id: lease.node_id,
        class: request.class,
        amount: request.minimum,
        expires_at: lease.expires_at,
    }));
    Ok((StatusCode::ACCEPTED, Json(lease)))
}

async fn submit_shard_set(
    State(state): State<AppState>,
    Json(set): Json<ShardSetV1>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
        ));
    }
    let now = chrono::Utc::now();
    set.validate_at(now).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let _admission_guard = state.admission_gate.lock().await;

    let existing = state
        .shard_sets
        .read()
        .map_err(lock_error)?
        .values()
        .find(|record| record.spec.idempotency_key == set.idempotency_key)
        .cloned();
    if let Some(existing) = existing {
        if serde_json::to_value(&existing.spec).map_err(internal_error)?
            != serde_json::to_value(&set).map_err(internal_error)?
        {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error": "shard-set idempotency key is already bound to other data"})),
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(json!({
                "schema": "rampage.shard-set-admission.v1",
                "set_id": existing.spec.set_id,
                "minimum_successes": existing.spec.minimum_successes,
                "leases": existing.leases,
                "all_admitted": true,
                "idempotent_replay": true
            })),
        ));
    }

    {
        let idempotency = state.idempotency.read().map_err(lock_error)?;
        if set.shards.iter().any(|job| {
            idempotency
                .get(&job.idempotency_key)
                .is_some_and(|existing_job_id| *existing_job_id != job.job_id)
        }) {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error": "a shard idempotency key is already bound to another job"})),
            ));
        }
    }

    let offers = state
        .offers
        .read()
        .map_err(lock_error)?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let reservations = state.reservations.read().map_err(lock_error)?.clone();
    let locality = shard_input_locality(&state, &offers, &set)?;
    let placements = plan_shard_set(
        &set,
        &offers,
        &reservations,
        state.admission_policy.as_ref(),
        now,
        &locality,
    )
    .map_err(|failure| {
        (
            StatusCode::CONFLICT,
            Json(json!({
                "error": failure.reason,
                "blocked_job_id": failure.blocked_job_id,
                "planned_shards": failure.planned_shards,
                "authority_issued": false
            })),
        )
    })?;

    for (job, placement) in set.shards.iter().zip(&placements) {
        let offer = offers
            .iter()
            .find(|offer| offer.node_id == placement.node_id)
            .ok_or_else(|| {
                (
                    StatusCode::CONFLICT,
                    Json(json!({"error": "planned offer disappeared before admission"})),
                )
            })?;
        state
            .governor
            .check_job(job, offer, placement.node_id)
            .map_err(|error| {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({
                        "error": error.to_string(),
                        "blocked_job_id": job.job_id,
                        "authority_issued": false
                    })),
                )
            })?;
    }

    // Input staging may populate disposable cache replicas, but no execution authority or durable
    // shard-set admission is recorded until every member has staged successfully.
    for (job, placement) in set.shards.iter().zip(&placements) {
        let offer = offers
            .iter()
            .find(|offer| offer.node_id == placement.node_id)
            .expect("placement references an offer from the fixed planning set");
        stage_job_inputs(&state, offer, job).await?;
    }

    let mut leases = Vec::with_capacity(set.shards.len());
    for (job, placement) in set.shards.iter().zip(&placements) {
        let offer = offers
            .iter()
            .find(|offer| offer.node_id == placement.node_id)
            .expect("placement references an offer from the fixed planning set");
        leases.push(
            state
                .governor
                .authorize_job(job, offer, placement.node_id)
                .map_err(|error| {
                    (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "error": format!("authority changed during staging: {error}"),
                            "blocked_job_id": job.job_id,
                            "authority_persisted": false
                        })),
                    )
                })?,
        );
    }

    state
        .ledger
        .append(
            "shard_set.admitted",
            &set.set_id.to_string(),
            &json!({"set": &set, "leases": &leases, "placements": &placements}),
        )
        .map_err(internal_error)?;

    {
        let mut idempotency = state.idempotency.write().map_err(lock_error)?;
        let mut assignments = state.assignments.write().map_err(lock_error)?;
        let mut reservation_book = state.reservations.write().map_err(lock_error)?;
        reservation_book.retain(|reservation| reservation.expires_at > now);
        for ((job, lease), placement) in set.shards.iter().zip(&leases).zip(&placements) {
            idempotency.insert(job.idempotency_key.clone(), job.job_id);
            assignments.insert(
                job.job_id,
                Assignment {
                    job: job.clone(),
                    lease: lease.clone(),
                    claimed: false,
                },
            );
            reservation_book.extend(job.requests.iter().map(|request| ResourceReservation {
                job_id: job.job_id,
                node_id: placement.node_id,
                class: request.class,
                amount: request.minimum,
                expires_at: lease.expires_at,
            }));
        }
    }
    state.shard_sets.write().map_err(lock_error)?.insert(
        set.set_id,
        ShardSetRecord {
            spec: set.clone(),
            leases: leases.clone(),
        },
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "schema": "rampage.shard-set-admission.v1",
            "set_id": set.set_id,
            "minimum_successes": set.minimum_successes,
            "leases": leases,
            "placements": placements,
            "all_admitted": true,
            "idempotent_replay": false
        })),
    ))
}

async fn plan_shard_set_request(
    State(state): State<AppState>,
    Json(set): Json<ShardSetV1>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let now = chrono::Utc::now();
    set.validate_at(now).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let offers = state
        .offers
        .read()
        .map_err(lock_error)?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    let reservations = state.reservations.read().map_err(lock_error)?.clone();
    let locality = shard_input_locality(&state, &offers, &set)?;
    let result = plan_shard_set(
        &set,
        &offers,
        &reservations,
        state.admission_policy.as_ref(),
        now,
        &locality,
    );
    Ok(Json(match result {
        Ok(placements) => json!({
            "schema": "rampage.shard-set-plan.v1",
            "set_id": set.set_id,
            "admissible": true,
            "all_or_nothing": true,
            "placements": placements,
            "mutated": false
        }),
        Err(failure) => json!({
            "schema": "rampage.shard-set-plan.v1",
            "set_id": set.set_id,
            "admissible": false,
            "all_or_nothing": true,
            "placements": [],
            "blocked_job_id": failure.blocked_job_id,
            "planned_shards_before_block": failure.planned_shards,
            "reason": failure.reason,
            "mutated": false
        }),
    }))
}

async fn shard_set_status(
    State(state): State<AppState>,
    Path(set_id): Path<Uuid>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let record = state
        .shard_sets
        .read()
        .map_err(lock_error)?
        .get(&set_id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "shard set is absent"})),
            )
        })?;
    let job_ids = record
        .spec
        .shards
        .iter()
        .map(|job| job.job_id)
        .collect::<BTreeSet<_>>();
    let mut receipts = HashMap::<Uuid, ExecutionReceiptV1>::new();
    for job_id in &job_ids {
        for event in state
            .ledger
            .events_for_subject(&job_id.to_string(), 100)
            .map_err(internal_error)?
        {
            if event.event_type == "job.receipted"
                && let Ok(receipt) = serde_json::from_value::<ExecutionReceiptV1>(event.payload)
            {
                receipts.insert(receipt.job_id, receipt);
            }
        }
    }
    let assignments = state.assignments.read().map_err(lock_error)?;
    let lease_nodes = record
        .leases
        .iter()
        .map(|lease| (lease.job_id, lease.node_id))
        .collect::<HashMap<_, _>>();
    let mut succeeded = 0_u32;
    let mut failed = 0_u32;
    let members = record
        .spec
        .shards
        .iter()
        .map(|job| {
            let (status, result, receipt_id) = if let Some(receipt) = receipts.get(&job.job_id) {
                match receipt.state {
                    JobState::Succeeded => succeeded += 1,
                    JobState::Failed => failed += 1,
                    _ => {}
                }
                (
                    match receipt.state {
                        JobState::Succeeded => "succeeded",
                        JobState::Failed => "failed",
                        _ => "ambiguous",
                    },
                    receipt.result.clone(),
                    Some(receipt.receipt_id),
                )
            } else if assignments
                .get(&job.job_id)
                .is_some_and(|assignment| assignment.claimed)
            {
                ("running", None, None)
            } else if assignments.contains_key(&job.job_id) {
                ("admitted", None, None)
            } else {
                ("ambiguous", None, None)
            };
            json!({
                "job_id": job.job_id,
                "node_id": lease_nodes.get(&job.job_id),
                "status": status,
                "receipt_id": receipt_id,
                "result": result
            })
        })
        .collect::<Vec<_>>();
    let terminal = succeeded + failed;
    let total = record.spec.shards.len() as u32;
    let status = if terminal == total && succeeded >= record.spec.minimum_successes {
        "succeeded"
    } else if terminal == total {
        "failed"
    } else {
        "running"
    };
    Ok(Json(json!({
        "schema": "rampage.shard-set-status.v1",
        "set_id": set_id,
        "status": status,
        "total": total,
        "succeeded": succeeded,
        "failed": failed,
        "terminal": terminal,
        "minimum_successes": record.spec.minimum_successes,
        "threshold_met": succeeded >= record.spec.minimum_successes,
        "threshold_still_possible": succeeded + (total - terminal) >= record.spec.minimum_successes,
        "members": members
    })))
}

fn shard_input_locality(
    state: &AppState,
    offers: &[ResourceOfferV1],
    set: &ShardSetV1,
) -> Result<ShardInputLocality, (StatusCode, Json<Value>)> {
    let mut locality = HashMap::new();
    for job in &set.shards {
        for (node_id, digests) in input_locality(state, offers, job)? {
            locality.insert((job.job_id, node_id), digests);
        }
    }
    Ok(locality)
}

async fn plan_job(
    State(state): State<AppState>,
    Json(job): Json<JobSpecV1>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    job.validate_at(chrono::Utc::now()).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let offers: Vec<_> = state
        .offers
        .read()
        .map_err(lock_error)?
        .values()
        .cloned()
        .collect();
    let reservations = state.reservations.read().map_err(lock_error)?.clone();
    let now = chrono::Utc::now();
    let local_inputs_by_node = input_locality(&state, &offers, &job)?;
    let scores: Vec<_> = offers
        .iter()
        .map(|offer| {
            let local = local_inputs_by_node
                .get(&offer.node_id)
                .cloned()
                .unwrap_or_default();
            score_offer_with_topology(
                &job,
                offer,
                &reservations,
                state.admission_policy.as_ref(),
                now,
                &local,
            )
        })
        .collect();
    let selected_node = choose_offer_with_topology(
        &job,
        &offers,
        &reservations,
        state.admission_policy.as_ref(),
        now,
        &local_inputs_by_node,
    )
    .map(|(offer, _)| offer.node_id);
    Ok(Json(json!({
        "schema": "rampage.placement-plan.v1",
        "job_id": job.job_id,
        "selected_node": selected_node,
        "scores": scores,
        "mutated": false
    })))
}

async fn plan_model_session_request(
    State(state): State<AppState>,
    Json(request): Json<ModelSessionRequestV1>,
) -> Result<Json<rampage_controller::ModelSessionPlan>, (StatusCode, Json<Value>)> {
    let now = chrono::Utc::now();
    request.validate_at(now).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let offers = state
        .offers
        .read()
        .map_err(lock_error)?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    Ok(Json(plan_model_session(&request, &offers, now)))
}

fn input_locality(
    state: &AppState,
    offers: &[ResourceOfferV1],
    job: &JobSpecV1,
) -> Result<HashMap<Uuid, BTreeSet<String>>, (StatusCode, Json<Value>)> {
    let replicas = state.artifact_replicas.read().map_err(lock_error)?;
    let all_inputs: BTreeSet<_> = job
        .inputs
        .iter()
        .map(|input| input.digest.clone())
        .collect();
    Ok(offers
        .iter()
        .map(|offer| {
            let local = if offer.mesh_endpoint.is_none() {
                all_inputs.clone()
            } else {
                job.inputs
                    .iter()
                    .filter(|input| replicas.contains_key(&(input.digest.clone(), offer.node_id)))
                    .map(|input| input.digest.clone())
                    .collect()
            };
            (offer.node_id, local)
        })
        .collect())
}

async fn governor_key(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "schema": "rampage.governor-key.v1",
        "public_key": hex::encode(state.governor.verifying_key().to_bytes())
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverProjectRequest {
    path: PathBuf,
}

async fn discover_project(
    State(state): State<AppState>,
    Json(request): Json<DiscoverProjectRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let twin = rampage_project::discover_project(&request.path).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    state
        .ledger
        .append("project.discovered", &twin.fingerprint, &twin)
        .map_err(internal_error)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(twin).map_err(internal_error)?),
    ))
}

async fn claim_work(
    State(state): State<AppState>,
    Query(query): Query<ClaimQuery>,
) -> Result<Json<Option<WorkClaimV1>>, (StatusCode, Json<Value>)> {
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
        ));
    }
    if !state
        .nodes
        .read()
        .map_err(lock_error)?
        .contains_key(&query.node_id)
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "node is not enrolled"})),
        ));
    }
    let mut assignments = state.assignments.write().map_err(lock_error)?;
    let Some(assignment) = assignments
        .values_mut()
        .filter(|assignment| {
            !assignment.claimed
                && assignment.lease.node_id == query.node_id
                && assignment
                    .lease
                    .is_active_at(chrono::Utc::now(), assignment.lease.fencing_epoch)
        })
        .min_by_key(|assignment| assignment.lease.issued_at)
    else {
        return Ok(Json(None));
    };
    assignment.claimed = true;
    let claim = WorkClaimV1 {
        schema: WorkClaimV1::SCHEMA.into(),
        job: assignment.job.clone(),
        lease: assignment.lease.clone(),
        governor_public_key: hex::encode(state.governor.verifying_key().to_bytes()),
    };
    state
        .ledger
        .append("job.claimed", &claim.job.job_id.to_string(), &claim)
        .map_err(internal_error)?;
    Ok(Json(Some(claim)))
}

async fn list_receipts(
    State(state): State<AppState>,
    Query(query): Query<ReceiptQuery>,
) -> Result<Json<Vec<ExecutionReceiptV1>>, (StatusCode, Json<Value>)> {
    let receipts = state
        .ledger
        .events_for_subject(&query.job_id.to_string(), 100)
        .map_err(internal_error)?
        .into_iter()
        .filter(|event| event.event_type == "job.receipted")
        .filter_map(|event| serde_json::from_value(event.payload).ok())
        .collect();
    Ok(Json(receipts))
}

async fn submit_receipt(
    State(state): State<AppState>,
    Json(receipt): Json<ExecutionReceiptV1>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if receipt.schema != "rampage.execution-receipt.v1"
        || !matches!(receipt.state, JobState::Succeeded | JobState::Failed)
        || receipt.finished_at < receipt.started_at
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid terminal execution receipt"})),
        ));
    }
    let identity = state
        .nodes
        .read()
        .map_err(lock_error)?
        .get(&receipt.node_id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "node is not enrolled"})),
            )
        })?;
    rampage_policy::verify_receipt(&identity, &receipt).map_err(|error| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    if state
        .completed_receipts
        .read()
        .map_err(lock_error)?
        .get(&receipt.receipt_id)
        == Some(&receipt.job_id)
    {
        return Ok((
            StatusCode::OK,
            Json(json!({
                "receipt_id": receipt.receipt_id,
                "state": receipt.state,
                "duplicate": true
            })),
        ));
    }
    let assignment = state
        .assignments
        .read()
        .map_err(lock_error)?
        .get(&receipt.job_id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                Json(json!({"error": "receipt has no active assignment"})),
            )
        })?;
    if assignment.lease.lease_id != receipt.lease_id
        || assignment.lease.node_id != receipt.node_id
        || !assignment.claimed
    {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "receipt does not match the claimed lease"})),
        ));
    }
    for output in &receipt.outputs {
        let valid_digest = output.digest.strip_prefix("sha256:").is_some_and(|value| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if output.schema != "rampage.artifact-ref.v1"
            || !output.encrypted
            || !valid_digest
            || output.size_bytes > MAX_ARTIFACT_TRANSFER_BYTES
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "receipt contains an invalid output artifact"})),
            ));
        }
    }
    state
        .assignments
        .write()
        .map_err(lock_error)?
        .remove(&receipt.job_id);
    state
        .idempotency
        .write()
        .map_err(lock_error)?
        .remove(&assignment.job.idempotency_key);
    state
        .reservations
        .write()
        .map_err(lock_error)?
        .retain(|reservation| reservation.job_id != receipt.job_id);
    state
        .ledger
        .append("job.receipted", &receipt.job_id.to_string(), &receipt)
        .map_err(internal_error)?;
    state
        .completed_receipts
        .write()
        .map_err(lock_error)?
        .insert(receipt.receipt_id, receipt.job_id);
    for output in &receipt.outputs {
        state
            .artifact_replicas
            .write()
            .map_err(lock_error)?
            .insert((output.digest.clone(), receipt.node_id), output.clone());
        state
            .ledger
            .append(
                "artifact.output.recorded",
                &output.digest,
                &json!({"node_id": receipt.node_id, "job_id": receipt.job_id, "artifact": output}),
            )
            .map_err(internal_error)?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"receipt_id": receipt.receipt_id, "state": receipt.state})),
    ))
}

async fn put_artifact(
    State(state): State<AppState>,
    Json(request): Json<ArtifactPutRequest>,
) -> Result<(StatusCode, Json<ArtifactRefV1>), (StatusCode, Json<Value>)> {
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
        ));
    }
    if request.storage_class == StorageClass::Protected {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error": "protected artifacts require a remote replica; put as cache or scratch, then replicate as protected"}),
            ),
        ));
    }
    let payload = BASE64.decode(&request.data_base64).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "artifact payload is not valid base64"})),
        )
    })?;
    if payload.len() as u64 > MAX_ARTIFACT_TRANSFER_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "artifact exceeds the 64 MiB transfer limit"})),
        ));
    }
    let artifact = state
        .artifact_store
        .put(
            &payload,
            rampage_storage::PutOptions {
                media_type: request.media_type,
                storage_class: request.storage_class,
                required_replicas: 1,
            },
        )
        .map_err(internal_error)?;
    state
        .ledger
        .append("artifact.stored.local", &artifact.digest, &artifact)
        .map_err(internal_error)?;
    Ok((StatusCode::CREATED, Json(artifact)))
}

async fn get_artifact(
    State(state): State<AppState>,
    Query(query): Query<ArtifactGetQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let artifact = state.artifact_store.head(&query.digest).map_err(|error| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let payload = state
        .artifact_store
        .get(&query.digest)
        .map_err(internal_error)?;
    Ok(Json(json!({
        "schema": "rampage.artifact-payload.v1",
        "artifact": artifact,
        "data_base64": BASE64.encode(payload)
    })))
}

fn remote_offer(
    state: &AppState,
    node_id: Uuid,
) -> Result<(ResourceOfferV1, iroh::EndpointAddr), (StatusCode, Json<Value>)> {
    let offer = state
        .offers
        .read()
        .map_err(lock_error)?
        .get(&node_id)
        .filter(|offer| offer.expires_at > chrono::Utc::now())
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                Json(json!({"error": "node has no current resource offer"})),
            )
        })?;
    let endpoint_record = offer.mesh_endpoint.as_ref().ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            Json(json!({"error": "node does not advertise an artifact endpoint"})),
        )
    })?;
    let endpoint = rampage_mesh::endpoint_addr_from_record(endpoint_record).map_err(|error| {
        (
            StatusCode::CONFLICT,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    Ok((offer, endpoint))
}

async fn stage_job_inputs(
    state: &AppState,
    offer: &ResourceOfferV1,
    job: &JobSpecV1,
) -> Result<(), (StatusCode, Json<Value>)> {
    for input in &job.inputs {
        let local = state.artifact_store.head(&input.digest).map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("job input is unavailable: {error}")})),
            )
        })?;
        if local.size_bytes != input.size_bytes
            || local.media_type != input.media_type
            || local.storage_class != input.storage_class
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "job input metadata does not match the controller CAS"})),
            ));
        }
        let Some(endpoint_record) = &offer.mesh_endpoint else {
            continue;
        };
        if state
            .artifact_replicas
            .read()
            .map_err(lock_error)?
            .contains_key(&(input.digest.clone(), offer.node_id))
        {
            continue;
        }
        let endpoint =
            rampage_mesh::endpoint_addr_from_record(endpoint_record).map_err(|error| {
                (
                    StatusCode::CONFLICT,
                    Json(json!({"error": error.to_string()})),
                )
            })?;
        let payload = state
            .artifact_store
            .get(&input.digest)
            .map_err(internal_error)?;
        let storage_lease = state
            .governor
            .authorize_storage(
                offer,
                &input.digest,
                input.size_bytes,
                input.storage_class,
                ArtifactTransferOperation::Put,
            )
            .map_err(|error| {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"error": error.to_string()})),
                )
            })?;
        state
            .ledger
            .append(
                "storage.lease.issued",
                &storage_lease.lease_id.to_string(),
                &storage_lease,
            )
            .map_err(internal_error)?;
        let remote = rampage_mesh::artifact_put(
            &state.mesh.endpoint(),
            endpoint,
            storage_lease.clone(),
            input.media_type.clone(),
            &payload,
        )
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("input staging failed: {error}")})),
            )
        })?;
        state
            .artifact_replicas
            .write()
            .map_err(lock_error)?
            .insert((remote.digest.clone(), offer.node_id), remote.clone());
        state
            .ledger
            .append(
                "artifact.input.staged",
                &remote.digest,
                &json!({
                    "node_id": offer.node_id,
                    "job_id": job.job_id,
                    "artifact": remote,
                    "storage_lease_id": storage_lease.lease_id
                }),
            )
            .map_err(internal_error)?;
    }
    Ok(())
}

async fn replicate_artifact(
    State(state): State<AppState>,
    Json(request): Json<ArtifactReplicateRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
        ));
    }
    let source = state
        .artifact_store
        .head(&request.digest)
        .map_err(|error| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    let payload = state
        .artifact_store
        .get(&request.digest)
        .map_err(internal_error)?;
    let (offer, endpoint) = remote_offer(&state, request.node_id)?;
    let lease = state
        .governor
        .authorize_storage(
            &offer,
            &source.digest,
            source.size_bytes,
            request.storage_class,
            ArtifactTransferOperation::Put,
        )
        .map_err(|error| {
            (
                StatusCode::FORBIDDEN,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    state
        .ledger
        .append("storage.lease.issued", &lease.lease_id.to_string(), &lease)
        .map_err(internal_error)?;
    let remote_artifact = rampage_mesh::artifact_put(
        &state.mesh.endpoint(),
        endpoint,
        lease.clone(),
        request.media_type,
        &payload,
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    state.artifact_replicas.write().map_err(lock_error)?.insert(
        (remote_artifact.digest.clone(), request.node_id),
        remote_artifact.clone(),
    );
    state
        .ledger
        .append(
            "artifact.replicated",
            &remote_artifact.digest,
            &json!({
                "node_id": request.node_id,
                "artifact": remote_artifact,
                "storage_lease_id": lease.lease_id
            }),
        )
        .map_err(internal_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "artifact": remote_artifact,
            "node_id": request.node_id,
            "storage_lease_id": lease.lease_id
        })),
    ))
}

async fn retrieve_artifact(
    State(state): State<AppState>,
    Json(request): Json<ArtifactRetrieveRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let remote_artifact = state
        .artifact_replicas
        .read()
        .map_err(lock_error)?
        .get(&(request.digest.clone(), request.node_id))
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "no verified replica is recorded for that node"})),
            )
        })?;
    let (offer, endpoint) = remote_offer(&state, request.node_id)?;
    let lease = state
        .governor
        .authorize_storage(
            &offer,
            &remote_artifact.digest,
            remote_artifact.size_bytes,
            remote_artifact.storage_class,
            ArtifactTransferOperation::Get,
        )
        .map_err(|error| {
            (
                StatusCode::FORBIDDEN,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    state
        .ledger
        .append("storage.lease.issued", &lease.lease_id.to_string(), &lease)
        .map_err(internal_error)?;
    let (artifact, payload) = rampage_mesh::artifact_get(
        &state.mesh.endpoint(),
        endpoint,
        lease.clone(),
        remote_artifact.media_type.clone(),
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": error.to_string()})),
        )
    })?;
    let local = state
        .artifact_store
        .put(
            &payload,
            rampage_storage::PutOptions {
                media_type: artifact.media_type.clone(),
                storage_class: artifact.storage_class,
                required_replicas: if artifact.storage_class == StorageClass::Protected {
                    2
                } else {
                    1
                },
            },
        )
        .map_err(internal_error)?;
    if local.digest != artifact.digest {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": "retrieved artifact failed content-address verification"})),
        ));
    }
    state
        .ledger
        .append(
            "artifact.retrieved",
            &artifact.digest,
            &json!({"node_id": request.node_id, "artifact": artifact, "storage_lease_id": lease.lease_id}),
        )
        .map_err(internal_error)?;
    Ok(Json(json!({
        "schema": "rampage.artifact-payload.v1",
        "artifact": artifact,
        "node_id": request.node_id,
        "data_base64": BASE64.encode(payload)
    })))
}

async fn events(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Vec<LedgerEvent>>, (StatusCode, Json<Value>)> {
    state
        .ledger
        .events(query.after.unwrap_or(0), query.limit.unwrap_or(250))
        .map(Json)
        .map_err(internal_error)
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": error.to_string()})),
    )
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> (StatusCode, Json<Value>) {
    internal_error("controller state lock poisoned")
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

fn load_or_create_governor(
    path: &std::path::Path,
    config: GovernorConfig,
) -> anyhow::Result<Governor> {
    use ed25519_dalek::SigningKey;
    let key = SigningKey::from_bytes(&load_or_create_secret(path)?);
    Ok(Governor::from_signing_key(config, key))
}

fn load_or_create_secret(path: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    use rand::RngCore;
    if path.is_file() {
        let bytes = hex::decode(std::fs::read_to_string(path)?.trim())?;
        return bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret key must contain exactly 32 bytes"));
    }
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let temporary = path.with_extension("key.tmp");
    std::fs::write(&temporary, hex::encode(bytes))?;
    std::fs::rename(temporary, path)?;
    Ok(bytes)
}

fn mesh_config_from_env(nodes: &HashMap<Uuid, NodeIdentityV1>) -> anyhow::Result<MeshConfig> {
    let allowed_peer_keys = nodes.values().map(|node| node.public_key.clone()).collect();
    let mode = match std::env::var("RAMPAGE_PRIVATE_RELAYS") {
        Ok(value) if !value.trim().is_empty() => MeshMode::PrivateRelay {
            urls: value
                .split(';')
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string)
                .collect(),
        },
        _ => MeshMode::LocalOnly,
    };
    let config = MeshConfig {
        schema: "rampage.mesh-config.v1".into(),
        mode,
        allowed_peer_keys,
    };
    config.validate()?;
    Ok(config)
}

type RestoredState = (
    HashMap<Uuid, NodeIdentityV1>,
    HashMap<Uuid, ResourceOfferV1>,
    HashMap<Uuid, InviteRecord>,
    HashMap<Uuid, Assignment>,
    HashMap<String, Uuid>,
    Vec<ResourceReservation>,
    HashMap<Uuid, Uuid>,
    HashMap<Uuid, ShardSetRecord>,
    HashMap<(String, Uuid), ArtifactRefV1>,
);

fn restore_state(ledger: &Ledger) -> anyhow::Result<RestoredState> {
    let mut nodes = HashMap::new();
    let mut offers = HashMap::new();
    let mut invites = HashMap::new();
    let mut proposed_jobs: HashMap<Uuid, JobSpecV1> = HashMap::new();
    let mut assignments: HashMap<Uuid, Assignment> = HashMap::new();
    let mut completed_receipts = HashMap::new();
    let mut shard_sets = HashMap::new();
    let mut artifact_replicas = HashMap::new();
    let now = chrono::Utc::now();
    let mut after_sequence = 0_u64;
    loop {
        let events = ledger.events(after_sequence, 10_000)?;
        if events.is_empty() {
            break;
        }
        after_sequence = events.last().expect("non-empty ledger page").sequence;
        for event in events {
            match event.event_type.as_str() {
                "node.enrolled" => {
                    let identity: NodeIdentityV1 = serde_json::from_value(event.payload)?;
                    nodes.insert(identity.node_id, identity);
                }
                "resource.offer.registered" => {
                    let offer: ResourceOfferV1 = serde_json::from_value(event.payload)?;
                    if offer.expires_at > now {
                        offers.insert(offer.node_id, offer);
                    }
                }
                "enrollment.invite.created" => {
                    let Some(secret_hash) =
                        event.payload.get("secret_hash").and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let Some(expires_at) = event.payload.get("expires_at").and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at)?
                        .with_timezone(&chrono::Utc);
                    let invite_id = Uuid::parse_str(&event.subject_id)?;
                    if expires_at > now {
                        invites.insert(
                            invite_id,
                            InviteRecord {
                                secret_hash: secret_hash.into(),
                                expires_at,
                            },
                        );
                    }
                }
                "enrollment.invite.consumed" => {
                    if let Ok(invite_id) = Uuid::parse_str(&event.subject_id) {
                        invites.remove(&invite_id);
                    }
                }
                "job.proposed" => {
                    let job: JobSpecV1 = serde_json::from_value(event.payload)?;
                    proposed_jobs.insert(job.job_id, job);
                }
                "lease.issued" => {
                    let Some(lease_value) = event.payload.get("lease") else {
                        continue;
                    };
                    let lease: CapabilityLeaseV1 = serde_json::from_value(lease_value.clone())?;
                    if lease.expires_at > now
                        && let Some(job) = proposed_jobs.get(&lease.job_id).cloned()
                    {
                        assignments.insert(
                            lease.job_id,
                            Assignment {
                                job,
                                lease,
                                claimed: false,
                            },
                        );
                    }
                }
                "shard_set.admitted" => {
                    let Some(set_value) = event.payload.get("set") else {
                        continue;
                    };
                    let Some(leases_value) = event.payload.get("leases") else {
                        continue;
                    };
                    let set: ShardSetV1 = serde_json::from_value(set_value.clone())?;
                    let leases: Vec<CapabilityLeaseV1> =
                        serde_json::from_value(leases_value.clone())?;
                    let jobs = set
                        .shards
                        .iter()
                        .map(|job| (job.job_id, job.clone()))
                        .collect::<HashMap<_, _>>();
                    for lease in &leases {
                        if lease.expires_at > now
                            && let Some(job) = jobs.get(&lease.job_id).cloned()
                        {
                            assignments.insert(
                                lease.job_id,
                                Assignment {
                                    job,
                                    lease: lease.clone(),
                                    claimed: false,
                                },
                            );
                        }
                    }
                    shard_sets.insert(set.set_id, ShardSetRecord { spec: set, leases });
                }
                "job.claimed" => {
                    if let Ok(job_id) = Uuid::parse_str(&event.subject_id)
                        && let Some(assignment) = assignments.get_mut(&job_id)
                    {
                        assignment.claimed = true;
                    }
                }
                "job.receipted" => {
                    if let Ok(job_id) = Uuid::parse_str(&event.subject_id) {
                        assignments.remove(&job_id);
                        if let Ok(receipt) =
                            serde_json::from_value::<ExecutionReceiptV1>(event.payload)
                        {
                            completed_receipts.insert(receipt.receipt_id, receipt.job_id);
                        }
                    }
                }
                "artifact.replicated" | "artifact.input.staged" | "artifact.output.recorded" => {
                    let Some(node_id) = event
                        .payload
                        .get("node_id")
                        .and_then(Value::as_str)
                        .and_then(|value| Uuid::parse_str(value).ok())
                    else {
                        continue;
                    };
                    let Some(artifact_value) = event.payload.get("artifact") else {
                        continue;
                    };
                    let artifact: ArtifactRefV1 = serde_json::from_value(artifact_value.clone())?;
                    artifact_replicas.insert((artifact.digest.clone(), node_id), artifact);
                }
                _ => {}
            }
        }
    }
    let idempotency = assignments
        .values()
        .map(|assignment| {
            (
                assignment.job.idempotency_key.clone(),
                assignment.job.job_id,
            )
        })
        .collect();
    let reservations = assignments
        .values()
        .flat_map(|assignment| {
            assignment
                .job
                .requests
                .iter()
                .map(|request| ResourceReservation {
                    job_id: assignment.job.job_id,
                    node_id: assignment.lease.node_id,
                    class: request.class,
                    amount: request.minimum,
                    expires_at: assignment.lease.expires_at,
                })
        })
        .collect();
    Ok((
        nodes,
        offers,
        invites,
        assignments,
        idempotency,
        reservations,
        completed_receipts,
        shard_sets,
        artifact_replicas,
    ))
}

async fn serve_mesh_gateway(controller_address: SocketAddr, state: AppState) {
    let endpoint = state.mesh.endpoint();
    let controller = format!("http://{controller_address}");
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        let controller = controller.clone();
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else {
                return;
            };
            if connection.alpn() != rampage_mesh::CONTROL_ALPN {
                connection.close(1_u8.into(), b"unsupported protocol");
                return;
            }
            let peer = connection.remote_id().to_string();
            while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                let state = state.clone();
                let controller = controller.clone();
                let peer = peer.clone();
                tokio::spawn(async move {
                    let response = match receive.read_to_end(1024 * 1024).await {
                        Ok(bytes) => match serde_json::from_slice::<MeshControlRequestV1>(&bytes) {
                            Ok(request) => {
                                process_mesh_control(&state, &controller, &peer, request).await
                            }
                            Err(_) => mesh_error_response(
                                Uuid::nil(),
                                StatusCode::BAD_REQUEST,
                                "invalid mesh control request",
                            ),
                        },
                        Err(_) => mesh_error_response(
                            Uuid::nil(),
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "mesh control request exceeds one MiB",
                        ),
                    };
                    if let Ok(encoded) = serde_json::to_vec(&response) {
                        let _ = send.write_all(&encoded).await;
                        let _ = send.finish();
                    }
                });
            }
        });
    }
}

async fn process_mesh_control(
    state: &AppState,
    controller: &str,
    peer: &str,
    request: MeshControlRequestV1,
) -> MeshControlResponseV1 {
    if request.schema != MeshControlRequestV1::SCHEMA {
        return mesh_error_response(
            request.request_id,
            StatusCode::BAD_REQUEST,
            "unsupported mesh control schema",
        );
    }
    if let Err(reason) = authorize_mesh_control(state, peer, &request) {
        let _ = state.ledger.append(
            "mesh.request.denied",
            peer,
            &json!({"method": request.method, "path": request.path, "reason": reason}),
        );
        return mesh_error_response(request.request_id, StatusCode::FORBIDDEN, &reason);
    }
    let client = reqwest::Client::new();
    let url = format!("{controller}{}", request.path);
    let mut outgoing = match request.method.as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url),
        _ => {
            return mesh_error_response(
                request.request_id,
                StatusCode::METHOD_NOT_ALLOWED,
                "mesh method is not allowlisted",
            );
        }
    };
    outgoing = outgoing.header("x-rampage-token", state.local_api_token.as_str());
    if let Some(body) = &request.body {
        outgoing = outgoing.json(body);
    }
    match outgoing.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.bytes().await {
                Ok(bytes) if bytes.len() <= 1024 * 1024 => MeshControlResponseV1 {
                    schema: MeshControlResponseV1::SCHEMA.into(),
                    request_id: request.request_id,
                    status,
                    body: serde_json::from_slice(&bytes)
                        .unwrap_or_else(|_| json!({"error": "controller returned non-JSON"})),
                },
                _ => mesh_error_response(
                    request.request_id,
                    StatusCode::BAD_GATEWAY,
                    "controller response exceeded one MiB",
                ),
            }
        }
        Err(_) => mesh_error_response(
            request.request_id,
            StatusCode::BAD_GATEWAY,
            "local controller unavailable",
        ),
    }
}

fn authorize_mesh_control(
    state: &AppState,
    peer: &str,
    request: &MeshControlRequestV1,
) -> Result<(), String> {
    if request.path.contains("..") || request.path.contains('%') || request.path.contains('#') {
        return Err("mesh path is not canonical".into());
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/health") => Ok(()),
        ("POST", "/v1/nodes/enroll") => {
            let enrollment: EnrollmentRequestV1 = serde_json::from_value(
                request
                    .body
                    .clone()
                    .ok_or_else(|| "enrollment body is missing".to_string())?,
            )
            .map_err(|_| "enrollment body is invalid".to_string())?;
            if enrollment.identity.public_key == peer {
                Ok(())
            } else {
                Err("transport identity and enrollment identity differ".into())
            }
        }
        ("POST", "/v1/offers") => {
            let offer: ResourceOfferV1 = serde_json::from_value(
                request
                    .body
                    .clone()
                    .ok_or_else(|| "offer body is missing".to_string())?,
            )
            .map_err(|_| "offer body is invalid".to_string())?;
            require_mesh_node(state, peer, offer.node_id)
        }
        ("POST", "/v1/receipts") => {
            let receipt: ExecutionReceiptV1 = serde_json::from_value(
                request
                    .body
                    .clone()
                    .ok_or_else(|| "receipt body is missing".to_string())?,
            )
            .map_err(|_| "receipt body is invalid".to_string())?;
            require_mesh_node(state, peer, receipt.node_id)
        }
        ("POST", "/v1/benchmarks/link") => {
            let probe: LinkProbeRequest = serde_json::from_value(
                request
                    .body
                    .clone()
                    .ok_or_else(|| "link probe body is missing".to_string())?,
            )
            .map_err(|_| "link probe body is invalid".to_string())?;
            require_mesh_node(state, peer, probe.node_id)
        }
        ("GET", path) if path.starts_with("/v1/work/claim?node_id=") => {
            let node_id = path
                .strip_prefix("/v1/work/claim?node_id=")
                .and_then(|value| Uuid::parse_str(value).ok())
                .ok_or_else(|| "claim node id is invalid".to_string())?;
            require_mesh_node(state, peer, node_id)
        }
        _ => Err("route is not exposed to remote workers".into()),
    }
}

fn require_mesh_node(state: &AppState, peer: &str, node_id: Uuid) -> Result<(), String> {
    let nodes = state
        .nodes
        .read()
        .map_err(|_| "node registry unavailable".to_string())?;
    match nodes.get(&node_id) {
        Some(identity) if identity.public_key == peer => Ok(()),
        _ => Err("remote worker is not enrolled for this node id".into()),
    }
}

fn mesh_error_response(
    request_id: Uuid,
    status: StatusCode,
    message: &str,
) -> MeshControlResponseV1 {
    MeshControlResponseV1 {
        schema: MeshControlResponseV1::SCHEMA.into(),
        request_id,
        status: status.as_u16(),
        body: json!({"error": message}),
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
