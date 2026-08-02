use anyhow::Context;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::{
        HeaderName, HeaderValue, Method, Request, StatusCode,
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE},
    },
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
    Governor, GovernorConfig, ModelSessionLimits, verify_artifact_replica_receipt,
    verify_enrollment, verify_mesh_endpoint_with_key, verify_model_receipt, verify_offer,
};
use rampage_protocol::{
    ArtifactRefV1, ArtifactReplicaReceiptV1, ArtifactTransferOperation, CapabilityLeaseV1,
    DeviceKind, EnrollmentInviteV1, EnrollmentRequestV1, ExecutionReceiptV1, InstalledModelV1,
    JobSpecV1, JobState, LINK_BENCHMARK_TRANSFER_BYTES, MAX_ARTIFACT_TRANSFER_BYTES,
    MAX_MODEL_OUTPUT_BYTES, MAX_MODEL_OUTPUT_TOKENS, MAX_MODEL_PROMPT_BYTES, MeshControlRequestV1,
    MeshControlResponseV1, MeshEndpointRecordV1, ModelBackend, ModelChatMessageV1,
    ModelExecutionReceiptV1, ModelInvocationFrameKind, ModelInvocationRequestV1, ModelMemoryKind,
    ModelParallelism, ModelRuntimeOfferV1, ModelRuntimeStatus, ModelSessionLeaseV1,
    ModelSessionRequestV1, ModelUsageV1, NodeIdentityV1, PromotionCanaryLeaseV1,
    PromotionCandidateV1, RelayAccessManifestV1, ResourceClass, ResourceOfferV1, ShardSetV1,
    StorageClass, StorageLeaseV1, WorkClaimV1,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    convert::Infallible,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing::{info, warn};
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
    replica_evidence: Arc<RwLock<HashMap<(String, Uuid), ArtifactReplicaReceiptV1>>>,
    storage_probe_cursor: Arc<AtomicU64>,
    artifact_store: Arc<rampage_storage::CasStore>,
    local_api_token: Arc<String>,
    admission_gate: Arc<tokio::sync::Mutex<()>>,
    fencing_epoch: Arc<AtomicU64>,
    model_cancellations: Arc<tokio::sync::Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
    diagnostic_report: Arc<RwLock<Option<FabricDiagnosticReport>>>,
    diagnostic_digest: Arc<RwLock<Option<String>>>,
    autonomous_constraints: Arc<RwLock<AutonomousConstraints>>,
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
    fencing_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum DiagnosticSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
struct DiagnosticProposal {
    action: &'static str,
    risk: &'static str,
    auto_eligible: bool,
    threshold: &'static str,
    required_gates: Vec<&'static str>,
    rollback: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct DiagnosticFinding {
    severity: DiagnosticSeverity,
    code: &'static str,
    scope: String,
    evidence: String,
    proposal: DiagnosticProposal,
}

#[derive(Debug, Clone, Serialize)]
struct DiagnosticMetrics {
    enrolled_nodes: usize,
    live_offers: usize,
    expired_offers: usize,
    active_assignments: usize,
    protected_artifacts: usize,
    under_replicated_protected_artifacts: usize,
    recent_denials: usize,
    recent_failed_receipts: usize,
    available_resources: BTreeMap<&'static str, u64>,
}

#[derive(Debug, Clone, Serialize)]
struct DiagnosticAutonomy {
    mode: &'static str,
    per_change_approval_required: bool,
    eligible_within_envelope: Vec<&'static str>,
    authority_expansion: &'static str,
    promotion_requirements: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
struct FabricDiagnosticReport {
    schema: &'static str,
    generated_at: chrono::DateTime<chrono::Utc>,
    status: &'static str,
    health_score: u8,
    evidence_digest: String,
    metrics: DiagnosticMetrics,
    autonomy: DiagnosticAutonomy,
    findings: Vec<DiagnosticFinding>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct AutonomousConstraints {
    evidence_digest: String,
    excluded_nodes: BTreeMap<Uuid, Vec<String>>,
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
    let mut fencing_epoch = match ledger.current_fencing_epoch("controller")? {
        0 => ledger.advance_fencing_epoch("controller")?,
        current => current,
    };
    let kill_latch_path = data_dir.join("KILL");
    if kill_latch_path.is_file() && read_stop_epoch_marker(&kill_latch_path) != Some(fencing_epoch)
    {
        fencing_epoch = ledger.advance_fencing_epoch("controller")?;
        write_stop_epoch_marker(&kill_latch_path, fencing_epoch)?;
        ledger.append(
            "fabric.owner_stop.recovered",
            "local-fabric",
            &json!({"fencing_epoch": fencing_epoch}),
        )?;
    }
    let address: SocketAddr = std::env::var("RAMPAGE_BIND")
        .unwrap_or_else(|_| "127.0.0.1:47831".into())
        .parse()?;
    anyhow::ensure!(
        address.ip().is_loopback(),
        "the controller API must remain loopback-only; use the authenticated mesh transport for remote nodes"
    );
    let governor_config = governor_config_from_env()?;
    ledger.append(
        "governor.autonomy.envelope.loaded",
        "controller",
        &json!({
            "r1_projects": governor_config.trusted_autopilot_projects.len(),
            "r2_projects": governor_config.autonomous_protected_projects.len(),
            "per_change_approval_required": false,
            "authority_expansion": "denied"
        }),
    )?;
    let governor = Arc::new(load_or_create_governor(
        &data_dir.join("governor.key"),
        governor_config,
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
        replica_evidence,
    ) = restore_state(&ledger, fencing_epoch)?;
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
        kill_latch_path: Arc::new(kill_latch_path),
        mesh,
        reservations: Arc::new(RwLock::new(reservations)),
        admission_policy: Arc::new(AdmissionPolicy::default()),
        completed_receipts: Arc::new(RwLock::new(completed_receipts)),
        shard_sets: Arc::new(RwLock::new(shard_sets)),
        artifact_replicas: Arc::new(RwLock::new(artifact_replicas)),
        replica_evidence: Arc::new(RwLock::new(replica_evidence)),
        storage_probe_cursor: Arc::new(AtomicU64::new(0)),
        artifact_store: Arc::new(rampage_storage::CasStore::open(
            data_dir.join("cas"),
            load_or_create_secret(&data_dir.join("storage.key"))?,
        )?),
        local_api_token: local_api_token.clone(),
        admission_gate: Arc::new(tokio::sync::Mutex::new(())),
        fencing_epoch: Arc::new(AtomicU64::new(fencing_epoch)),
        model_cancellations: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        diagnostic_report: Arc::new(RwLock::new(None)),
        diagnostic_digest: Arc::new(RwLock::new(None)),
        autonomous_constraints: Arc::new(RwLock::new(AutonomousConstraints::default())),
    };
    refresh_diagnostics(&state).map_err(anyhow::Error::msg)?;
    tokio::spawn(run_diagnostic_loop(state.clone()));
    tokio::spawn(run_storage_repair_loop(state.clone()));
    let mesh_state = state.clone();
    let protected = Router::new()
        .route("/v1/stop", post(local_stop))
        .route("/v1/resume", post(local_resume))
        .route("/v1/enrollment/invites", post(create_invite))
        .route("/v1/nodes", get(list_nodes))
        .route("/v1/nodes/enroll", post(enroll_node))
        .route("/v1/offers", get(list_offers).post(register_offer))
        .route("/v1/workload-capabilities", get(list_workload_capabilities))
        .route("/v1/diagnostics/self-scan", get(self_scan))
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
        .route("/v1/artifacts/repair", post(repair_protected_artifacts))
        .route("/v1/benchmarks/link", post(link_probe))
        .route("/v1/mesh/relay-access", get(relay_access_manifest))
        .route("/v1/governor/key", get(governor_key))
        .route("/v1/improvements/canary", post(authorize_promotion_canary))
        .route("/v1/projects/discover", post(discover_project))
        .route("/v1/events", get(events))
        .route_layer(middleware::from_fn_with_state(
            local_api_token.clone(),
            require_local_token,
        ));
    let openai = Router::new()
        .route("/v1/models", get(openai_models))
        .route("/v1/chat/completions", post(openai_chat_completions))
        .route("/api/v1/models", get(openai_models))
        .route("/api/v1/chat/completions", post(openai_chat_completions))
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/capabilities", get(gateway_capabilities))
        .route(
            "/.well-known/rampage-capabilities",
            get(gateway_capabilities),
        )
        .route(
            "/v1/model-sessions/{session_id}/cancel",
            post(cancel_model_session),
        )
        .route_layer(middleware::from_fn_with_state(
            local_api_token.clone(),
            require_gateway_token,
        ));
    let app = Router::new()
        .route("/health", get(health))
        .merge(protected)
        .merge(openai)
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
                .allow_headers([
                    CONTENT_TYPE,
                    AUTHORIZATION,
                    HeaderName::from_static("x-rampage-token"),
                    HeaderName::from_static("x-api-key"),
                    HeaderName::from_static("anthropic-version"),
                    HeaderName::from_static("anthropic-beta"),
                ])
                .expose_headers([HeaderName::from_static("x-rampage-session-id")]),
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
        fencing_epoch: state.fencing_epoch.load(Ordering::Acquire),
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
    if !bool::from(expected_digest.ct_eq(&supplied_digest)) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "valid local Rampage token required"})),
        )
            .into_response();
    }
    next.run(request).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAiChatCompletionRequest {
    model: String,
    messages: Vec<OpenAiChatMessage>,
    #[serde(default)]
    stream: bool,
    max_tokens: Option<u32>,
    max_completion_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAiChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    system: Option<AnthropicContent>,
    #[serde(default)]
    stream: bool,
    temperature: Option<f32>,
    top_p: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicTextBlock>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnthropicTextBlock {
    r#type: String,
    text: String,
}

#[derive(Clone)]
struct ModelCandidate {
    offer: ResourceOfferV1,
    runtime: ModelRuntimeOfferV1,
    model: InstalledModelV1,
}

struct ActiveModelInvocation {
    request_id: Uuid,
    lease: ModelSessionLeaseV1,
    stream: rampage_mesh::ModelResponseStream,
    cancel: watch::Receiver<bool>,
}

struct ModelGatewayCompletion {
    content: String,
    finish_reason: String,
    usage: Option<ModelUsageV1>,
}

#[derive(Debug, Clone, Copy)]
enum ModelStreamFormat {
    OpenAi,
    Anthropic,
}

#[derive(Debug)]
struct ModelGatewayFailure {
    status: StatusCode,
    kind: &'static str,
    code: &'static str,
    message: String,
}

async fn require_gateway_token(
    State(expected): State<Arc<String>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let bearer = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token)
        .unwrap_or_default();
    let supplied = request
        .headers()
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or(bearer);
    let expected_digest = Sha256::digest(expected.as_bytes());
    let supplied_digest = Sha256::digest(supplied.as_bytes());
    if !bool::from(expected_digest.ct_eq(&supplied_digest)) {
        if request.uri().path() == "/v1/messages" {
            return anthropic_error(
                StatusCode::UNAUTHORIZED,
                "valid Rampage API key required",
                "authentication_error",
            );
        }
        return openai_error(
            StatusCode::UNAUTHORIZED,
            "valid Rampage bearer token required",
            "authentication_error",
            None,
            "invalid_api_key",
        );
    }
    next.run(request).await
}

async fn openai_models(State(state): State<AppState>) -> Response {
    let catalog = match live_model_catalog(&state) {
        Ok(catalog) => catalog,
        Err(error) => return gateway_failure_response(error),
    };
    let mut data = catalog
        .into_iter()
        .filter_map(|(model_id, candidates)| {
            let created = candidates
                .iter()
                .map(|candidate| candidate.offer.observed_at.timestamp())
                .max()?;
            Some(json!({
                "id": model_id,
                "object": "model",
                "created": created,
                "owned_by": "rampage-fabric"
            }))
        })
        .collect::<Vec<_>>();
    data.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    Json(json!({"object": "list", "data": data})).into_response()
}

async fn gateway_capabilities() -> Json<Value> {
    Json(json!({
        "schema": "rampage.gateway-capabilities.v1",
        "execution": {
            "topology": "whole_model_one_contributor",
            "cross_host_shared_memory": false,
            "terminal_success_requires_signed_receipt": true
        },
        "workload_contract": {
            "schema": "rampage.workload-capability.v1",
            "inventory_path": "/v1/workload-capabilities",
            "authority": "verified_signed_offer_plus_exact_adapter_and_operation",
            "candidate_profiles_authorize_execution": false,
            "domains": [
                "ai_inference",
                "ai_evaluation",
                "gaming",
                "creative_production",
                "software_build",
                "scientific_computing",
                "data_processing",
                "storage",
                "edge_utility"
            ],
            "note": "a domain is executable only when a live signed offer advertises a shipped or qualified exact capability"
        },
        "diagnostics": {
            "path": "/v1/diagnostics/self-scan",
            "mode": "deterministic_thresholded_governor",
            "per_change_approval_required": false,
            "authority_expansion": "automatically_denied_outside_owner_envelope",
            "signed_canary_path": "/v1/improvements/canary",
            "required_evidence_gates": 8
        },
        "protocols": [
            {
                "id": "openai.chat_completions",
                "paths": ["/v1/chat/completions", "/api/v1/chat/completions"],
                "models_paths": ["/v1/models", "/api/v1/models"],
                "streaming": "sse",
                "content": ["text"]
            },
            {
                "id": "anthropic.messages",
                "paths": ["/v1/messages"],
                "streaming": "anthropic_sse_events",
                "content": ["text"]
            }
        ],
        "unsupported": [
            "tools",
            "vision",
            "audio",
            "provider_routing",
            "cross_host_tensor_or_pipeline_launch"
        ]
    }))
}

async fn openai_chat_completions(
    State(state): State<AppState>,
    payload: Result<Json<OpenAiChatCompletionRequest>, JsonRejection>,
) -> Response {
    let request = match payload {
        Ok(Json(request)) => request,
        Err(error) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                &bounded_gateway_error(&error.to_string()),
                "invalid_request_error",
                None,
                "invalid_json",
            );
        }
    };
    let max_output_tokens = match validate_openai_request(&request) {
        Ok(limit) => limit,
        Err(error) => return gateway_failure_response(error),
    };
    let wants_stream = request.stream;
    let model_id = request.model.clone();
    let invocation = match start_model_invocation(&state, request, max_output_tokens).await {
        Ok(invocation) => invocation,
        Err(error) => return gateway_failure_response(error),
    };
    let session_id = invocation.lease.session_id;
    let request_id = invocation.request_id;
    let created = invocation.lease.issued_at.timestamp();

    if wants_stream {
        let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
        let task_sender = sender.clone();
        let task_state = state.clone();
        tokio::spawn(async move {
            let _ = task_sender
                .send(Ok(openai_sse(json!({
                    "id": format!("chatcmpl-{}", request_id.simple()),
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model_id,
                    "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]
                }))))
                .await;
            let result = consume_model_invocation(
                &task_state,
                invocation,
                Some((&task_sender, ModelStreamFormat::OpenAi)),
            )
            .await;
            task_state
                .model_cancellations
                .lock()
                .await
                .remove(&session_id);
            match result {
                Ok(completion) => {
                    let _ = task_sender
                        .send(Ok(openai_sse(json!({
                            "id": format!("chatcmpl-{}", request_id.simple()),
                            "object": "chat.completion.chunk",
                            "created": created,
                            "model": model_id,
                            "choices": [{"index": 0, "delta": {}, "finish_reason": completion.finish_reason}]
                        }))))
                        .await;
                }
                Err(error) => {
                    let _ = task_sender
                        .send(Ok(openai_sse(json!({"error": {
                            "message": error.message,
                            "type": error.kind,
                            "param": null,
                            "code": error.code
                        }}))))
                        .await;
                }
            }
            let _ = task_sender
                .send(Ok(Bytes::from_static(b"data: [DONE]\n\n")))
                .await;
        });
        drop(sender);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"))
            .header(CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .header("x-rampage-session-id", session_id.to_string())
            .body(Body::from_stream(ReceiverStream::new(receiver)))
            .expect("static streaming response is valid");
    }

    let result = consume_model_invocation(&state, invocation, None).await;
    state.model_cancellations.lock().await.remove(&session_id);
    match result {
        Ok(completion) => {
            let usage = completion.usage.map(|usage| {
                json!({
                    "prompt_tokens": usage.prompt_tokens,
                    "completion_tokens": usage.completion_tokens,
                    "total_tokens": usage.prompt_tokens.saturating_add(usage.completion_tokens)
                })
            });
            let mut response = json!({
                "id": format!("chatcmpl-{}", request_id.simple()),
                "object": "chat.completion",
                "created": created,
                "model": model_id,
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": completion.content},
                    "finish_reason": completion.finish_reason
                }]
            });
            if let Some(usage) = usage {
                response["usage"] = usage;
            }
            let mut response = Json(response).into_response();
            response.headers_mut().insert(
                HeaderName::from_static("x-rampage-session-id"),
                HeaderValue::from_str(&session_id.to_string())
                    .expect("UUID is a valid header value"),
            );
            response
        }
        Err(error) => gateway_failure_response(error),
    }
}

async fn anthropic_messages(
    State(state): State<AppState>,
    payload: Result<Json<AnthropicMessagesRequest>, JsonRejection>,
) -> Response {
    let request = match payload {
        Ok(Json(request)) => request,
        Err(error) => {
            return anthropic_error(
                StatusCode::BAD_REQUEST,
                &bounded_gateway_error(&error.to_string()),
                "invalid_request_error",
            );
        }
    };
    let wants_stream = request.stream;
    let model_id = request.model.clone();
    let openai_request = match translate_anthropic_request(request) {
        Ok(request) => request,
        Err(error) => return anthropic_gateway_failure_response(error),
    };
    let max_output_tokens = match validate_openai_request(&openai_request) {
        Ok(limit) => limit,
        Err(error) => return anthropic_gateway_failure_response(error),
    };
    let invocation = match start_model_invocation(&state, openai_request, max_output_tokens).await {
        Ok(invocation) => invocation,
        Err(error) => return anthropic_gateway_failure_response(error),
    };
    let session_id = invocation.lease.session_id;
    let request_id = invocation.request_id;
    let message_id = format!("msg_{}", request_id.simple());

    if wants_stream {
        let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
        let task_sender = sender.clone();
        let task_state = state.clone();
        tokio::spawn(async move {
            let _ = task_sender
                .send(Ok(anthropic_sse(
                    "message_start",
                    json!({
                        "type": "message_start",
                        "message": {
                            "id": message_id,
                            "type": "message",
                            "role": "assistant",
                            "content": [],
                            "model": model_id,
                            "stop_reason": null,
                            "stop_sequence": null,
                            "usage": {"input_tokens": 0, "output_tokens": 0}
                        }
                    }),
                )))
                .await;
            let _ = task_sender
                .send(Ok(anthropic_sse(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {"type": "text", "text": ""}
                    }),
                )))
                .await;
            let result = consume_model_invocation(
                &task_state,
                invocation,
                Some((&task_sender, ModelStreamFormat::Anthropic)),
            )
            .await;
            task_state
                .model_cancellations
                .lock()
                .await
                .remove(&session_id);
            match result {
                Ok(completion) => {
                    if let Some(usage) = completion.usage {
                        let _ = task_sender
                            .send(Ok(anthropic_sse(
                                "content_block_stop",
                                json!({"type": "content_block_stop", "index": 0}),
                            )))
                            .await;
                        let _ = task_sender
                            .send(Ok(anthropic_sse(
                                "message_delta",
                                json!({
                                    "type": "message_delta",
                                    "delta": {
                                        "stop_reason": anthropic_stop_reason(&completion.finish_reason),
                                        "stop_sequence": null
                                    },
                                    "usage": {
                                        "input_tokens": usage.prompt_tokens,
                                        "output_tokens": usage.completion_tokens
                                    }
                                }),
                            )))
                            .await;
                        let _ = task_sender
                            .send(Ok(anthropic_sse(
                                "message_stop",
                                json!({"type": "message_stop"}),
                            )))
                            .await;
                    } else {
                        let _ = task_sender
                            .send(Ok(anthropic_sse(
                                "error",
                                json!({"type": "error", "error": {
                                    "type": "api_error",
                                    "message": "worker omitted required verified token usage"
                                }}),
                            )))
                            .await;
                    }
                }
                Err(error) => {
                    let _ = task_sender
                        .send(Ok(anthropic_sse(
                            "error",
                            json!({"type": "error", "error": {
                                "type": anthropic_failure_kind(error.status),
                                "message": bounded_gateway_error(&error.message)
                            }}),
                        )))
                        .await;
                }
            }
        });
        drop(sender);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"))
            .header(CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .header("x-rampage-session-id", session_id.to_string())
            .header("request-id", format!("req_{}", request_id.simple()))
            .body(Body::from_stream(ReceiverStream::new(receiver)))
            .expect("static Anthropic streaming response is valid");
    }

    let result = consume_model_invocation(&state, invocation, None).await;
    state.model_cancellations.lock().await.remove(&session_id);
    match result {
        Ok(completion) => {
            let Some(usage) = completion.usage else {
                return anthropic_error(
                    StatusCode::BAD_GATEWAY,
                    "worker omitted required verified token usage",
                    "api_error",
                );
            };
            let response = json!({
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": completion.content}],
                "model": model_id,
                "stop_reason": anthropic_stop_reason(&completion.finish_reason),
                "stop_sequence": null,
                "usage": {
                    "input_tokens": usage.prompt_tokens,
                    "output_tokens": usage.completion_tokens
                }
            });
            let mut response = Json(response).into_response();
            response.headers_mut().insert(
                HeaderName::from_static("x-rampage-session-id"),
                HeaderValue::from_str(&session_id.to_string())
                    .expect("UUID is a valid header value"),
            );
            response.headers_mut().insert(
                HeaderName::from_static("request-id"),
                HeaderValue::from_str(&format!("req_{}", request_id.simple()))
                    .expect("request ID is a valid header value"),
            );
            response
        }
        Err(error) => anthropic_gateway_failure_response(error),
    }
}

fn translate_anthropic_request(
    request: AnthropicMessagesRequest,
) -> Result<OpenAiChatCompletionRequest, ModelGatewayFailure> {
    if request.max_tokens == 0 {
        return Err(invalid_model_request(
            "max_tokens must be positive; prompt-cache-only requests are not supported",
            "max_tokens",
        ));
    }
    let mut messages = Vec::with_capacity(request.messages.len().saturating_add(1));
    if let Some(system) = request.system {
        messages.push(OpenAiChatMessage {
            role: "system".into(),
            content: anthropic_content_text(system)?,
        });
    }
    for message in request.messages {
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err(invalid_model_request(
                "Anthropic messages support only user and assistant roles",
                "messages",
            ));
        }
        messages.push(OpenAiChatMessage {
            role: message.role,
            content: anthropic_content_text(message.content)?,
        });
    }
    Ok(OpenAiChatCompletionRequest {
        model: request.model,
        messages,
        stream: request.stream,
        max_tokens: None,
        max_completion_tokens: Some(request.max_tokens),
        temperature: request.temperature,
        top_p: request.top_p,
    })
}

fn anthropic_content_text(content: AnthropicContent) -> Result<String, ModelGatewayFailure> {
    let text = match content {
        AnthropicContent::Text(text) => text,
        AnthropicContent::Blocks(blocks) => {
            if blocks.is_empty()
                || blocks
                    .iter()
                    .any(|block| block.r#type != "text" || block.text.is_empty())
            {
                return Err(invalid_model_request(
                    "only non-empty Anthropic text content blocks are supported",
                    "messages",
                ));
            }
            blocks
                .into_iter()
                .map(|block| block.text)
                .collect::<Vec<_>>()
                .join("")
        }
    };
    if text.is_empty() {
        return Err(invalid_model_request(
            "Anthropic message content must not be empty",
            "messages",
        ));
    }
    Ok(text)
}

async fn cancel_model_session(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Response {
    let cancellations = state.model_cancellations.lock().await;
    let Some(cancellation) = cancellations.get(&session_id) else {
        return openai_error(
            StatusCode::NOT_FOUND,
            "model session is not active",
            "invalid_request_error",
            Some("session_id"),
            "session_not_found",
        );
    };
    let _ = cancellation.send(true);
    let _ = state.ledger.append(
        "model.session.cancel.requested",
        &session_id.to_string(),
        &json!({"source": "local-openai-api"}),
    );
    (
        StatusCode::ACCEPTED,
        Json(json!({"session_id": session_id, "cancelled": true})),
    )
        .into_response()
}

fn validate_openai_request(
    request: &OpenAiChatCompletionRequest,
) -> Result<u32, ModelGatewayFailure> {
    if request.model.trim().is_empty() || request.model.len() > 200 || !request.model.is_ascii() {
        return Err(invalid_model_request(
            "model must be a non-empty ASCII identifier",
            "model",
        ));
    }
    if request.messages.is_empty() || request.messages.len() > 256 {
        return Err(invalid_model_request(
            "messages must contain between 1 and 256 entries",
            "messages",
        ));
    }
    if request.messages.iter().any(|message| {
        !matches!(message.role.as_str(), "system" | "user" | "assistant")
            || message.content.is_empty()
    }) {
        return Err(invalid_model_request(
            "only non-empty system, user, and assistant text messages are supported",
            "messages",
        ));
    }
    let prompt_bytes = request.messages.iter().fold(0_u64, |total, message| {
        total
            .saturating_add(message.role.len() as u64)
            .saturating_add(message.content.len() as u64)
    });
    if prompt_bytes > MAX_MODEL_PROMPT_BYTES {
        return Err(ModelGatewayFailure {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            kind: "invalid_request_error",
            code: "prompt_too_large",
            message: "message text exceeds Rampage's one MiB prompt limit".into(),
        });
    }
    if request.max_tokens.is_some()
        && request.max_completion_tokens.is_some()
        && request.max_tokens != request.max_completion_tokens
    {
        return Err(invalid_model_request(
            "max_tokens and max_completion_tokens conflict",
            "max_completion_tokens",
        ));
    }
    let max_output_tokens = request
        .max_completion_tokens
        .or(request.max_tokens)
        .unwrap_or(512);
    if max_output_tokens == 0 || max_output_tokens > MAX_MODEL_OUTPUT_TOKENS {
        return Err(invalid_model_request(
            "requested output token limit is outside the supported range",
            "max_completion_tokens",
        ));
    }
    if request
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        || request
            .top_p
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(invalid_model_request(
            "temperature or top_p is outside the supported range",
            "temperature",
        ));
    }
    Ok(max_output_tokens)
}

async fn start_model_invocation(
    state: &AppState,
    request: OpenAiChatCompletionRequest,
    max_output_tokens: u32,
) -> Result<ActiveModelInvocation, ModelGatewayFailure> {
    let _admission_guard = state.admission_gate.lock().await;
    if state.kill_latch_path.is_file() {
        return Err(ModelGatewayFailure {
            status: StatusCode::LOCKED,
            kind: "server_error",
            code: "owner_stop_active",
            message: "owner STOP is active".into(),
        });
    }
    let catalog = live_model_catalog(state)?;
    let candidates = catalog
        .get(&request.model)
        .ok_or_else(|| ModelGatewayFailure {
            status: StatusCode::NOT_FOUND,
            kind: "invalid_request_error",
            code: "model_not_found",
            message: "model is not consistently installed on an eligible contributor".into(),
        })?;
    let candidate = candidates
        .iter()
        .max_by_key(|candidate| candidate.runtime.available_model_bytes)
        .cloned()
        .expect("catalog entries are non-empty");
    let prompt_bytes = request.messages.iter().fold(0_u64, |total, message| {
        total
            .saturating_add(message.role.len() as u64)
            .saturating_add(message.content.len() as u64)
    });
    let lease = state
        .governor
        .authorize_model_session_at_epoch(
            &candidate.offer,
            &candidate.runtime,
            &candidate.model,
            &state.mesh.endpoint_id(),
            ModelSessionLimits {
                max_prompt_bytes: prompt_bytes.max(1),
                max_output_tokens,
            },
            state.fencing_epoch.load(Ordering::Acquire),
        )
        .map_err(|error| ModelGatewayFailure {
            status: StatusCode::FORBIDDEN,
            kind: "server_error",
            code: "model_authority_denied",
            message: error.to_string(),
        })?;
    let endpoint_record = candidate
        .offer
        .mesh_endpoint
        .as_ref()
        .expect("catalog requires a mesh endpoint");
    let endpoint = rampage_mesh::endpoint_addr_from_record(endpoint_record).map_err(|error| {
        ModelGatewayFailure {
            status: StatusCode::BAD_GATEWAY,
            kind: "server_error",
            code: "worker_endpoint_invalid",
            message: error.to_string(),
        }
    })?;
    let request_id = Uuid::now_v7();
    let invocation = ModelInvocationRequestV1 {
        schema: ModelInvocationRequestV1::SCHEMA.into(),
        request_id,
        lease: lease.clone(),
        messages: request
            .messages
            .into_iter()
            .map(|message| ModelChatMessageV1 {
                role: message.role,
                content: message.content,
            })
            .collect(),
        max_output_tokens,
        stream: request.stream,
        temperature: request.temperature,
        top_p: request.top_p,
    };
    if !invocation.is_valid_for(candidate.offer.node_id, &state.mesh.endpoint_id()) {
        return Err(ModelGatewayFailure {
            status: StatusCode::BAD_REQUEST,
            kind: "invalid_request_error",
            code: "invalid_model_request",
            message: "request does not fit its bounded model-session authority".into(),
        });
    }
    state
        .ledger
        .append(
            "model.session.lease.issued",
            &lease.session_id.to_string(),
            &json!({
                "lease": &lease,
                "request_id": request_id,
                "model_digest": candidate.model.artifact_digest,
                "node_id": candidate.offer.node_id
            }),
        )
        .map_err(|error| ModelGatewayFailure {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: "server_error",
            code: "evidence_write_failed",
            message: error.to_string(),
        })?;
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        rampage_mesh::invoke_model(&state.mesh.endpoint(), endpoint, &invocation),
    )
    .await
    .map_err(|_| ModelGatewayFailure {
        status: StatusCode::GATEWAY_TIMEOUT,
        kind: "server_error",
        code: "worker_connect_timeout",
        message: "timed out connecting to the selected model worker".into(),
    })?
    .map_err(|error| ModelGatewayFailure {
        status: StatusCode::BAD_GATEWAY,
        kind: "server_error",
        code: "worker_unavailable",
        message: bounded_gateway_error(&error.to_string()),
    })?;
    let (cancellation, cancel) = watch::channel(false);
    state
        .model_cancellations
        .lock()
        .await
        .insert(lease.session_id, cancellation);
    Ok(ActiveModelInvocation {
        request_id,
        lease,
        stream,
        cancel,
    })
}

async fn consume_model_invocation(
    state: &AppState,
    mut invocation: ActiveModelInvocation,
    stream_sender: Option<(&mpsc::Sender<Result<Bytes, Infallible>>, ModelStreamFormat)>,
) -> Result<ModelGatewayCompletion, ModelGatewayFailure> {
    let mut expected_sequence = 0_u64;
    let mut output = String::new();
    loop {
        let remaining = (invocation.lease.expires_at - chrono::Utc::now())
            .to_std()
            .map_err(|_| model_timeout())?;
        let frame = tokio::select! {
            changed = invocation.cancel.changed() => {
                let _ = changed;
                return Err(ModelGatewayFailure {
                    status: StatusCode::REQUEST_TIMEOUT,
                    kind: "server_error",
                    code: "model_session_cancelled",
                    message: "model session was cancelled".into(),
                });
            }
            result = tokio::time::timeout(remaining, invocation.stream.next_frame()) => {
                result.map_err(|_| model_timeout())?.map_err(|error| ModelGatewayFailure {
                    status: StatusCode::BAD_GATEWAY,
                    kind: "server_error",
                    code: "invalid_worker_stream",
                    message: bounded_gateway_error(&error.to_string()),
                })?
            }
        };
        if frame.sequence != expected_sequence {
            return Err(invalid_worker_stream(
                "model frame sequence is not contiguous",
            ));
        }
        match frame.kind {
            ModelInvocationFrameKind::Delta => {
                if frame.receipt.is_some() || frame.error.is_some() || frame.finish_reason.is_some()
                {
                    return Err(invalid_worker_stream(
                        "model delta contains terminal fields",
                    ));
                }
                output.push_str(&frame.content);
                if output.len() > MAX_MODEL_OUTPUT_BYTES as usize {
                    return Err(invalid_worker_stream(
                        "model output exceeds the transcript limit",
                    ));
                }
                if let Some((sender, format)) = stream_sender {
                    let delta = match format {
                        ModelStreamFormat::OpenAi => openai_sse(json!({
                            "id": format!("chatcmpl-{}", invocation.request_id.simple()),
                            "object": "chat.completion.chunk",
                            "created": invocation.lease.issued_at.timestamp(),
                            "model": invocation.lease.model_id,
                            "choices": [{"index": 0, "delta": {"content": frame.content}, "finish_reason": null}]
                        })),
                        ModelStreamFormat::Anthropic => anthropic_sse(
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": 0,
                                "delta": {"type": "text_delta", "text": frame.content}
                            }),
                        ),
                    };
                    sender
                        .send(Ok(delta))
                        .await
                        .map_err(|_| ModelGatewayFailure {
                            status: StatusCode::REQUEST_TIMEOUT,
                            kind: "server_error",
                            code: "client_disconnected",
                            message: "streaming client disconnected".into(),
                        })?;
                }
                expected_sequence = expected_sequence.saturating_add(1);
            }
            ModelInvocationFrameKind::Complete | ModelInvocationFrameKind::Error => {
                if !frame.content.is_empty() {
                    return Err(invalid_worker_stream(
                        "terminal model frame contains output text",
                    ));
                }
                let Some(receipt) = frame.receipt else {
                    if frame.kind == ModelInvocationFrameKind::Error
                        && expected_sequence == 0
                        && output.is_empty()
                    {
                        return Err(ModelGatewayFailure {
                            status: StatusCode::BAD_GATEWAY,
                            kind: "server_error",
                            code: "worker_rejected_session",
                            message: frame
                                .error
                                .unwrap_or_else(|| "model worker rejected the session".into()),
                        });
                    }
                    return Err(invalid_worker_stream(
                        "terminal model frame omitted its signed receipt",
                    ));
                };
                verify_terminal_model_receipt(state, &invocation, &receipt, &output)?;
                if (frame.kind == ModelInvocationFrameKind::Complete
                    && receipt.state != JobState::Succeeded)
                    || (frame.kind == ModelInvocationFrameKind::Error
                        && receipt.state == JobState::Succeeded)
                {
                    return Err(invalid_worker_stream(
                        "terminal frame and signed receipt disagree on execution state",
                    ));
                }
                if frame
                    .finish_reason
                    .as_ref()
                    .is_some_and(|reason| reason.len() > 64 || !reason.is_ascii())
                    || (receipt.state == JobState::Succeeded && receipt.error.is_some())
                {
                    return Err(invalid_worker_stream(
                        "terminal model metadata is malformed",
                    ));
                }
                state
                    .ledger
                    .append(
                        "model.session.receipted",
                        &invocation.lease.session_id.to_string(),
                        &receipt,
                    )
                    .map_err(|error| ModelGatewayFailure {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        kind: "server_error",
                        code: "evidence_write_failed",
                        message: error.to_string(),
                    })?;
                if frame.kind == ModelInvocationFrameKind::Error {
                    return Err(ModelGatewayFailure {
                        status: StatusCode::BAD_GATEWAY,
                        kind: "server_error",
                        code: "model_execution_failed",
                        message: frame
                            .error
                            .or(receipt.error)
                            .unwrap_or_else(|| "model execution failed".into()),
                    });
                }
                return Ok(ModelGatewayCompletion {
                    content: output,
                    finish_reason: frame.finish_reason.unwrap_or_else(|| "stop".into()),
                    usage: receipt.usage,
                });
            }
        }
    }
}

fn verify_terminal_model_receipt(
    state: &AppState,
    invocation: &ActiveModelInvocation,
    receipt: &ModelExecutionReceiptV1,
    output: &str,
) -> Result<(), ModelGatewayFailure> {
    let identity = state
        .nodes
        .read()
        .map_err(|_| ModelGatewayFailure {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: "server_error",
            code: "controller_state_unavailable",
            message: "controller state lock poisoned".into(),
        })?
        .get(&receipt.node_id)
        .cloned()
        .ok_or_else(|| invalid_worker_stream("model receipt signer is not enrolled"))?;
    verify_model_receipt(&identity, receipt)
        .map_err(|_| invalid_worker_stream("model receipt signature is invalid"))?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(output.as_bytes())));
    if receipt.schema != ModelExecutionReceiptV1::SCHEMA
        || receipt.lease_id != invocation.lease.lease_id
        || receipt.session_id != invocation.lease.session_id
        || receipt.request_id != invocation.request_id
        || receipt.node_id != invocation.lease.node_id
        || !matches!(
            receipt.state,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled
        )
        || receipt.finished_at < receipt.started_at
        || receipt.output_digest != digest
        || receipt.output_bytes != output.len() as u64
    {
        return Err(invalid_worker_stream(
            "model receipt does not match the invocation transcript",
        ));
    }
    Ok(())
}

fn live_model_catalog(
    state: &AppState,
) -> Result<HashMap<String, Vec<ModelCandidate>>, ModelGatewayFailure> {
    let now = chrono::Utc::now();
    let excluded = state
        .autonomous_constraints
        .read()
        .map_err(|_| ModelGatewayFailure {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            kind: "server_error",
            code: "controller_state_unavailable",
            message: "autonomous constraint lock poisoned".into(),
        })?
        .excluded_nodes
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let offers = state.offers.read().map_err(|_| ModelGatewayFailure {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        kind: "server_error",
        code: "controller_state_unavailable",
        message: "controller state lock poisoned".into(),
    })?;
    let mut catalog: HashMap<String, Vec<ModelCandidate>> = HashMap::new();
    for offer in offers.values().filter(|offer| {
        offer.expires_at > now
            && !excluded.contains(&offer.node_id)
            && offer.mesh_endpoint.is_some()
            && offer.availability.foreground_allowed
            && offer.availability.thermal_headroom_percent >= 15
            && (offer.availability.on_ac_power
                || offer.availability.battery_percent.unwrap_or(100) >= 50)
    }) {
        for runtime in &offer.model_runtimes {
            if runtime.schema != ModelRuntimeOfferV1::SCHEMA
                || runtime.backend != ModelBackend::LocalOllama
                || runtime.status != ModelRuntimeStatus::ShippedLocal
                || runtime.adapter != "rampage.ollama.v1"
                || !offer.adapters.contains(&runtime.adapter)
                || !runtime
                    .supported_parallelism
                    .contains(&ModelParallelism::WholeModel)
            {
                continue;
            }
            for model in &runtime.installed_models {
                let guarded_bytes = model
                    .artifact_size_bytes
                    .saturating_add(model.artifact_size_bytes / 5);
                if !model.is_valid() || runtime_capacity_from_offer(offer, runtime) < guarded_bytes
                {
                    continue;
                }
                catalog
                    .entry(model.model_id.clone())
                    .or_default()
                    .push(ModelCandidate {
                        offer: offer.clone(),
                        runtime: runtime.clone(),
                        model: model.clone(),
                    });
            }
        }
    }
    catalog.retain(|_, candidates| {
        candidates
            .iter()
            .map(|candidate| candidate.model.artifact_digest.as_str())
            .collect::<BTreeSet<_>>()
            .len()
            == 1
    });
    Ok(catalog)
}

fn validate_model_runtime_contracts(offer: &ResourceOfferV1) -> Result<(), String> {
    if offer.workload_capabilities.len() > 64 {
        return Err("offer contains too many workload-capability profiles".into());
    }
    let mut capability_adapters = BTreeSet::new();
    if offer.workload_capabilities.iter().any(|capability| {
        !capability.is_valid()
            || !offer.adapters.contains(&capability.adapter)
            || !capability_adapters.insert(capability.adapter.as_str())
    }) {
        return Err("offer contains a malformed or duplicate workload capability".into());
    }
    if offer.model_runtimes.len() > 16 {
        return Err("offer exceeds the 16 model-runtime profile limit".into());
    }
    let mut runtimes = BTreeSet::new();
    for runtime in &offer.model_runtimes {
        if runtime.schema != ModelRuntimeOfferV1::SCHEMA
            || runtime.adapter.trim().is_empty()
            || runtime.runtime_version.trim().is_empty()
            || runtime.runtime_digest.trim().is_empty()
            || runtime.compatibility_key.trim().is_empty()
            || runtime.available_model_bytes == 0
            || runtime.available_model_bytes > runtime_capacity_from_offer(offer, runtime)
            || !offer.adapters.contains(&runtime.adapter)
            || runtime.installed_models.len() > 128
            || !runtimes.insert((runtime.backend, runtime.compatibility_key.as_str()))
        {
            return Err("offer contains a malformed or contradictory model-runtime profile".into());
        }
        let mut model_ids = BTreeSet::new();
        if runtime
            .installed_models
            .iter()
            .any(|model| !model.is_valid() || !model_ids.insert(model.model_id.as_str()))
        {
            return Err("offer contains a malformed or duplicate installed model".into());
        }
        match runtime.status {
            ModelRuntimeStatus::ShippedLocal
                if runtime.backend == ModelBackend::LocalOllama
                    && runtime.adapter == "rampage.ollama.v1"
                    && runtime.runtime_digest.starts_with("shipped-local:")
                    && runtime
                        .supported_parallelism
                        .contains(&ModelParallelism::WholeModel)
                    && runtime.certification_digest.is_none() => {}
            ModelRuntimeStatus::Qualified
                if runtime.installed_models.is_empty()
                    && runtime.is_qualified_for_distributed() => {}
            ModelRuntimeStatus::Candidate if runtime.installed_models.is_empty() => {}
            _ => {
                return Err(
                    "model runtime status, backend, topology, or installed-model authority is inconsistent"
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn runtime_capacity_from_offer(offer: &ResourceOfferV1, runtime: &ModelRuntimeOfferV1) -> u64 {
    let available = |class| {
        offer
            .resources
            .iter()
            .find(|resource| resource.class == class && resource.unit == "byte")
            .map_or(0, |resource| resource.available)
    };
    let observed = match runtime.memory_kind {
        ModelMemoryKind::DedicatedGpu => available(ResourceClass::GpuMemory),
        ModelMemoryKind::Unified | ModelMemoryKind::Host => available(ResourceClass::RamWorkingSet),
        ModelMemoryKind::Hybrid => available(ResourceClass::GpuMemory)
            .saturating_add(available(ResourceClass::RamWorkingSet)),
    };
    runtime.available_model_bytes.min(observed)
}

fn openai_sse(value: Value) -> Bytes {
    Bytes::from(format!("data: {}\n\n", value))
}

fn anthropic_sse(event: &str, value: Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {value}\n\n"))
}

fn anthropic_stop_reason(finish_reason: &str) -> &'static str {
    if finish_reason == "length" {
        "max_tokens"
    } else {
        "end_turn"
    }
}

fn anthropic_failure_kind(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => "invalid_request_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        _ => "api_error",
    }
}

fn anthropic_error(status: StatusCode, message: &str, kind: &'static str) -> Response {
    (
        status,
        Json(json!({
            "type": "error",
            "error": {"type": kind, "message": message}
        })),
    )
        .into_response()
}

fn openai_error(
    status: StatusCode,
    message: &str,
    kind: &'static str,
    param: Option<&str>,
    code: &'static str,
) -> Response {
    (
        status,
        Json(json!({"error": {
            "message": message,
            "type": kind,
            "param": param,
            "code": code
        }})),
    )
        .into_response()
}

fn gateway_failure_response(error: ModelGatewayFailure) -> Response {
    openai_error(
        error.status,
        &bounded_gateway_error(&error.message),
        error.kind,
        None,
        error.code,
    )
}

fn anthropic_gateway_failure_response(error: ModelGatewayFailure) -> Response {
    anthropic_error(
        error.status,
        &bounded_gateway_error(&error.message),
        anthropic_failure_kind(error.status),
    )
}

fn invalid_model_request(message: &str, _param: &'static str) -> ModelGatewayFailure {
    ModelGatewayFailure {
        status: StatusCode::BAD_REQUEST,
        kind: "invalid_request_error",
        code: "invalid_model_request",
        message: message.into(),
    }
}

fn invalid_worker_stream(message: &str) -> ModelGatewayFailure {
    ModelGatewayFailure {
        status: StatusCode::BAD_GATEWAY,
        kind: "server_error",
        code: "invalid_worker_stream",
        message: message.into(),
    }
}

fn model_timeout() -> ModelGatewayFailure {
    ModelGatewayFailure {
        status: StatusCode::GATEWAY_TIMEOUT,
        kind: "server_error",
        code: "model_session_timeout",
        message: "model session lease expired before completion".into(),
    }
}

fn bounded_gateway_error(error: &str) -> String {
    error.chars().take(512).collect()
}

async fn local_stop(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let _admission_guard = state.admission_gate.lock().await;
    let current_epoch = state.fencing_epoch.load(Ordering::Acquire);
    if state.kill_latch_path.is_file()
        && read_stop_epoch_marker(state.kill_latch_path.as_ref()) == Some(current_epoch)
    {
        return Ok((
            StatusCode::OK,
            Json(json!({
                "stopped": true,
                "fencing_epoch": current_epoch,
                "duplicate": true
            })),
        ));
    }
    if !state.kill_latch_path.is_file() {
        match write_new_durable_file(state.kill_latch_path.as_ref(), b"owner-stop-v1\n") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(internal_error(error)),
        }
    }
    let fencing_epoch = state
        .ledger
        .advance_fencing_epoch("controller")
        .map_err(internal_error)?;
    state.fencing_epoch.store(fencing_epoch, Ordering::Release);
    write_stop_epoch_marker(state.kill_latch_path.as_ref(), fencing_epoch)
        .map_err(internal_error)?;
    for cancellation in state.model_cancellations.lock().await.values() {
        let _ = cancellation.send(true);
    }
    state
        .ledger
        .append(
            "fabric.owner_stop",
            "local-fabric",
            &json!({"source": "local-api", "fencing_epoch": fencing_epoch}),
        )
        .map_err(internal_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"stopped": true, "fencing_epoch": fencing_epoch})),
    ))
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
    let _admission_guard = state.admission_gate.lock().await;
    let marker = stop_epoch_marker(state.kill_latch_path.as_ref());
    if marker.is_file() {
        std::fs::remove_file(marker).map_err(internal_error)?;
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
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "stopped": false,
            "fencing_epoch": state.fencing_epoch.load(Ordering::Acquire)
        })),
    ))
}

fn stop_epoch_marker(kill_latch_path: &std::path::Path) -> PathBuf {
    kill_latch_path.with_extension("epoch")
}

fn read_stop_epoch_marker(kill_latch_path: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(stop_epoch_marker(kill_latch_path))
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

fn write_stop_epoch_marker(
    kill_latch_path: &std::path::Path,
    fencing_epoch: u64,
) -> std::io::Result<()> {
    let marker = stop_epoch_marker(kill_latch_path);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&marker)?;
    use std::io::Write as _;
    file.write_all(format!("{fencing_epoch}\n").as_bytes())?;
    file.sync_all()?;
    sync_parent_directory(&marker)
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
    validate_offer_identity_binding(&identity, &offer)
        .map_err(|error| (StatusCode::UNAUTHORIZED, Json(json!({"error": error}))))?;
    validate_model_runtime_contracts(&offer)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(json!({"error": error}))))?;
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

fn validate_offer_identity_binding(
    identity: &NodeIdentityV1,
    offer: &ResourceOfferV1,
) -> Result<(), String> {
    if offer.resources.is_empty() {
        return Err("resource offer must contain at least one exact resource".into());
    }
    let expected = rampage_policy::device_kind_label(identity.device_kind);
    if offer
        .resources
        .iter()
        .any(|resource| resource.labels.get("device_kind").map(String::as_str) != Some(expected))
    {
        return Err("resource device class does not match the enrolled native identity".into());
    }
    if matches!(
        identity.device_kind,
        DeviceKind::Phone | DeviceKind::Tablet | DeviceKind::Console
    ) && offer.availability.battery_percent.is_none()
    {
        return Err("edge offers must include a native battery observation".into());
    }
    Ok(())
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

async fn list_workload_capabilities(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let now = chrono::Utc::now();
    let offers = state.offers.read().map_err(lock_error)?;
    let mut nodes = offers
        .values()
        .filter(|offer| offer.expires_at > now)
        .map(|offer| {
            json!({
                "node_id": offer.node_id,
                "offer_id": offer.offer_id,
                "observed_at": offer.observed_at,
                "expires_at": offer.expires_at,
                "signed_offer": !offer.signature.is_empty(),
                "capabilities": offer.workload_capabilities
            })
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| left["node_id"].as_str().cmp(&right["node_id"].as_str()));
    Ok(Json(json!({
        "schema": "rampage.workload-capability-inventory.v1",
        "authority": "exact_adapter_operation_from_verified_signed_offer",
        "candidate_authority": false,
        "nodes": nodes
    })))
}

async fn self_scan(
    State(state): State<AppState>,
) -> Result<Json<FabricDiagnosticReport>, (StatusCode, Json<Value>)> {
    refresh_diagnostics(&state).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error})),
        )
    })?;
    let report = state
        .diagnostic_report
        .read()
        .map_err(lock_error)?
        .clone()
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "fabric diagnostic report is not available"})),
            )
        })?;
    Ok(Json(report))
}

async fn run_diagnostic_loop(state: AppState) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = refresh_diagnostics(&state) {
            warn!(%error, "Rampage fabric self-scan failed");
        }
    }
}

const MAX_AUTONOMOUS_REPAIRS_PER_CYCLE: usize = 4;
const MAX_REPLICA_PROBES_PER_CYCLE: usize = 4;
const MAX_REPLICA_VERIFICATION_BYTES_PER_CYCLE: u64 = 128 * 1024 * 1024;

async fn run_storage_repair_loop(state: AppState) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = reconcile_protected_artifacts(&state).await {
            warn!(%error, "Rampage protected-storage reconciliation failed");
        }
    }
}

async fn repair_protected_artifacts(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    reconcile_protected_artifacts(&state)
        .await
        .map_err(internal_error)?;
    refresh_diagnostics(&state).map_err(internal_error)?;
    let evidence = state
        .replica_evidence
        .read()
        .map_err(lock_error)?
        .values()
        .filter(|receipt| receipt.is_valid_at(chrono::Utc::now()))
        .count();
    Ok(Json(json!({
        "schema": "rampage.protected-storage-reconciliation.v1",
        "status": "reconciled",
        "fresh_replica_receipts": evidence,
        "per_change_approval_required": false,
        "authority_expansion": "denied"
    })))
}

async fn reconcile_protected_artifacts(state: &AppState) -> Result<(), String> {
    let _admission_guard = state.admission_gate.lock().await;
    if state.kill_latch_path.is_file() {
        return Ok(());
    }
    let now = chrono::Utc::now();
    let offers = state
        .offers
        .read()
        .map_err(|_| "offer registry lock is poisoned".to_string())?
        .values()
        .filter(|offer| offer.expires_at > now && offer.mesh_endpoint.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let mut protected = state
        .artifact_replicas
        .read()
        .map_err(|_| "artifact replica registry lock is poisoned".to_string())?
        .iter()
        .filter(|(_, artifact)| artifact.storage_class == StorageClass::Protected)
        .map(|((digest, node_id), artifact)| (digest.clone(), *node_id, artifact.clone()))
        .collect::<Vec<_>>();
    protected.sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
    let evidence = state
        .replica_evidence
        .read()
        .map_err(|_| "replica evidence registry lock is poisoned".to_string())?
        .clone();
    let stale = protected
        .iter()
        .filter(|(digest, node_id, _)| {
            !evidence
                .get(&(digest.clone(), *node_id))
                .is_some_and(|receipt| receipt.is_valid_at(now))
        })
        .cloned()
        .collect::<Vec<_>>();
    let selected_probes = select_replica_probes(
        &stale,
        state
            .storage_probe_cursor
            .fetch_add(MAX_REPLICA_PROBES_PER_CYCLE as u64, Ordering::AcqRel),
    );
    let mut by_digest = BTreeMap::<String, (ArtifactRefV1, BTreeSet<Uuid>)>::new();
    for (digest, node_id, artifact) in protected {
        let entry = by_digest
            .entry(digest)
            .or_insert_with(|| (artifact, BTreeSet::new()));
        entry.1.insert(node_id);
    }
    let mut repairs = 0_usize;
    for (digest, (remote_artifact, recorded_nodes)) in by_digest {
        let known_nodes = recorded_nodes.clone();
        let mut verified_nodes = recorded_nodes
            .iter()
            .filter(|node_id| {
                evidence
                    .get(&(digest.clone(), **node_id))
                    .is_some_and(|receipt| receipt.is_valid_at(now))
            })
            .copied()
            .collect::<BTreeSet<_>>();
        let verification_complete = recorded_nodes.iter().all(|node_id| {
            verified_nodes.contains(node_id)
                || selected_probes.contains(&(digest.clone(), *node_id))
        });
        for node_id in recorded_nodes {
            if !selected_probes.contains(&(digest.clone(), node_id)) {
                continue;
            }
            let Some(offer) = offers.iter().find(|offer| offer.node_id == node_id) else {
                continue;
            };
            match probe_replica(state, offer, &remote_artifact).await {
                Ok(receipt) => {
                    state
                        .replica_evidence
                        .write()
                        .map_err(|_| "replica evidence registry lock is poisoned".to_string())?
                        .insert((digest.clone(), node_id), receipt.clone());
                    state
                        .ledger
                        .append(
                            "artifact.replica.verified",
                            &digest,
                            &json!({
                                "node_id": node_id,
                                "artifact": remote_artifact,
                                "replica_receipt": receipt
                            }),
                        )
                        .map_err(|error| error.to_string())?;
                    verified_nodes.insert(node_id);
                }
                Err(error) => invalidate_replica(state, &digest, node_id, &error.to_string())?,
            }
        }
        if !verification_complete
            || verified_nodes.len() >= 2
            || repairs >= MAX_AUTONOMOUS_REPAIRS_PER_CYCLE
        {
            continue;
        }
        let source = match state.artifact_store.head(&digest) {
            Ok(source) => source,
            Err(error) => {
                state
                    .ledger
                    .append(
                        "artifact.repair.blocked",
                        &digest,
                        &json!({"reason": format!("local source unavailable: {error}")}),
                    )
                    .map_err(|ledger_error| ledger_error.to_string())?;
                continue;
            }
        };
        for offer in offers
            .iter()
            .filter(|offer| !known_nodes.contains(&offer.node_id))
        {
            if verified_nodes.len() >= 2 || repairs >= MAX_AUTONOMOUS_REPAIRS_PER_CYCLE {
                break;
            }
            let Some(endpoint_record) = &offer.mesh_endpoint else {
                continue;
            };
            let endpoint = match rampage_mesh::endpoint_addr_from_record(endpoint_record) {
                Ok(endpoint) => endpoint,
                Err(_) => continue,
            };
            match replicate_to_offer(state, offer, endpoint, &source, StorageClass::Protected).await
            {
                Ok(outcome) => {
                    let key = (digest.clone(), offer.node_id);
                    state
                        .artifact_replicas
                        .write()
                        .map_err(|_| "artifact replica registry lock is poisoned".to_string())?
                        .insert(key.clone(), outcome.artifact.clone());
                    state
                        .replica_evidence
                        .write()
                        .map_err(|_| "replica evidence registry lock is poisoned".to_string())?
                        .insert(key, outcome.receipt.clone());
                    state
                        .ledger
                        .append(
                            "artifact.repaired",
                            &digest,
                            &json!({
                                "node_id": offer.node_id,
                                "artifact": outcome.artifact,
                                "storage_lease_id": outcome.lease_id,
                                "transfer_session_id": outcome.session_id,
                                "resumed_chunks": outcome.resumed_chunks,
                                "chunk_count": outcome.chunk_count,
                                "replica_receipt": outcome.receipt,
                                "autonomous_threshold": "fewer_than_two_fresh_independent_receipts"
                            }),
                        )
                        .map_err(|error| error.to_string())?;
                    verified_nodes.insert(offer.node_id);
                    repairs += 1;
                }
                Err(error) => {
                    state
                        .ledger
                        .append(
                            "artifact.repair.attempt_failed",
                            &digest,
                            &json!({"node_id": offer.node_id, "reason": error.to_string()}),
                        )
                        .map_err(|ledger_error| ledger_error.to_string())?;
                }
            }
        }
    }
    Ok(())
}

fn select_replica_probes(
    candidates: &[(String, Uuid, ArtifactRefV1)],
    cursor: u64,
) -> BTreeSet<(String, Uuid)> {
    if candidates.is_empty() {
        return BTreeSet::new();
    }
    let start = cursor as usize % candidates.len();
    let mut selected = BTreeSet::new();
    let mut bytes = 0_u64;
    for offset in 0..candidates.len() {
        if selected.len() >= MAX_REPLICA_PROBES_PER_CYCLE {
            break;
        }
        let candidate = &candidates[(start + offset) % candidates.len()];
        let Some(next_bytes) = bytes.checked_add(candidate.2.size_bytes) else {
            continue;
        };
        if next_bytes > MAX_REPLICA_VERIFICATION_BYTES_PER_CYCLE && !selected.is_empty() {
            continue;
        }
        bytes = next_bytes;
        selected.insert((candidate.0.clone(), candidate.1));
    }
    selected
}

fn invalidate_replica(
    state: &AppState,
    digest: &str,
    node_id: Uuid,
    reason: &str,
) -> Result<(), String> {
    let key = (digest.to_string(), node_id);
    state
        .replica_evidence
        .write()
        .map_err(|_| "replica evidence registry lock is poisoned".to_string())?
        .remove(&key);
    state
        .ledger
        .append(
            "artifact.replica.invalidated",
            digest,
            &json!({"digest": digest, "node_id": node_id, "reason": reason}),
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn probe_replica(
    state: &AppState,
    offer: &ResourceOfferV1,
    artifact: &ArtifactRefV1,
) -> anyhow::Result<ArtifactReplicaReceiptV1> {
    let endpoint_record = offer
        .mesh_endpoint
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("replica has no signed mesh endpoint"))?;
    let endpoint = rampage_mesh::endpoint_addr_from_record(endpoint_record)?;
    let lease = state.governor.authorize_storage_at_epoch(
        offer,
        &artifact.digest,
        artifact.size_bytes,
        artifact.storage_class,
        ArtifactTransferOperation::Get,
        state.fencing_epoch.load(Ordering::Acquire),
    )?;
    state
        .ledger
        .append("storage.lease.issued", &lease.lease_id.to_string(), &lease)?;
    let session_id = artifact_transfer_session_id(
        offer.node_id,
        &artifact.digest,
        ArtifactTransferOperation::Get,
    );
    let challenge_nonce = Uuid::new_v4().simple().to_string();
    let (remote, receipt) = rampage_mesh::artifact_head(
        &state.mesh.endpoint(),
        rampage_mesh::ArtifactTransferContext {
            destination: endpoint,
            lease: lease.clone(),
            media_type: artifact.media_type.clone(),
            session_id,
            challenge_nonce: challenge_nonce.clone(),
        },
    )
    .await?;
    anyhow::ensure!(
        remote.digest == artifact.digest
            && remote.size_bytes == artifact.size_bytes
            && remote.media_type == artifact.media_type
            && remote.storage_class == artifact.storage_class
            && remote.encrypted,
        "replica probe returned a different artifact contract"
    );
    verify_replica_evidence(state, offer, &lease, session_id, &challenge_nonce, &receipt)?;
    Ok(receipt)
}

fn refresh_diagnostics(state: &AppState) -> Result<(), String> {
    let now = chrono::Utc::now();
    let nodes = state
        .nodes
        .read()
        .map_err(|_| "node registry lock is poisoned".to_string())?
        .clone();
    let offers = state
        .offers
        .read()
        .map_err(|_| "offer registry lock is poisoned".to_string())?
        .clone();
    let active_assignments = state
        .assignments
        .read()
        .map_err(|_| "assignment registry lock is poisoned".to_string())?
        .values()
        .filter(|assignment| assignment.lease.expires_at > now)
        .count();
    let artifact_replicas = state
        .artifact_replicas
        .read()
        .map_err(|_| "artifact replica registry lock is poisoned".to_string())?
        .clone();
    let replica_evidence = state
        .replica_evidence
        .read()
        .map_err(|_| "replica evidence registry lock is poisoned".to_string())?
        .clone();
    let events = state
        .ledger
        .latest_events(512)
        .map_err(|error| format!("diagnostic evidence read failed: {error}"))?;
    let report = build_diagnostic_report(
        now,
        &nodes,
        &offers,
        active_assignments,
        ArtifactDiagnosticState {
            replicas: &artifact_replicas,
            evidence: &replica_evidence,
        },
        &events,
        state.kill_latch_path.is_file(),
    );
    let constraints = derive_autonomous_constraints(&state.governor, &report);
    let constraints_changed = *state
        .autonomous_constraints
        .read()
        .map_err(|_| "autonomous constraint lock is poisoned".to_string())?
        != constraints;
    if constraints_changed {
        state
            .ledger
            .append(
                "diagnostic.autonomy.applied",
                &constraints.evidence_digest,
                &constraints,
            )
            .map_err(|error| format!("autonomous constraint evidence write failed: {error}"))?;
        *state
            .autonomous_constraints
            .write()
            .map_err(|_| "autonomous constraint lock is poisoned".to_string())? = constraints;
    }
    let changed = state
        .diagnostic_digest
        .read()
        .map_err(|_| "diagnostic digest lock is poisoned".to_string())?
        .as_ref()
        != Some(&report.evidence_digest);
    if changed {
        state
            .ledger
            .append(
                "diagnostic.self_scan.completed",
                &report.evidence_digest,
                &report,
            )
            .map_err(|error| format!("diagnostic evidence write failed: {error}"))?;
        *state
            .diagnostic_digest
            .write()
            .map_err(|_| "diagnostic digest lock is poisoned".to_string())? =
            Some(report.evidence_digest.clone());
    }
    *state
        .diagnostic_report
        .write()
        .map_err(|_| "diagnostic report lock is poisoned".to_string())? = Some(report);
    Ok(())
}

fn derive_autonomous_constraints(
    governor: &Governor,
    report: &FabricDiagnosticReport,
) -> AutonomousConstraints {
    let mut excluded_nodes = BTreeMap::<Uuid, Vec<String>>::new();
    for finding in &report.findings {
        let action = match finding.code {
            "THERMAL_HEADROOM_CONSTRAINED" => "suppress_thermally_constrained_node",
            "BATTERY_RESERVE_CONSTRAINED" => "suppress_low_battery_node",
            "AUTHENTICATED_ROUTE_MISSING" | "AUTHENTICATED_ROUTE_EMPTY" => {
                "suppress_unroutable_node"
            }
            _ => continue,
        };
        if governor.authorize_diagnostic_action(action).is_err() {
            continue;
        }
        let Ok(node_id) = Uuid::parse_str(&finding.scope) else {
            continue;
        };
        excluded_nodes
            .entry(node_id)
            .or_default()
            .push(finding.code.to_string());
    }
    for reasons in excluded_nodes.values_mut() {
        reasons.sort();
        reasons.dedup();
    }
    AutonomousConstraints {
        evidence_digest: report.evidence_digest.clone(),
        excluded_nodes,
    }
}

fn placement_offers(state: &AppState) -> Result<Vec<ResourceOfferV1>, (StatusCode, Json<Value>)> {
    let excluded = state
        .autonomous_constraints
        .read()
        .map_err(lock_error)?
        .excluded_nodes
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    Ok(state
        .offers
        .read()
        .map_err(lock_error)?
        .values()
        .filter(|offer| !excluded.contains(&offer.node_id))
        .cloned()
        .collect())
}

struct ArtifactDiagnosticState<'a> {
    replicas: &'a HashMap<(String, Uuid), ArtifactRefV1>,
    evidence: &'a HashMap<(String, Uuid), ArtifactReplicaReceiptV1>,
}

fn build_diagnostic_report(
    now: chrono::DateTime<chrono::Utc>,
    nodes: &HashMap<Uuid, NodeIdentityV1>,
    offers: &HashMap<Uuid, ResourceOfferV1>,
    active_assignments: usize,
    artifacts: ArtifactDiagnosticState<'_>,
    events: &[LedgerEvent],
    kill_latch: bool,
) -> FabricDiagnosticReport {
    let mut findings = Vec::new();
    let live_offers = offers
        .values()
        .filter(|offer| offer.expires_at > now)
        .collect::<Vec<_>>();
    let expired_offers = offers.len().saturating_sub(live_offers.len());
    let live_node_ids = live_offers
        .iter()
        .map(|offer| offer.node_id)
        .collect::<BTreeSet<_>>();

    if kill_latch {
        findings.push(diagnostic_finding(
            DiagnosticSeverity::Critical,
            "OWNER_STOP_ACTIVE",
            "fabric",
            "The durable owner stop latch is active; no new work can be admitted.",
            "retain_stop_and_surface_state",
            "r0_configuration",
            false,
            "owner stop is never overridden by autonomy",
        ));
    }
    if nodes.is_empty() {
        findings.push(diagnostic_finding(
            DiagnosticSeverity::Warning,
            "NO_ENROLLED_CONTRIBUTORS",
            "fabric",
            "No contributor identity is enrolled, so the fabric has no remote capacity.",
            "prepare_one_time_enrollment",
            "r0_configuration",
            true,
            "invite remains short-lived and enrollment remains cryptographically bound",
        ));
    }
    for node_id in nodes
        .keys()
        .filter(|node_id| !live_node_ids.contains(node_id))
    {
        findings.push(diagnostic_finding(
            DiagnosticSeverity::Warning,
            "NODE_WITHOUT_LIVE_OFFER",
            node_id.to_string(),
            "The enrolled contributor has no unexpired signed resource offer.",
            "request_offer_refresh",
            "r0_configuration",
            true,
            "refresh only; no work is issued until a verified offer arrives",
        ));
    }

    let mut available_resources = BTreeMap::new();
    for offer in &live_offers {
        for resource in &offer.resources {
            let current = available_resources
                .entry(resource_class_name(resource.class))
                .or_insert(0_u64);
            *current = current.saturating_add(resource.available);
        }
        if offer.workload_capabilities.is_empty() {
            findings.push(diagnostic_finding(
                DiagnosticSeverity::Warning,
                "MISSING_WORKLOAD_CAPABILITY_CONTRACT",
                offer.node_id.to_string(),
                "The live offer predates operation-exact workload capability discovery.",
                "request_offer_refresh",
                "r0_configuration",
                true,
                "jobs remain fail-closed to the legacy adapter/resource contract",
            ));
        }
        if offer.availability.thermal_headroom_percent < 35 {
            findings.push(diagnostic_finding(
                DiagnosticSeverity::Warning,
                "THERMAL_HEADROOM_CONSTRAINED",
                offer.node_id.to_string(),
                format!(
                    "Thermal headroom is {}%, below the autonomous scheduling threshold of 35%.",
                    offer.availability.thermal_headroom_percent
                ),
                "reduce_or_preempt_contributor_load",
                "r0_configuration",
                true,
                "preempt restart-tolerant work; never increase the owner thermal envelope",
            ));
        }
        if !offer.availability.on_ac_power
            && offer
                .availability
                .battery_percent
                .is_some_and(|battery| battery < 40)
        {
            findings.push(diagnostic_finding(
                DiagnosticSeverity::Warning,
                "BATTERY_RESERVE_CONSTRAINED",
                offer.node_id.to_string(),
                format!(
                    "Battery is {}%, below the autonomous donation floor of 40%.",
                    offer.availability.battery_percent.unwrap_or_default()
                ),
                "preempt_mobile_contribution",
                "r0_configuration",
                true,
                "resume only after a fresh signed offer proves the battery floor",
            ));
        }
        match &offer.mesh_endpoint {
            None => findings.push(diagnostic_finding(
                DiagnosticSeverity::Info,
                "LOOPBACK_POLLING_ONLY",
                offer.node_id.to_string(),
                "The contributor uses the token-protected loopback polling lane and cannot receive mesh-only model or artifact traffic.",
                "refresh_signed_mesh_endpoint",
                "r0_configuration",
                false,
                "local polling jobs remain eligible; mesh-only operations still require a signed endpoint",
            )),
            Some(endpoint)
                if endpoint.direct_addresses.is_empty() && endpoint.relay_urls.is_empty() =>
            {
                findings.push(diagnostic_finding(
                    DiagnosticSeverity::Critical,
                    "AUTHENTICATED_ROUTE_EMPTY",
                    offer.node_id.to_string(),
                    "The signed mesh endpoint exposes neither a direct address nor an owner-controlled relay.",
                    "reprobe_nat_and_owner_relay",
                    "r1_allowlisted_source",
                    true,
                    "canary route must authenticate the enrolled endpoint before use",
                ));
            }
            Some(_) => {}
        }
        match &offer.link_benchmark {
            None => findings.push(diagnostic_finding(
                DiagnosticSeverity::Info,
                "LINK_QUALIFICATION_MISSING",
                offer.node_id.to_string(),
                "No fresh bounded link benchmark is attached; latency-sensitive placement stays conservative.",
                "run_bounded_link_probe",
                "r0_configuration",
                true,
                "probe is capped at the protocol transfer limit and grants no job authority",
            )),
            Some(link) => {
                if link.rtt_micros_p50 > 75_000 {
                    findings.push(diagnostic_finding(
                        DiagnosticSeverity::Warning,
                        "HIGH_LINK_LATENCY",
                        offer.node_id.to_string(),
                        format!(
                            "Measured median RTT is {:.1} ms; interactive speed-lane placement is suppressed.",
                            link.rtt_micros_p50 as f64 / 1_000.0
                        ),
                        "prefer_throughput_or_whole_workload_lane",
                        "r0_configuration",
                        true,
                        "placement change must improve measured completion time in shadow and canary",
                    ));
                }
                if link.uplink_bps.min(link.downlink_bps) < 25_000_000 {
                    findings.push(diagnostic_finding(
                        DiagnosticSeverity::Warning,
                        "LOW_LINK_BANDWIDTH",
                        offer.node_id.to_string(),
                        format!(
                            "The slower measured direction is {:.1} Mbps; transfer-heavy shards are suppressed.",
                            link.uplink_bps.min(link.downlink_bps) as f64 / 1_000_000.0
                        ),
                        "prefer_data_local_or_low_transfer_work",
                        "r0_configuration",
                        true,
                        "placement change must reduce measured bytes per successful result",
                    ));
                }
            }
        }
        if offer.adapters.contains("rampage.ollama.v1")
            && offer
                .model_runtimes
                .iter()
                .all(|runtime| runtime.installed_models.is_empty())
        {
            findings.push(diagnostic_finding(
                DiagnosticSeverity::Warning,
                "LOCAL_MODEL_INVENTORY_EMPTY",
                offer.node_id.to_string(),
                "Ollama is advertised but no exact locally installed model is executable.",
                "refresh_local_model_inventory",
                "r0_configuration",
                true,
                "cloud aliases and digestless models remain excluded",
            ));
        }
        if offer.availability.owner_idle
            && offer
                .resources
                .iter()
                .any(|resource| resource.available > 0)
        {
            findings.push(diagnostic_finding(
                DiagnosticSeverity::Info,
                "IDLE_CAPACITY_AVAILABLE",
                offer.node_id.to_string(),
                "The contributor reports idle capacity that can accept bounded preemptible work.",
                "prefer_idle_capacity",
                "r0_configuration",
                true,
                "owner activity, battery, thermal, lease, and deadline gates remain authoritative",
            ));
        }
    }

    let recent_cutoff = now - chrono::Duration::minutes(15);
    let recent_denials = events
        .iter()
        .filter(|event| {
            event.recorded_at >= recent_cutoff
                && matches!(
                    event.event_type.as_str(),
                    "job.blocked" | "mesh.request.denied"
                )
        })
        .count();
    let recent_failed_receipts = events
        .iter()
        .filter(|event| {
            event.recorded_at >= recent_cutoff
                && event.event_type == "job.receipted"
                && matches!(
                    event.payload.get("state").and_then(Value::as_str),
                    Some("failed" | "ambiguous" | "cancelled" | "fenced")
                )
        })
        .count();
    if recent_denials >= 3 {
        findings.push(diagnostic_finding(
            DiagnosticSeverity::Warning,
            "REPEATED_AUTHORITY_DENIALS",
            "fabric",
            format!(
                "{recent_denials} requests were denied in the bounded 15-minute evidence window."
            ),
            "cluster_denial_causes_and_adjust_safe_placement",
            "r1_allowlisted_source",
            true,
            "never broaden adapter, resource, network, or identity authority",
        ));
    }
    if recent_failed_receipts >= 2 {
        findings.push(diagnostic_finding(
            DiagnosticSeverity::Critical,
            "REPEATED_EXECUTION_FAILURES",
            "fabric",
            format!("{recent_failed_receipts} terminal execution failures occurred in the bounded 15-minute evidence window."),
            "quarantine_failing_route_or_adapter",
            "r1_allowlisted_source",
            true,
            "canary recovery must produce verified receipts before normal traffic resumes",
        ));
    }

    let mut protected_replica_counts = HashMap::<String, usize>::new();
    for (key @ (digest, _), artifact) in artifacts.replicas {
        if artifact.storage_class == StorageClass::Protected {
            protected_replica_counts.entry(digest.clone()).or_default();
            if artifacts
                .evidence
                .get(key)
                .is_some_and(|receipt| receipt.is_valid_at(now))
            {
                *protected_replica_counts.entry(digest.clone()).or_default() += 1;
            }
        }
    }
    let under_replicated_protected_artifacts = protected_replica_counts
        .values()
        .filter(|replicas| **replicas < 2)
        .count();
    if under_replicated_protected_artifacts > 0 {
        findings.push(diagnostic_finding(
            DiagnosticSeverity::Warning,
            "PROTECTED_ARTIFACT_UNDER_REPLICATED",
            "protected_store",
            format!(
                "{under_replicated_protected_artifacts} protected artifact(s) have fewer than two evidenced contributor replicas."
            ),
            "schedule_encrypted_replica_repair",
            "r1_allowlisted_source",
            true,
            "repair requires a signed storage lease, digest verification, and an independent destination",
        ));
    }

    findings.sort_by(|left, right| {
        left.severity
            .cmp(&right.severity)
            .reverse()
            .then_with(|| left.code.cmp(right.code))
            .then_with(|| left.scope.cmp(&right.scope))
    });
    let critical = findings
        .iter()
        .filter(|finding| finding.severity == DiagnosticSeverity::Critical)
        .count() as u8;
    let warnings = findings
        .iter()
        .filter(|finding| finding.severity == DiagnosticSeverity::Warning)
        .count() as u8;
    let health_score = if kill_latch {
        0
    } else {
        100_u8
            .saturating_sub(critical.saturating_mul(25))
            .saturating_sub(warnings.saturating_mul(8))
    };
    let status = if kill_latch {
        "stopped"
    } else if health_score < 70 {
        "degraded"
    } else if health_score < 90 {
        "attention"
    } else {
        "healthy"
    };
    let metrics = DiagnosticMetrics {
        enrolled_nodes: nodes.len(),
        live_offers: live_offers.len(),
        expired_offers,
        active_assignments,
        protected_artifacts: protected_replica_counts.len(),
        under_replicated_protected_artifacts,
        recent_denials,
        recent_failed_receipts,
        available_resources,
    };
    let autonomy = DiagnosticAutonomy {
        mode: "deterministic_thresholded_governor",
        per_change_approval_required: false,
        eligible_within_envelope: vec!["r0_configuration", "r1_allowlisted_source"],
        authority_expansion: "automatically_denied_outside_owner_envelope",
        promotion_requirements: vec![
            "schema_policy_static",
            "deterministic_replay",
            "quality_reliability_cost",
            "sealed_holdout",
            "adversarial_security",
            "independent_replication",
            "shadow",
            "canary_rollback",
        ],
    };
    let digest_value = json!({
        "status": status,
        "health_score": health_score,
        "metrics": &metrics,
        "autonomy": &autonomy,
        "findings": &findings,
    });
    let evidence_digest = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(
            serde_json::to_vec(&digest_value).expect("diagnostic report is serializable")
        ))
    );
    FabricDiagnosticReport {
        schema: "rampage.fabric-diagnostic-report.v1",
        generated_at: now,
        status,
        health_score,
        evidence_digest,
        metrics,
        autonomy,
        findings,
    }
}

#[allow(clippy::too_many_arguments)] // each call remains a self-contained, auditable policy rule
fn diagnostic_finding(
    severity: DiagnosticSeverity,
    code: &'static str,
    scope: impl Into<String>,
    evidence: impl Into<String>,
    action: &'static str,
    risk: &'static str,
    auto_eligible: bool,
    threshold: &'static str,
) -> DiagnosticFinding {
    DiagnosticFinding {
        severity,
        code,
        scope: scope.into(),
        evidence: evidence.into(),
        proposal: DiagnosticProposal {
            action,
            risk,
            auto_eligible,
            threshold,
            required_gates: vec![
                "deterministic_replay",
                "adversarial_security",
                "shadow",
                "canary_rollback",
            ],
            rollback: "automatic_on_threshold_breach",
        },
    }
}

fn resource_class_name(class: ResourceClass) -> &'static str {
    match class {
        ResourceClass::CpuCompute => "cpu_compute",
        ResourceClass::GpuCompute => "gpu_compute",
        ResourceClass::NpuCompute => "npu_compute",
        ResourceClass::GpuMemory => "gpu_memory",
        ResourceClass::RamWorkingSet => "ram_working_set",
        ResourceClass::RamCache => "ram_cache",
        ResourceClass::StorageCache => "storage_cache",
        ResourceClass::StorageScratch => "storage_scratch",
        ResourceClass::ProtectedStore => "protected_store",
        ResourceClass::NetworkFetch => "network_fetch",
        ResourceClass::NetworkRelay => "network_relay",
        ResourceClass::Toolchain => "toolchain",
        ResourceClass::Runtime => "runtime",
        ResourceClass::Codec => "codec",
        ResourceClass::LicensedService => "licensed_service",
    }
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
    let offers = placement_offers(&state)?;
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
        .authorize_job_at_epoch(
            &job,
            offer,
            offer.node_id,
            state.fencing_epoch.load(Ordering::Acquire),
        )
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

    let offers = placement_offers(&state)?;
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
                .authorize_job_at_epoch(
                    job,
                    offer,
                    placement.node_id,
                    state.fencing_epoch.load(Ordering::Acquire),
                )
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
    let offers = placement_offers(&state)?;
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
    let offers = placement_offers(&state)?;
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
    let offers = placement_offers(&state)?;
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

async fn relay_access_manifest(
    State(state): State<AppState>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let now = chrono::Utc::now();
    let mut allowed_endpoint_ids = state
        .nodes
        .read()
        .map_err(lock_error)?
        .values()
        .map(|identity| identity.public_key.clone())
        .collect::<BTreeSet<_>>();
    allowed_endpoint_ids.insert(state.mesh.endpoint_id());
    let mut manifest = RelayAccessManifestV1 {
        schema: RelayAccessManifestV1::SCHEMA.into(),
        fabric_id: String::new(),
        generation: state.fencing_epoch.load(Ordering::Acquire),
        allowed_endpoint_ids,
        issued_at: now,
        expires_at: now + chrono::Duration::minutes(10),
        signature: String::new(),
    };
    state.governor.sign_relay_access_manifest(&mut manifest);
    let mut response = Json(manifest).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn authorize_promotion_canary(
    State(state): State<AppState>,
    Json(candidate): Json<PromotionCandidateV1>,
) -> Result<(StatusCode, Json<PromotionCanaryLeaseV1>), (StatusCode, Json<Value>)> {
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
        ));
    }
    let _guard = state.admission_gate.lock().await;
    let subject = candidate.proposal_id.to_string();
    for event in state
        .ledger
        .events_for_subject(&subject, 128)
        .map_err(internal_error)?
    {
        if event.event_type == "promotion.candidate.proposed" {
            let existing: PromotionCandidateV1 =
                serde_json::from_value(event.payload).map_err(internal_error)?;
            if existing.candidate_digest != candidate.candidate_digest
                || existing.project_id != candidate.project_id
                || existing.changed_paths != candidate.changed_paths
            {
                return Err((
                    StatusCode::CONFLICT,
                    Json(json!({"error": "proposal id is already bound to another candidate"})),
                ));
            }
        } else if event.event_type == "promotion.canary.lease.issued" {
            let existing: PromotionCanaryLeaseV1 =
                serde_json::from_value(event.payload).map_err(internal_error)?;
            if existing.candidate_digest != candidate.candidate_digest
                || existing.project_id != candidate.project_id
            {
                return Err((
                    StatusCode::CONFLICT,
                    Json(json!({"error": "proposal id is already bound to another canary"})),
                ));
            }
            if existing.is_active_at(
                chrono::Utc::now(),
                state.fencing_epoch.load(Ordering::Acquire),
            ) {
                return Ok((StatusCode::OK, Json(existing)));
            }
        }
    }
    let lease = state
        .governor
        .authorize_promotion_canary_at_epoch(
            &candidate,
            state.fencing_epoch.load(Ordering::Acquire),
        )
        .map_err(|error| {
            let status = match error {
                rampage_policy::Denial::InvalidPromotionEvidence
                | rampage_policy::Denial::PromotionRiskMismatch => StatusCode::BAD_REQUEST,
                rampage_policy::Denial::KillLatch => StatusCode::LOCKED,
                _ => StatusCode::FORBIDDEN,
            };
            (status, Json(json!({"error": error.to_string()})))
        })?;
    state
        .ledger
        .append("promotion.candidate.proposed", &subject, &candidate)
        .map_err(internal_error)?;
    state
        .ledger
        .append("promotion.canary.lease.issued", &subject, &lease)
        .map_err(internal_error)?;
    Ok((StatusCode::CREATED, Json(lease)))
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
    let _admission_guard = state.admission_gate.lock().await;
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
    let current_epoch = state.fencing_epoch.load(Ordering::Acquire);
    let now = chrono::Utc::now();
    let Some(assignment) = assignments
        .values_mut()
        .filter(|assignment| {
            !assignment.claimed
                && assignment.lease.node_id == query.node_id
                && assignment.lease.is_active_at(now, current_epoch)
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
    let _admission_guard = state.admission_gate.lock().await;
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
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
        || assignment.lease.fencing_epoch != state.fencing_epoch.load(Ordering::Acquire)
        || receipt.started_at < assignment.lease.issued_at
        || receipt.started_at > assignment.lease.expires_at
    {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "receipt does not match the current claimed lease authority"})),
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

struct ReplicationOutcome {
    artifact: ArtifactRefV1,
    receipt: ArtifactReplicaReceiptV1,
    lease_id: Uuid,
    session_id: Uuid,
    resumed_chunks: usize,
    chunk_count: usize,
}

fn artifact_transfer_session_id(
    node_id: Uuid,
    digest: &str,
    operation: ArtifactTransferOperation,
) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"rampage.artifact-transfer-session.v1\0");
    hasher.update(node_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(match operation {
        ArtifactTransferOperation::Put => b"put".as_slice(),
        ArtifactTransferOperation::Get => b"get".as_slice(),
    });
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn verify_replica_evidence(
    state: &AppState,
    offer: &ResourceOfferV1,
    lease: &StorageLeaseV1,
    session_id: Uuid,
    challenge_nonce: &str,
    receipt: &ArtifactReplicaReceiptV1,
) -> anyhow::Result<()> {
    let identity = state
        .nodes
        .read()
        .map_err(|_| anyhow::anyhow!("node identity lock is poisoned"))?
        .get(&offer.node_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("replica node identity is unavailable"))?;
    verify_artifact_replica_receipt(&identity, receipt)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    anyhow::ensure!(
        receipt.session_id == session_id
            && receipt.lease_id == lease.lease_id
            && receipt.node_id == offer.node_id
            && receipt.digest == lease.digest
            && receipt.size_bytes == lease.size_bytes
            && receipt.storage_class == lease.storage_class
            && receipt.challenge_nonce == challenge_nonce
            && receipt.fencing_epoch == lease.fencing_epoch,
        "replica receipt is not bound to the exact transfer authority"
    );
    Ok(())
}

async fn replicate_to_offer(
    state: &AppState,
    offer: &ResourceOfferV1,
    endpoint: iroh::EndpointAddr,
    source: &ArtifactRefV1,
    storage_class: StorageClass,
) -> anyhow::Result<ReplicationOutcome> {
    let lease = state.governor.authorize_storage_at_epoch(
        offer,
        &source.digest,
        source.size_bytes,
        storage_class,
        ArtifactTransferOperation::Put,
        state.fencing_epoch.load(Ordering::Acquire),
    )?;
    state
        .ledger
        .append("storage.lease.issued", &lease.lease_id.to_string(), &lease)?;
    let session_id = artifact_transfer_session_id(
        offer.node_id,
        &source.digest,
        ArtifactTransferOperation::Put,
    );
    let progress = rampage_mesh::artifact_put(
        &state.mesh.endpoint(),
        rampage_mesh::ArtifactTransferContext {
            destination: endpoint.clone(),
            lease: lease.clone(),
            media_type: source.media_type.clone(),
            session_id,
            challenge_nonce: Uuid::new_v4().simple().to_string(),
        },
    )
    .await?;
    anyhow::ensure!(
        progress.is_valid()
            && progress.session_id == session_id
            && progress.digest == source.digest
            && progress.size_bytes == source.size_bytes,
        "remote transfer progress is malformed or mismatched"
    );
    let resumed_chunks = progress.received_chunks.len();
    let chunk_count = progress.chunk_count as usize;
    for index in progress.missing_chunks {
        let chunk = state.artifact_store.get_chunk(&source.digest, index)?;
        let chunk_digest = format!("sha256:{}", hex::encode(Sha256::digest(&chunk)));
        let updated = rampage_mesh::artifact_put_chunk(
            &state.mesh.endpoint(),
            rampage_mesh::ArtifactTransferContext {
                destination: endpoint.clone(),
                lease: lease.clone(),
                media_type: source.media_type.clone(),
                session_id,
                challenge_nonce: Uuid::new_v4().simple().to_string(),
            },
            index,
            chunk_digest,
            &chunk,
        )
        .await?;
        anyhow::ensure!(
            updated.is_valid()
                && updated.session_id == session_id
                && updated.digest == source.digest,
            "remote transfer progress changed its content binding"
        );
    }
    let challenge_nonce = Uuid::new_v4().simple().to_string();
    let (artifact, receipt) = rampage_mesh::artifact_commit(
        &state.mesh.endpoint(),
        rampage_mesh::ArtifactTransferContext {
            destination: endpoint,
            lease: lease.clone(),
            media_type: source.media_type.clone(),
            session_id,
            challenge_nonce: challenge_nonce.clone(),
        },
    )
    .await?;
    anyhow::ensure!(
        artifact.digest == source.digest
            && artifact.size_bytes == source.size_bytes
            && artifact.media_type == source.media_type
            && artifact.storage_class == storage_class
            && artifact.encrypted,
        "remote artifact does not match the source contract"
    );
    verify_replica_evidence(state, offer, &lease, session_id, &challenge_nonce, &receipt)?;
    Ok(ReplicationOutcome {
        artifact,
        receipt,
        lease_id: lease.lease_id,
        session_id,
        resumed_chunks,
        chunk_count,
    })
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
        let outcome = replicate_to_offer(state, offer, endpoint, &local, input.storage_class)
            .await
            .map_err(|error| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("input staging failed: {error}")})),
                )
            })?;
        let remote = outcome.artifact;
        state
            .artifact_replicas
            .write()
            .map_err(lock_error)?
            .insert((remote.digest.clone(), offer.node_id), remote.clone());
        state.replica_evidence.write().map_err(lock_error)?.insert(
            (remote.digest.clone(), offer.node_id),
            outcome.receipt.clone(),
        );
        state
            .ledger
            .append(
                "artifact.input.staged",
                &remote.digest,
                &json!({
                    "node_id": offer.node_id,
                    "job_id": job.job_id,
                    "artifact": remote,
                    "storage_lease_id": outcome.lease_id,
                    "transfer_session_id": outcome.session_id,
                    "resumed_chunks": outcome.resumed_chunks,
                    "chunk_count": outcome.chunk_count,
                    "replica_receipt": outcome.receipt
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
    let _admission_guard = state.admission_gate.lock().await;
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
    if request.media_type != source.media_type {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "replica media type must match the source artifact"})),
        ));
    }
    let (offer, endpoint) = remote_offer(&state, request.node_id)?;
    let outcome = replicate_to_offer(&state, &offer, endpoint, &source, request.storage_class)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": error.to_string()})),
            )
        })?;
    let remote_artifact = outcome.artifact;
    state.artifact_replicas.write().map_err(lock_error)?.insert(
        (remote_artifact.digest.clone(), request.node_id),
        remote_artifact.clone(),
    );
    state.replica_evidence.write().map_err(lock_error)?.insert(
        (remote_artifact.digest.clone(), request.node_id),
        outcome.receipt.clone(),
    );
    state
        .ledger
        .append(
            "artifact.replicated",
            &remote_artifact.digest,
            &json!({
                "node_id": request.node_id,
                "artifact": remote_artifact,
                "storage_lease_id": outcome.lease_id,
                "transfer_session_id": outcome.session_id,
                "resumed_chunks": outcome.resumed_chunks,
                "chunk_count": outcome.chunk_count,
                "replica_receipt": outcome.receipt
            }),
        )
        .map_err(internal_error)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "artifact": remote_artifact,
            "node_id": request.node_id,
            "storage_lease_id": outcome.lease_id,
            "transfer_session_id": outcome.session_id,
            "resumed_chunks": outcome.resumed_chunks,
            "chunk_count": outcome.chunk_count,
            "replica_receipt": outcome.receipt
        })),
    ))
}

async fn retrieve_artifact(
    State(state): State<AppState>,
    Json(request): Json<ArtifactRetrieveRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let _admission_guard = state.admission_gate.lock().await;
    if state.kill_latch_path.is_file() {
        return Err((
            StatusCode::LOCKED,
            Json(json!({"error": "owner kill latch is active"})),
        ));
    }
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
    let local_contract = state.artifact_store.head(&remote_artifact.digest).ok();
    let (sink_storage_class, sink_required_replicas) =
        retrieval_sink_contract(&remote_artifact, local_contract.as_ref())
            .map_err(|error| (StatusCode::CONFLICT, Json(json!({"error": error}))))?;
    let session_id = artifact_transfer_session_id(
        request.node_id,
        &remote_artifact.digest,
        ArtifactTransferOperation::Get,
    );
    let mut lease = state
        .governor
        .authorize_storage_at_epoch(
            &offer,
            &remote_artifact.digest,
            remote_artifact.size_bytes,
            remote_artifact.storage_class,
            ArtifactTransferOperation::Get,
            state.fencing_epoch.load(Ordering::Acquire),
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
    let initial_spec = rampage_storage::ResumablePutSpec {
        session_id: session_id.simple().to_string(),
        lease_id: lease.lease_id.simple().to_string(),
        authority_scope: "governor".into(),
        fencing_epoch: lease.fencing_epoch,
        authority_nonce: lease.nonce.clone(),
        expires_at: lease.expires_at,
        digest: remote_artifact.digest.clone(),
        size_bytes: remote_artifact.size_bytes,
        media_type: remote_artifact.media_type.clone(),
        // Retrieval verifies remote bytes into the controller's existing local content-address
        // contract. A protected remote replica must not silently relabel a cache source object.
        storage_class: sink_storage_class,
        required_replicas: sink_required_replicas,
        chunk_size: rampage_protocol::ARTIFACT_TRANSFER_CHUNK_BYTES,
    };
    let progress = state
        .artifact_store
        .begin_resumable_put(&initial_spec)
        .map_err(internal_error)?;
    let resumed_chunks = progress.received_chunks.len();
    let mut lease_ids = Vec::new();
    for (position, index) in progress.missing_chunks.into_iter().enumerate() {
        if position > 0 {
            lease = state
                .governor
                .authorize_storage_at_epoch(
                    &offer,
                    &remote_artifact.digest,
                    remote_artifact.size_bytes,
                    remote_artifact.storage_class,
                    ArtifactTransferOperation::Get,
                    state.fencing_epoch.load(Ordering::Acquire),
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
        }
        lease_ids.push(lease.lease_id);
        let (artifact, chunk_digest, chunk) = rampage_mesh::artifact_get_chunk(
            &state.mesh.endpoint(),
            rampage_mesh::ArtifactTransferContext {
                destination: endpoint.clone(),
                lease: lease.clone(),
                media_type: remote_artifact.media_type.clone(),
                session_id,
                challenge_nonce: Uuid::new_v4().simple().to_string(),
            },
            index,
        )
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": error.to_string()})),
            )
        })?;
        if artifact.digest != remote_artifact.digest
            || artifact.size_bytes != remote_artifact.size_bytes
            || artifact.media_type != remote_artifact.media_type
            || artifact.storage_class != remote_artifact.storage_class
            || !artifact.encrypted
        {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "remote chunk changed the artifact contract"})),
            ));
        }
        let spec = rampage_storage::ResumablePutSpec {
            lease_id: lease.lease_id.simple().to_string(),
            fencing_epoch: lease.fencing_epoch,
            authority_nonce: lease.nonce.clone(),
            expires_at: lease.expires_at,
            ..initial_spec.clone()
        };
        state
            .artifact_store
            .begin_resumable_put(&spec)
            .map_err(internal_error)?;
        state
            .artifact_store
            .put_resumable_chunk(
                &session_id.simple().to_string(),
                index,
                &chunk_digest,
                &chunk,
            )
            .map_err(internal_error)?;
    }
    let local = state
        .artifact_store
        .commit_resumable_put(&session_id.simple().to_string())
        .map_err(internal_error)?;
    if local.digest != remote_artifact.digest {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": "retrieved artifact failed content-address verification"})),
        ));
    }
    let replica_receipt = probe_replica(&state, &offer, &remote_artifact)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("replica possession proof failed: {error}")})),
            )
        })?;
    state.replica_evidence.write().map_err(lock_error)?.insert(
        (remote_artifact.digest.clone(), request.node_id),
        replica_receipt.clone(),
    );
    let payload = state
        .artifact_store
        .get(&local.digest)
        .map_err(internal_error)?;
    state
        .ledger
        .append(
            "artifact.retrieved",
            &local.digest,
            &json!({
                "node_id": request.node_id,
                "artifact": local,
                "storage_lease_ids": lease_ids,
                "transfer_session_id": session_id,
                "resumed_chunks": resumed_chunks,
                "replica_receipt": replica_receipt
            }),
        )
        .map_err(internal_error)?;
    Ok(Json(json!({
        "schema": "rampage.artifact-payload.v1",
        "artifact": local,
        "node_id": request.node_id,
        "transfer_session_id": session_id,
        "resumed_chunks": resumed_chunks,
        "data_base64": BASE64.encode(payload)
    })))
}

fn retrieval_sink_contract(
    remote: &ArtifactRefV1,
    local: Option<&ArtifactRefV1>,
) -> Result<(StorageClass, u8), String> {
    if let Some(local) = local {
        if local.digest != remote.digest
            || local.size_bytes != remote.size_bytes
            || local.media_type != remote.media_type
            || !local.encrypted
        {
            return Err("local content address conflicts with the remote artifact contract".into());
        }
        return Ok((
            local.storage_class,
            if local.storage_class == StorageClass::Protected {
                2
            } else {
                1
            },
        ));
    }
    Ok((
        remote.storage_class,
        if remote.storage_class == StorageClass::Protected {
            2
        } else {
            1
        },
    ))
}

async fn events(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
) -> Result<Json<Vec<LedgerEvent>>, (StatusCode, Json<Value>)> {
    state
        .ledger
        .events(
            query.after.unwrap_or(0),
            query.limit.unwrap_or(250).clamp(1, 10_000),
        )
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
    use rand::{TryRng as _, rngs::SysRng};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => return read_secret_file(path, "secret key"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut bytes = [0_u8; 32];
    SysRng.try_fill_bytes(&mut bytes)?;
    let encoded = hex::encode(bytes);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
            sync_parent_directory(path)?;
            Ok(bytes)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_secret_file(path, "secret key")
        }
        Err(error) => Err(error.into()),
    }
}

fn read_secret_file(path: &std::path::Path, label: &str) -> anyhow::Result<[u8; 32]> {
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink() && metadata.is_file(),
        "{label} must be a regular non-symlink file"
    );
    anyhow::ensure!(metadata.len() <= 128, "{label} exceeds its size limit");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        if metadata.mode() & 0o077 != 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let bytes = hex::decode(std::fs::read_to_string(path)?.trim())?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{label} must contain exactly 32 bytes"))
}

fn write_new_durable_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    use std::io::Write as _;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_parent_directory(path)
}

#[cfg(unix)]
fn sync_parent_directory(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

fn governor_config_from_env() -> anyhow::Result<GovernorConfig> {
    Ok(GovernorConfig {
        trusted_autopilot_projects: parse_uuid_set_env("RAMPAGE_AUTONOMY_R1_PROJECTS")?,
        autonomous_protected_projects: parse_uuid_set_env("RAMPAGE_AUTONOMY_R2_PROJECTS")?,
        ..GovernorConfig::default()
    })
}

fn parse_uuid_set_env(name: &str) -> anyhow::Result<BTreeSet<Uuid>> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(BTreeSet::new());
    };
    value
        .to_string_lossy()
        .split([';', ','])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            Uuid::parse_str(value).map_err(|error| {
                anyhow::anyhow!("{name} contains invalid project UUID {value}: {error}")
            })
        })
        .collect()
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
    HashMap<(String, Uuid), ArtifactReplicaReceiptV1>,
);

fn restore_state(ledger: &Ledger, fencing_epoch: u64) -> anyhow::Result<RestoredState> {
    let mut nodes = HashMap::new();
    let mut offers = HashMap::new();
    let mut invites = HashMap::new();
    let mut proposed_jobs: HashMap<Uuid, JobSpecV1> = HashMap::new();
    let mut assignments: HashMap<Uuid, Assignment> = HashMap::new();
    let mut completed_receipts = HashMap::new();
    let mut shard_sets = HashMap::new();
    let mut artifact_replicas = HashMap::new();
    let mut replica_evidence = HashMap::new();
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
                        && lease.fencing_epoch == fencing_epoch
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
                            && lease.fencing_epoch == fencing_epoch
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
                "artifact.replicated"
                | "artifact.input.staged"
                | "artifact.output.recorded"
                | "artifact.replica.verified"
                | "artifact.repaired" => {
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
                    let key = (artifact.digest.clone(), node_id);
                    artifact_replicas.insert(key.clone(), artifact);
                    if let Some(receipt) = event
                        .payload
                        .get("replica_receipt")
                        .cloned()
                        .and_then(|value| serde_json::from_value(value).ok())
                        .filter(|receipt: &ArtifactReplicaReceiptV1| {
                            receipt.node_id == node_id
                                && receipt.digest == key.0
                                && receipt.size_bytes
                                    == artifact_replicas
                                        .get(&key)
                                        .map(|artifact| artifact.size_bytes)
                                        .unwrap_or_default()
                                && nodes.get(&node_id).is_some_and(|identity| {
                                    verify_artifact_replica_receipt(identity, receipt).is_ok()
                                })
                        })
                    {
                        replica_evidence.insert(key, receipt);
                    }
                }
                "artifact.replica.invalidated" => {
                    let Some(node_id) = event
                        .payload
                        .get("node_id")
                        .and_then(Value::as_str)
                        .and_then(|value| Uuid::parse_str(value).ok())
                    else {
                        continue;
                    };
                    let Some(digest) = event.payload.get("digest").and_then(Value::as_str) else {
                        continue;
                    };
                    let key = (digest.to_string(), node_id);
                    replica_evidence.remove(&key);
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
        replica_evidence,
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
            match bounded_mesh_controller_response(response).await {
                Ok(bytes) => MeshControlResponseV1 {
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

async fn bounded_mesh_controller_response(
    mut response: reqwest::Response,
) -> anyhow::Result<Vec<u8>> {
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
    anyhow::ensure!(
        response.content_length().unwrap_or(0) <= MAX_RESPONSE_BYTES as u64,
        "controller response exceeded one MiB"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        append_bounded_bytes(&mut bytes, &chunk, MAX_RESPONSE_BYTES)?;
    }
    Ok(bytes)
}

fn append_bounded_bytes(buffer: &mut Vec<u8>, chunk: &[u8], limit: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        buffer.len().saturating_add(chunk.len()) <= limit,
        "controller response exceeded one MiB"
    );
    buffer.extend_from_slice(chunk);
    Ok(())
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
            if offer.mesh_endpoint.is_none() {
                return Err("remote worker offer requires a signed mesh endpoint".into());
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::collections::BTreeMap;

    fn valid_model_offer() -> ResourceOfferV1 {
        let now = chrono::Utc::now();
        let model = InstalledModelV1 {
            schema: InstalledModelV1::SCHEMA.into(),
            model_id: "gemma3:4b".into(),
            artifact_digest: format!("sha256:{}", "a".repeat(64)),
            artifact_size_bytes: 1024 * 1024 * 1024,
        };
        ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id: Uuid::now_v7(),
            observed_at: now,
            expires_at: now + Duration::minutes(1),
            resources: vec![rampage_protocol::ResourceQuantityV1 {
                class: ResourceClass::RamWorkingSet,
                capacity: 4 * 1024 * 1024 * 1024,
                available: 4 * 1024 * 1024 * 1024,
                unit: "byte".into(),
                labels: BTreeMap::new(),
            }],
            availability: rampage_protocol::AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 80,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::from(["rampage.ollama.v1".into()]),
            workload_capabilities: Vec::new(),
            model_runtimes: vec![ModelRuntimeOfferV1 {
                schema: ModelRuntimeOfferV1::SCHEMA.into(),
                adapter: "rampage.ollama.v1".into(),
                backend: ModelBackend::LocalOllama,
                runtime_version: "test".into(),
                runtime_digest: "shipped-local:test".into(),
                compatibility_key: "ollama-test".into(),
                memory_kind: ModelMemoryKind::Host,
                available_model_bytes: 4 * 1024 * 1024 * 1024,
                supported_parallelism: BTreeSet::from([ModelParallelism::WholeModel]),
                status: ModelRuntimeStatus::ShippedLocal,
                installed_models: vec![model],
                certification_digest: None,
            }],
            link_benchmark: None,
            mesh_endpoint: None,
            signature: "signed".into(),
        }
    }

    #[test]
    fn chunked_mesh_proxy_response_is_bounded_during_accumulation() {
        let mut bytes = Vec::new();
        append_bounded_bytes(&mut bytes, &[1, 2], 3).unwrap();
        assert!(append_bounded_bytes(&mut bytes, &[3, 4], 3).is_err());
        assert_eq!(bytes, vec![1, 2]);
    }

    #[test]
    fn model_offer_cannot_exceed_signed_memory_resources() {
        let mut offer = valid_model_offer();
        assert_eq!(validate_model_runtime_contracts(&offer), Ok(()));
        offer.model_runtimes[0].available_model_bytes += 1;
        assert!(validate_model_runtime_contracts(&offer).is_err());
    }

    #[test]
    fn edge_offer_is_bound_to_enrolled_device_class_and_battery_telemetry() {
        let mut offer = valid_model_offer();
        offer.resources[0]
            .labels
            .insert("device_kind".into(), "phone".into());
        offer.availability.battery_percent = Some(75);
        let identity = NodeIdentityV1 {
            schema: NodeIdentityV1::SCHEMA.into(),
            node_id: offer.node_id,
            owner_id: Uuid::now_v7(),
            display_name: "native phone".into(),
            device_kind: DeviceKind::Phone,
            platform: "android".into(),
            public_key: "a".repeat(64),
            enrolled_at: chrono::Utc::now(),
            fencing_epoch: 0,
        };
        assert_eq!(validate_offer_identity_binding(&identity, &offer), Ok(()));

        offer.resources[0]
            .labels
            .insert("device_kind".into(), "desktop".into());
        assert!(validate_offer_identity_binding(&identity, &offer).is_err());

        offer.resources[0]
            .labels
            .insert("device_kind".into(), "phone".into());
        offer.availability.battery_percent = None;
        assert!(validate_offer_identity_binding(&identity, &offer).is_err());
    }

    #[test]
    fn unqualified_runtime_cannot_advertise_executable_models() {
        let mut offer = valid_model_offer();
        offer.model_runtimes[0].status = ModelRuntimeStatus::Candidate;
        assert!(validate_model_runtime_contracts(&offer).is_err());
    }

    #[test]
    fn workload_capability_must_be_valid_unique_and_adapter_bound() {
        let mut offer = valid_model_offer();
        offer.workload_capabilities = vec![rampage_protocol::WorkloadCapabilityV1 {
            schema: rampage_protocol::WorkloadCapabilityV1::SCHEMA.into(),
            adapter: "unadvertised.adapter".into(),
            domain: rampage_protocol::WorkloadDomain::Gaming,
            operations: BTreeSet::from(["stream_session".into()]),
            execution_patterns: BTreeSet::from([
                rampage_protocol::ExecutionPattern::StreamingService,
            ]),
            resource_classes: BTreeSet::from([ResourceClass::GpuCompute]),
            isolation: rampage_protocol::WorkloadIsolation::VendorWorker,
            runtime_digest: "candidate:gaming".into(),
            checkpointable: false,
            preemptible: true,
            network_allowlist_required: true,
            status: rampage_protocol::WorkloadCapabilityStatus::Candidate,
            qualification_digest: None,
        }];
        assert!(validate_model_runtime_contracts(&offer).is_err());
        offer.workload_capabilities[0].adapter = "rampage.ollama.v1".into();
        assert_eq!(validate_model_runtime_contracts(&offer), Ok(()));
        offer
            .workload_capabilities
            .push(offer.workload_capabilities[0].clone());
        assert!(validate_model_runtime_contracts(&offer).is_err());
    }

    #[test]
    fn openai_subset_rejects_tools_instead_of_ignoring_them() {
        let request = serde_json::json!({
            "model": "gemma3:4b",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": []
        });
        assert!(serde_json::from_value::<OpenAiChatCompletionRequest>(request).is_err());
    }

    #[test]
    fn openai_subset_bounds_generation_controls() {
        let request = OpenAiChatCompletionRequest {
            model: "gemma3:4b".into(),
            messages: vec![OpenAiChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            stream: false,
            max_tokens: Some(128),
            max_completion_tokens: Some(128),
            temperature: Some(0.5),
            top_p: Some(0.9),
        };
        assert_eq!(validate_openai_request(&request).unwrap(), 128);
    }

    #[test]
    fn anthropic_text_messages_translate_without_broadening_authority() {
        let request = serde_json::from_value::<AnthropicMessagesRequest>(serde_json::json!({
            "model": "gemma3:4b",
            "max_tokens": 64,
            "system": [{"type": "text", "text": "Be concise."}],
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "hello"}]
            }],
            "stream": true
        }))
        .unwrap();
        let translated = translate_anthropic_request(request).unwrap();
        assert_eq!(translated.messages.len(), 2);
        assert_eq!(translated.messages[0].role, "system");
        assert_eq!(translated.messages[1].content, "hello");
        assert_eq!(translated.max_completion_tokens, Some(64));
        assert!(translated.stream);
    }

    #[test]
    fn anthropic_subset_rejects_tools_instead_of_ignoring_them() {
        let request = serde_json::json!({
            "model": "gemma3:4b",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "hello"}],
            "tools": []
        });
        assert!(serde_json::from_value::<AnthropicMessagesRequest>(request).is_err());
    }

    #[test]
    fn anthropic_stop_reasons_are_explicitly_mapped() {
        assert_eq!(anthropic_stop_reason("stop"), "end_turn");
        assert_eq!(anthropic_stop_reason("length"), "max_tokens");
    }

    #[test]
    fn self_scan_is_stable_and_requires_no_per_change_approval() {
        let now = chrono::Utc::now();
        let report = build_diagnostic_report(
            now,
            &HashMap::new(),
            &HashMap::new(),
            0,
            ArtifactDiagnosticState {
                replicas: &HashMap::new(),
                evidence: &HashMap::new(),
            },
            &[],
            false,
        );
        let repeated = build_diagnostic_report(
            now + Duration::seconds(1),
            &HashMap::new(),
            &HashMap::new(),
            0,
            ArtifactDiagnosticState {
                replicas: &HashMap::new(),
                evidence: &HashMap::new(),
            },
            &[],
            false,
        );
        assert!(!report.autonomy.per_change_approval_required);
        assert_eq!(
            report.autonomy.authority_expansion,
            "automatically_denied_outside_owner_envelope"
        );
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == "NO_ENROLLED_CONTRIBUTORS")
        );
        assert_eq!(report.evidence_digest, repeated.evidence_digest);
        assert!(report.evidence_digest.starts_with("sha256:"));
    }

    #[test]
    fn self_scan_promotes_repeated_denials_to_an_evidenced_finding() {
        let ledger = Ledger::in_memory().unwrap();
        for index in 0..3 {
            ledger
                .append(
                    "job.blocked",
                    &format!("job-{index}"),
                    &json!({"reason": "test"}),
                )
                .unwrap();
        }
        let report = build_diagnostic_report(
            chrono::Utc::now(),
            &HashMap::new(),
            &HashMap::new(),
            0,
            ArtifactDiagnosticState {
                replicas: &HashMap::new(),
                evidence: &HashMap::new(),
            },
            &ledger.latest_events(10).unwrap(),
            false,
        );
        assert_eq!(report.metrics.recent_denials, 3);
        let finding = report
            .findings
            .iter()
            .find(|finding| finding.code == "REPEATED_AUTHORITY_DENIALS")
            .unwrap();
        assert!(finding.proposal.auto_eligible);
        assert_eq!(finding.proposal.risk, "r1_allowlisted_source");
    }

    #[test]
    fn diagnostics_preserve_local_polling_but_exclude_an_empty_mesh_route() {
        let mut offer = valid_model_offer();
        let identity = NodeIdentityV1 {
            schema: "rampage.node-identity.v1".into(),
            node_id: offer.node_id,
            owner_id: Uuid::now_v7(),
            display_name: "unroutable".into(),
            device_kind: rampage_protocol::DeviceKind::Desktop,
            platform: "windows-x86_64".into(),
            public_key: "test-key".into(),
            enrolled_at: chrono::Utc::now(),
            fencing_epoch: 1,
        };
        let report = build_diagnostic_report(
            chrono::Utc::now(),
            &HashMap::from([(identity.node_id, identity.clone())]),
            &HashMap::from([(offer.node_id, offer.clone())]),
            0,
            ArtifactDiagnosticState {
                replicas: &HashMap::new(),
                evidence: &HashMap::new(),
            },
            &[],
            false,
        );
        let constraints =
            derive_autonomous_constraints(&Governor::ephemeral(GovernorConfig::default()), &report);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == "LOOPBACK_POLLING_ONLY")
        );
        assert!(constraints.excluded_nodes.is_empty());

        let now = chrono::Utc::now();
        offer.mesh_endpoint = Some(MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: "test-key".into(),
            direct_addresses: Vec::new(),
            relay_urls: Vec::new(),
            issued_at: now,
            expires_at: offer.expires_at,
            signature: "signed".into(),
        });
        let report = build_diagnostic_report(
            now,
            &HashMap::from([(identity.node_id, identity.clone())]),
            &HashMap::from([(offer.node_id, offer.clone())]),
            0,
            ArtifactDiagnosticState {
                replicas: &HashMap::new(),
                evidence: &HashMap::new(),
            },
            &[],
            false,
        );
        let constraints =
            derive_autonomous_constraints(&Governor::ephemeral(GovernorConfig::default()), &report);
        assert_eq!(
            constraints.excluded_nodes.values().next().unwrap(),
            &["AUTHENTICATED_ROUTE_EMPTY".to_string()]
        );
        assert_eq!(constraints.evidence_digest, report.evidence_digest);
    }

    #[test]
    fn protected_durability_counts_only_fresh_independent_receipts() {
        let now = chrono::Utc::now();
        let digest = format!("sha256:{}", "d".repeat(64));
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let artifact = ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest: digest.clone(),
            size_bytes: 42,
            media_type: "application/octet-stream".into(),
            storage_class: StorageClass::Protected,
            encrypted: true,
        };
        let replicas = HashMap::from([
            ((digest.clone(), first), artifact.clone()),
            ((digest.clone(), second), artifact),
        ]);
        let receipt = |node_id, expires_at| ArtifactReplicaReceiptV1 {
            schema: ArtifactReplicaReceiptV1::SCHEMA.into(),
            receipt_id: Uuid::now_v7(),
            session_id: Uuid::now_v7(),
            lease_id: Uuid::now_v7(),
            node_id,
            digest: digest.clone(),
            size_bytes: 42,
            storage_class: StorageClass::Protected,
            challenge_nonce: Uuid::new_v4().simple().to_string(),
            verified_at: now - Duration::minutes(1),
            expires_at,
            fencing_epoch: 3,
            signature: "signed".into(),
        };
        let evidence = HashMap::from([
            (
                (digest.clone(), first),
                receipt(first, now + Duration::minutes(9)),
            ),
            (
                (digest.clone(), second),
                receipt(second, now - Duration::seconds(1)),
            ),
        ]);
        let report = build_diagnostic_report(
            now,
            &HashMap::new(),
            &HashMap::new(),
            0,
            ArtifactDiagnosticState {
                replicas: &replicas,
                evidence: &evidence,
            },
            &[],
            false,
        );
        assert_eq!(report.metrics.protected_artifacts, 1);
        assert_eq!(report.metrics.under_replicated_protected_artifacts, 1);
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.code == "PROTECTED_ARTIFACT_UNDER_REPLICATED")
        );
    }

    #[test]
    fn transfer_sessions_are_deterministic_but_peer_and_direction_specific() {
        let digest = format!("sha256:{}", "e".repeat(64));
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let put = artifact_transfer_session_id(first, &digest, ArtifactTransferOperation::Put);
        assert_eq!(
            put,
            artifact_transfer_session_id(first, &digest, ArtifactTransferOperation::Put)
        );
        assert_ne!(
            put,
            artifact_transfer_session_id(first, &digest, ArtifactTransferOperation::Get)
        );
        assert_ne!(
            put,
            artifact_transfer_session_id(second, &digest, ArtifactTransferOperation::Put)
        );
    }

    #[test]
    fn protected_remote_retrieval_preserves_the_local_content_address_contract() {
        let digest = format!("sha256:{}", "f".repeat(64));
        let remote = ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest: digest.clone(),
            size_bytes: 42,
            media_type: "application/octet-stream".into(),
            storage_class: StorageClass::Protected,
            encrypted: true,
        };
        let local = ArtifactRefV1 {
            storage_class: StorageClass::Cache,
            ..remote.clone()
        };
        assert_eq!(
            retrieval_sink_contract(&remote, Some(&local)).unwrap(),
            (StorageClass::Cache, 1)
        );
        assert_eq!(
            retrieval_sink_contract(&remote, None).unwrap(),
            (StorageClass::Protected, 2)
        );

        let conflicting = ArtifactRefV1 {
            size_bytes: 43,
            ..local
        };
        assert!(retrieval_sink_contract(&remote, Some(&conflicting)).is_err());
    }

    #[test]
    fn replica_probe_selection_is_rotating_and_resource_bounded() {
        let artifact = |suffix: char, size_bytes| ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest: format!("sha256:{}", suffix.to_string().repeat(64)),
            size_bytes,
            media_type: "application/octet-stream".into(),
            storage_class: StorageClass::Protected,
            encrypted: true,
        };
        let candidates = (0..6)
            .map(|index| {
                let artifact = artifact((b'a' + index as u8) as char, 40 * 1024 * 1024);
                (artifact.digest.clone(), Uuid::now_v7(), artifact)
            })
            .collect::<Vec<_>>();
        let first = select_replica_probes(&candidates, 0);
        let rotated = select_replica_probes(&candidates, 4);
        assert_eq!(first.len(), 3);
        assert_eq!(rotated.len(), 3);
        assert_ne!(first, rotated);
    }

    #[test]
    fn durable_secret_creation_is_idempotent_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("controller.key");
        let first = load_or_create_secret(&path).unwrap();
        let second = load_or_create_secret(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read_to_string(path).unwrap().len(), 64);
    }

    #[cfg(unix)]
    #[test]
    fn secret_loader_rejects_symlinks_and_removes_group_permissions() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.key");
        std::fs::write(&target, "11".repeat(32)).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let linked = temp.path().join("linked.key");
        symlink(&target, &linked).unwrap();
        assert!(load_or_create_secret(&linked).is_err());
        assert!(load_or_create_secret(&target).is_ok());
        assert_eq!(
            std::fs::metadata(target).unwrap().permissions().mode() & 0o077,
            0
        );
    }
}
