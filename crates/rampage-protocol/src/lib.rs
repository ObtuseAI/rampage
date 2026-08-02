//! Canonical versioned contracts shared by every Rampage plane.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_VERSION: &str = "rampage.protocol.v1";
pub const MAX_ARTIFACT_TRANSFER_BYTES: u64 = 64 * 1024 * 1024;
pub const LINK_BENCHMARK_TRANSFER_BYTES: u64 = 256 * 1024;
pub const MAX_SHARDS_PER_SET: usize = 256;
pub const MAX_MODEL_SESSION_NODES: u16 = 64;
pub const MAX_MODEL_SESSION_BYTES: u64 = 16 * 1024 * 1024 * 1024 * 1024;
pub const MAX_MODEL_PROMPT_BYTES: u64 = 1024 * 1024;
pub const MAX_MODEL_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_MODEL_OUTPUT_TOKENS: u32 = 32 * 1024;
pub const MAX_RELAY_ENDPOINTS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    CpuCompute,
    GpuCompute,
    NpuCompute,
    GpuMemory,
    RamWorkingSet,
    RamCache,
    StorageCache,
    StorageScratch,
    ProtectedStore,
    NetworkFetch,
    NetworkRelay,
    Toolchain,
    Runtime,
    Codec,
    LicensedService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    Cache,
    Scratch,
    Protected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTransferOperation {
    Put,
    Get,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Desktop,
    Laptop,
    Server,
    SteamDeck,
    Phone,
    Tablet,
    Console,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Full,
    LocalReduced,
    DeterministicOnly,
    ReadOnly,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionRiskV1 {
    R0Configuration,
    R1AllowlistedSource,
    R2ProtectedChange,
    R3AuthorityCritical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionEvidenceGateV1 {
    pub name: String,
    pub passed: bool,
    pub evidence_digest: String,
    pub independent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionCandidateV1 {
    pub schema: String,
    pub proposal_id: Uuid,
    pub project_id: Uuid,
    pub base_revision: String,
    pub candidate_digest: String,
    pub changed_paths: Vec<String>,
    pub risk: PromotionRiskV1,
    pub gates: Vec<PromotionEvidenceGateV1>,
    pub requested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl PromotionCandidateV1 {
    pub const SCHEMA: &'static str = "rampage.promotion-candidate.v1";
    pub const REQUIRED_GATES: [&'static str; 8] = [
        "g0_schema_policy_static",
        "g1_deterministic_replay",
        "g2_quality_reliability_cost",
        "g3_sealed_holdout",
        "g4_adversarial_security",
        "g5_independent_replication",
        "g6_shadow",
        "g7_canary_rollback",
    ];

    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        let names = self
            .gates
            .iter()
            .map(|gate| gate.name.as_str())
            .collect::<BTreeSet<_>>();
        self.schema == Self::SCHEMA
            && !self.base_revision.is_empty()
            && self.base_revision.len() <= 128
            && self.base_revision.is_ascii()
            && is_sha256_digest(&self.candidate_digest)
            && !self.changed_paths.is_empty()
            && self.changed_paths.len() <= 32
            && self.changed_paths.iter().all(|path| {
                !path.is_empty()
                    && path.len() <= 260
                    && path.is_ascii()
                    && !path.starts_with(['/', '\\'])
                    && !path.contains(':')
                    && path
                        .split(['/', '\\'])
                        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
            })
            && self.requested_at <= now
            && now < self.expires_at
            && (self.expires_at - self.requested_at).num_seconds() <= 1_800
            && self.gates.len() == Self::REQUIRED_GATES.len()
            && names.len() == Self::REQUIRED_GATES.len()
            && Self::REQUIRED_GATES
                .iter()
                .all(|required| names.contains(required))
            && self.gates.iter().all(|gate| {
                gate.passed
                    && is_sha256_digest(&gate.evidence_digest)
                    && (gate.name != "g5_independent_replication" || gate.independent)
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionCanaryLeaseV1 {
    pub schema: String,
    pub canary_id: Uuid,
    pub proposal_id: Uuid,
    pub project_id: Uuid,
    pub candidate_digest: String,
    pub risk: PromotionRiskV1,
    pub max_traffic_basis_points: u16,
    pub max_error_regression_basis_points: u16,
    pub max_latency_regression_basis_points: u16,
    pub max_cost_regression_basis_points: u16,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce: String,
    pub fencing_epoch: u64,
    pub signature: String,
}

impl PromotionCanaryLeaseV1 {
    pub const SCHEMA: &'static str = "rampage.promotion-canary-lease.v1";

    pub fn is_active_at(&self, now: DateTime<Utc>, fencing_epoch: u64) -> bool {
        self.schema == Self::SCHEMA
            && self.issued_at <= now
            && now < self.expires_at
            && self.fencing_epoch == fencing_epoch
            && is_sha256_digest(&self.candidate_digest)
            && (1..=1_000).contains(&self.max_traffic_basis_points)
            && self.max_error_regression_basis_points <= 500
            && self.max_latency_regression_basis_points <= 1_000
            && self.max_cost_regression_basis_points <= 1_000
            && !self.nonce.is_empty()
            && self.nonce.len() <= 128
            && self.nonce.is_ascii()
            && !self.signature.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadDomain {
    AiInference,
    AiEvaluation,
    Gaming,
    CreativeProduction,
    SoftwareBuild,
    ScientificComputing,
    DataProcessing,
    Storage,
    EdgeUtility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPattern {
    WholeWorkload,
    IndependentShard,
    Replica,
    StreamingService,
    ApplicationNativeDistributed,
    TensorParallel,
    PipelineParallel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadIsolation {
    AllowlistedInProcess,
    DedicatedProcess,
    Container,
    WasmSandbox,
    ExternalService,
    VendorWorker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadCapabilityStatus {
    Shipped,
    Qualified,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadCapabilityV1 {
    pub schema: String,
    pub adapter: String,
    pub domain: WorkloadDomain,
    pub operations: BTreeSet<String>,
    pub execution_patterns: BTreeSet<ExecutionPattern>,
    pub resource_classes: BTreeSet<ResourceClass>,
    pub isolation: WorkloadIsolation,
    pub runtime_digest: String,
    pub checkpointable: bool,
    pub preemptible: bool,
    pub network_allowlist_required: bool,
    pub status: WorkloadCapabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualification_digest: Option<String>,
}

impl WorkloadCapabilityV1 {
    pub const SCHEMA: &'static str = "rampage.workload-capability.v1";

    pub fn is_valid(&self) -> bool {
        self.schema == Self::SCHEMA
            && !self.adapter.is_empty()
            && self.adapter.len() <= 128
            && self.adapter.is_ascii()
            && !self.operations.is_empty()
            && self.operations.len() <= 32
            && self.operations.iter().all(|operation| {
                !operation.is_empty() && operation.len() <= 64 && operation.is_ascii()
            })
            && !self.execution_patterns.is_empty()
            && !self.resource_classes.is_empty()
            && !self.runtime_digest.is_empty()
            && self.runtime_digest.len() <= 200
            && self.runtime_digest.is_ascii()
            && match self.status {
                WorkloadCapabilityStatus::Qualified => self
                    .qualification_digest
                    .as_deref()
                    .is_some_and(is_sha256_digest),
                WorkloadCapabilityStatus::Shipped | WorkloadCapabilityStatus::Candidate => {
                    self.qualification_digest.is_none()
                }
            }
    }

    pub fn authorizes(&self, adapter: &str, operation: &str) -> bool {
        self.is_valid()
            && self.adapter == adapter
            && self.operations.contains(operation)
            && self.status != WorkloadCapabilityStatus::Candidate
    }
}

/// The owner-selected objective for additional fabric compute.
///
/// These objectives are deliberately distinct: fitting a larger model, reducing interactive
/// latency, and serving more concurrent requests require different placements and may conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeStrategy {
    MaximumModelSize,
    SpeedBoost,
    MaximumThroughput,
    Efficiency,
    AutonomousBalanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    LocalOllama,
    ExoMlx,
    VllmRay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelParallelism {
    WholeModel,
    Pipeline,
    Tensor,
    Replica,
    Speculative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelMemoryKind {
    DedicatedGpu,
    Unified,
    Host,
    /// One machine's local RAM and dedicated VRAM, used by a runtime that can offload layers
    /// between them. This never means memory from different machines is one address space.
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRuntimeStatus {
    /// A local-only adapter shipped and governed by Rampage.
    ShippedLocal,
    /// The runtime was detected, but has no backend/topology qualification evidence.
    Candidate,
    /// The exact runtime and topology campaign are identified by a certification digest.
    Qualified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRuntimeOfferV1 {
    pub schema: String,
    pub adapter: String,
    pub backend: ModelBackend,
    pub runtime_version: String,
    pub runtime_digest: String,
    pub compatibility_key: String,
    pub memory_kind: ModelMemoryKind,
    pub available_model_bytes: u64,
    pub supported_parallelism: BTreeSet<ModelParallelism>,
    pub status: ModelRuntimeStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installed_models: Vec<InstalledModelV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certification_digest: Option<String>,
}

impl ModelRuntimeOfferV1 {
    pub const SCHEMA: &'static str = "rampage.model-runtime-offer.v1";

    pub fn is_qualified_for_distributed(&self) -> bool {
        self.schema == Self::SCHEMA
            && self.status == ModelRuntimeStatus::Qualified
            && self
                .certification_digest
                .as_deref()
                .is_some_and(is_sha256_digest)
            && is_sha256_digest(&self.runtime_digest)
            && !self.compatibility_key.trim().is_empty()
            && self.available_model_bytes > 0
            && (self
                .supported_parallelism
                .contains(&ModelParallelism::Pipeline)
                || self
                    .supported_parallelism
                    .contains(&ModelParallelism::Tensor))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledModelV1 {
    pub schema: String,
    pub model_id: String,
    pub artifact_digest: String,
    pub artifact_size_bytes: u64,
}

impl InstalledModelV1 {
    pub const SCHEMA: &'static str = "rampage.installed-model.v1";

    pub fn is_valid(&self) -> bool {
        self.schema == Self::SCHEMA
            && !self.model_id.trim().is_empty()
            && self.model_id.len() <= 200
            && self.model_id.is_ascii()
            && is_sha256_digest(&self.artifact_digest)
            && self.artifact_size_bytes > 0
            && self.artifact_size_bytes <= MAX_MODEL_SESSION_BYTES
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Proposed,
    Admitted,
    Prepared,
    Running,
    Succeeded,
    Failed,
    Ambiguous,
    Cancelled,
    Fenced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadTrust {
    NativeTrusted,
    GeneratedSandboxed,
    ExternalUntrusted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeIdentityV1 {
    pub schema: String,
    pub node_id: Uuid,
    pub owner_id: Uuid,
    pub display_name: String,
    pub device_kind: DeviceKind,
    pub platform: String,
    pub public_key: String,
    pub enrolled_at: DateTime<Utc>,
    pub fencing_epoch: u64,
}

impl NodeIdentityV1 {
    pub const SCHEMA: &'static str = "rampage.node-identity.v1";

    pub fn is_valid_for_enrollment(&self) -> bool {
        self.schema == Self::SCHEMA
            && self.node_id != Uuid::nil()
            && self.owner_id != Uuid::nil()
            && !self.display_name.trim().is_empty()
            && self.display_name.len() <= 80
            && !self.display_name.chars().any(char::is_control)
            && !self.platform.is_empty()
            && self.platform.len() <= 100
            && self.platform.is_ascii()
            && self.public_key.len() == 64
            && self.public_key.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.fencing_epoch == 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceQuantityV1 {
    pub class: ResourceClass,
    pub capacity: u64,
    pub available: u64,
    pub unit: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvailabilityV1 {
    pub on_ac_power: bool,
    pub battery_percent: Option<u8>,
    pub thermal_headroom_percent: u8,
    pub foreground_allowed: bool,
    pub owner_idle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkBenchmarkV1 {
    pub schema: String,
    pub controller_endpoint_id: String,
    pub observed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub rtt_micros_p50: u64,
    pub uplink_bps: u64,
    pub downlink_bps: u64,
    pub transfer_bytes: u64,
    pub samples: u16,
    pub transport: String,
}

impl LinkBenchmarkV1 {
    pub const SCHEMA: &'static str = "rampage.link-benchmark.v1";

    pub fn is_valid_for(
        &self,
        controller_endpoint_id: &str,
        offer_observed_at: DateTime<Utc>,
        offer_expires_at: DateTime<Utc>,
    ) -> bool {
        self.schema == Self::SCHEMA
            && self.controller_endpoint_id == controller_endpoint_id
            && self.observed_at <= offer_observed_at
            && self.expires_at >= offer_expires_at
            && self.rtt_micros_p50 > 0
            && self.uplink_bps > 0
            && self.downlink_bps > 0
            && self.transfer_bytes == LINK_BENCHMARK_TRANSFER_BYTES
            && self.samples >= 3
            && self.transport == "authenticated_quic"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceOfferV1 {
    pub schema: String,
    pub offer_id: Uuid,
    pub node_id: Uuid,
    pub observed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub resources: Vec<ResourceQuantityV1>,
    pub availability: AvailabilityV1,
    pub adapters: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workload_capabilities: Vec<WorkloadCapabilityV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_runtimes: Vec<ModelRuntimeOfferV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_benchmark: Option<LinkBenchmarkV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_endpoint: Option<MeshEndpointRecordV1>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSessionRequestV1 {
    pub schema: String,
    pub session_id: Uuid,
    pub model_id: String,
    pub estimated_weight_bytes: u64,
    pub kv_cache_bytes: u64,
    pub context_tokens: u32,
    pub strategy: ComputeStrategy,
    pub max_nodes: u16,
    pub deadline: DateTime<Utc>,
    pub idempotency_key: String,
}

impl ModelSessionRequestV1 {
    pub const SCHEMA: &'static str = "rampage.model-session-request.v1";

    pub fn required_bytes(&self) -> u64 {
        self.estimated_weight_bytes
            .saturating_add(self.kv_cache_bytes)
    }

    pub fn validate_at(&self, now: DateTime<Utc>) -> Result<(), ContractError> {
        if self.schema != Self::SCHEMA {
            return Err(ContractError::WrongSchema {
                expected: Self::SCHEMA,
                actual: self.schema.clone(),
            });
        }
        if self.deadline <= now {
            return Err(ContractError::DeadlineExpired);
        }
        if self.idempotency_key.trim().is_empty() {
            return Err(ContractError::EmptyIdempotencyKey);
        }
        if self.model_id.trim().is_empty() {
            return Err(ContractError::EmptyModelId);
        }
        if self.estimated_weight_bytes == 0 || self.required_bytes() > MAX_MODEL_SESSION_BYTES {
            return Err(ContractError::InvalidModelSize);
        }
        if self.context_tokens == 0 {
            return Err(ContractError::InvalidContextTokens);
        }
        if self.max_nodes == 0 || self.max_nodes > MAX_MODEL_SESSION_NODES {
            return Err(ContractError::InvalidModelNodeCount);
        }
        Ok(())
    }
}

/// One-shot authority for an exact installed model on one authenticated worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSessionLeaseV1 {
    pub schema: String,
    pub lease_id: Uuid,
    pub session_id: Uuid,
    pub node_id: Uuid,
    pub controller_endpoint_id: String,
    pub model_id: String,
    pub model_digest: String,
    pub backend: ModelBackend,
    pub runtime_digest: String,
    pub parallelism: ModelParallelism,
    pub max_prompt_bytes: u64,
    pub max_output_tokens: u32,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce: String,
    pub fencing_epoch: u64,
    pub signature: String,
}

impl ModelSessionLeaseV1 {
    pub const SCHEMA: &'static str = "rampage.model-session-lease.v1";

    pub fn is_active_at(&self, now: DateTime<Utc>, current_epoch: u64) -> bool {
        self.schema == Self::SCHEMA
            && self.issued_at <= now
            && now < self.expires_at
            && self.fencing_epoch == current_epoch
            && self.backend == ModelBackend::LocalOllama
            && self.parallelism == ModelParallelism::WholeModel
            && !self.controller_endpoint_id.is_empty()
            && self.controller_endpoint_id.is_ascii()
            && !self.model_id.trim().is_empty()
            && is_sha256_digest(&self.model_digest)
            && !self.runtime_digest.trim().is_empty()
            && !self.nonce.is_empty()
            && self.nonce.len() <= 128
            && self.nonce.is_ascii()
            && self.max_prompt_bytes > 0
            && self.max_prompt_bytes <= MAX_MODEL_PROMPT_BYTES
            && self.max_output_tokens > 0
            && self.max_output_tokens <= MAX_MODEL_OUTPUT_TOKENS
            && !self.signature.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelChatMessageV1 {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInvocationRequestV1 {
    pub schema: String,
    pub request_id: Uuid,
    pub lease: ModelSessionLeaseV1,
    pub messages: Vec<ModelChatMessageV1>,
    pub max_output_tokens: u32,
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

impl ModelInvocationRequestV1 {
    pub const SCHEMA: &'static str = "rampage.model-invocation-request.v1";

    pub fn prompt_bytes(&self) -> u64 {
        self.messages.iter().fold(0_u64, |total, message| {
            total
                .saturating_add(message.role.len() as u64)
                .saturating_add(message.content.len() as u64)
        })
    }

    pub fn is_valid_for(&self, node_id: Uuid, controller_endpoint_id: &str) -> bool {
        self.schema == Self::SCHEMA
            && self.lease.node_id == node_id
            && self.lease.controller_endpoint_id == controller_endpoint_id
            && !self.messages.is_empty()
            && self.messages.len() <= 256
            && self.messages.iter().all(|message| {
                matches!(message.role.as_str(), "system" | "user" | "assistant")
                    && !message.content.is_empty()
            })
            && self.prompt_bytes() <= self.lease.max_prompt_bytes
            && self.max_output_tokens > 0
            && self.max_output_tokens <= self.lease.max_output_tokens
            && self
                .temperature
                .is_none_or(|value| value.is_finite() && (0.0..=2.0).contains(&value))
            && self
                .top_p
                .is_none_or(|value| value.is_finite() && (0.0..=1.0).contains(&value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInvocationFrameKind {
    Delta,
    Complete,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelUsageV1 {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelExecutionReceiptV1 {
    pub schema: String,
    pub receipt_id: Uuid,
    pub lease_id: Uuid,
    pub session_id: Uuid,
    pub request_id: Uuid,
    pub node_id: Uuid,
    pub state: JobState,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub output_digest: String,
    pub output_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelUsageV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub signature: String,
}

impl ModelExecutionReceiptV1 {
    pub const SCHEMA: &'static str = "rampage.model-execution-receipt.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInvocationFrameV1 {
    pub schema: String,
    pub request_id: Uuid,
    pub sequence: u64,
    pub kind: ModelInvocationFrameKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ModelExecutionReceiptV1>,
}

impl ModelInvocationFrameV1 {
    pub const SCHEMA: &'static str = "rampage.model-invocation-frame.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentInviteV1 {
    pub schema: String,
    pub invite_id: Uuid,
    pub enrollment_code: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_mesh: Option<MeshEndpointRecordV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governor_public_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshEndpointRecordV1 {
    pub schema: String,
    pub endpoint_id: String,
    pub direct_addresses: Vec<String>,
    pub relay_urls: Vec<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub signature: String,
}

impl MeshEndpointRecordV1 {
    pub const SCHEMA: &'static str = "rampage.mesh-endpoint.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayAccessManifestV1 {
    pub schema: String,
    pub fabric_id: String,
    pub generation: u64,
    pub allowed_endpoint_ids: BTreeSet<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub signature: String,
}

impl RelayAccessManifestV1 {
    pub const SCHEMA: &'static str = "rampage.relay-access-manifest.v1";

    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        self.schema == Self::SCHEMA
            && is_sha256_digest(&self.fabric_id)
            && self.generation > 0
            && !self.allowed_endpoint_ids.is_empty()
            && self.allowed_endpoint_ids.len() <= MAX_RELAY_ENDPOINTS
            && self.allowed_endpoint_ids.iter().all(|endpoint| {
                endpoint.len() == 64 && endpoint.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            && self.issued_at <= now
            && self.expires_at > now
            && self.expires_at - self.issued_at <= chrono::Duration::minutes(15)
            && !self.signature.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshControlRequestV1 {
    pub schema: String,
    pub request_id: Uuid,
    pub method: String,
    pub path: String,
    pub body: Option<serde_json::Value>,
}

impl MeshControlRequestV1 {
    pub const SCHEMA: &'static str = "rampage.mesh-control-request.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshControlResponseV1 {
    pub schema: String,
    pub request_id: Uuid,
    pub status: u16,
    pub body: serde_json::Value,
}

impl MeshControlResponseV1 {
    pub const SCHEMA: &'static str = "rampage.mesh-control-response.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageLeaseV1 {
    pub schema: String,
    pub lease_id: Uuid,
    pub node_id: Uuid,
    pub digest: String,
    pub operation: ArtifactTransferOperation,
    pub storage_class: StorageClass,
    pub size_bytes: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce: String,
    pub fencing_epoch: u64,
    pub signature: String,
}

impl StorageLeaseV1 {
    pub const SCHEMA: &'static str = "rampage.storage-lease.v1";

    pub fn is_active_at(&self, now: DateTime<Utc>, current_epoch: u64) -> bool {
        self.schema == Self::SCHEMA
            && self.issued_at <= now
            && now < self.expires_at
            && self.fencing_epoch == current_epoch
            && self.size_bytes <= MAX_ARTIFACT_TRANSFER_BYTES
            && !self.signature.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactTransferRequestV1 {
    pub schema: String,
    pub request_id: Uuid,
    pub lease: StorageLeaseV1,
    pub media_type: String,
}

impl ArtifactTransferRequestV1 {
    pub const SCHEMA: &'static str = "rampage.artifact-transfer-request.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactTransferResponseV1 {
    pub schema: String,
    pub request_id: Uuid,
    pub status: u16,
    pub artifact: Option<ArtifactRefV1>,
    pub payload_size: u64,
    pub error: Option<String>,
}

impl ArtifactTransferResponseV1 {
    pub const SCHEMA: &'static str = "rampage.artifact-transfer-response.v1";
}

/// Bounded, restart-safe artifact transfer operations. A PUT session may reuse one exact signed
/// lease for idempotent chunk retries; GET_CHUNK and HEAD consume a fresh signed lease per request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactTransferActionV2 {
    Begin,
    Status,
    PutChunk,
    Commit,
    GetChunk,
    Head,
}

pub const ARTIFACT_TRANSFER_CHUNK_BYTES: u32 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactTransferRequestV2 {
    pub schema: String,
    pub request_id: Uuid,
    pub session_id: Uuid,
    pub lease: StorageLeaseV1,
    pub media_type: String,
    pub action: ArtifactTransferActionV2,
    pub chunk_size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_digest: Option<String>,
    pub payload_size: u64,
    pub challenge_nonce: String,
}

impl ArtifactTransferRequestV2 {
    pub const SCHEMA: &'static str = "rampage.artifact-transfer-request.v2";

    pub fn is_valid(&self) -> bool {
        if self.schema != Self::SCHEMA
            || self.request_id.is_nil()
            || self.session_id.is_nil()
            || self.lease.schema != StorageLeaseV1::SCHEMA
            || self.lease.size_bytes == 0
            || self.lease.size_bytes > MAX_ARTIFACT_TRANSFER_BYTES
            || self.media_type.is_empty()
            || self.media_type.len() > 255
            || !self.media_type.is_ascii()
            || self.chunk_size != ARTIFACT_TRANSFER_CHUNK_BYTES
            || self.challenge_nonce.is_empty()
            || self.challenge_nonce.len() > 128
            || !self.challenge_nonce.is_ascii()
        {
            return false;
        }
        let chunk_count = self.lease.size_bytes.div_ceil(u64::from(self.chunk_size));
        match self.action {
            ArtifactTransferActionV2::Begin
            | ArtifactTransferActionV2::Status
            | ArtifactTransferActionV2::Commit => {
                self.lease.operation == ArtifactTransferOperation::Put
                    && self.chunk_index.is_none()
                    && self.chunk_digest.is_none()
                    && self.payload_size == 0
            }
            ArtifactTransferActionV2::Head => {
                self.lease.operation == ArtifactTransferOperation::Get
                    && self.chunk_index.is_none()
                    && self.chunk_digest.is_none()
                    && self.payload_size == 0
            }
            ArtifactTransferActionV2::PutChunk => {
                self.lease.operation == ArtifactTransferOperation::Put
                    && self
                        .chunk_index
                        .is_some_and(|index| u64::from(index) < chunk_count)
                    && self.chunk_digest.as_deref().is_some_and(is_sha256_digest)
                    && self.payload_size
                        == expected_artifact_chunk_size(
                            self.lease.size_bytes,
                            self.chunk_size,
                            self.chunk_index.unwrap_or_default(),
                        )
                        .unwrap_or_default()
            }
            ArtifactTransferActionV2::GetChunk => {
                self.lease.operation == ArtifactTransferOperation::Get
                    && self
                        .chunk_index
                        .is_some_and(|index| u64::from(index) < chunk_count)
                    && self.chunk_digest.is_none()
                    && self.payload_size == 0
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactTransferProgressV1 {
    pub schema: String,
    pub session_id: Uuid,
    pub digest: String,
    pub size_bytes: u64,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub received_chunks: Vec<u32>,
    pub missing_chunks: Vec<u32>,
    pub complete: bool,
}

impl ArtifactTransferProgressV1 {
    pub const SCHEMA: &'static str = "rampage.artifact-transfer-progress.v1";

    pub fn is_valid(&self) -> bool {
        self.schema == Self::SCHEMA
            && !self.session_id.is_nil()
            && is_sha256_digest(&self.digest)
            && self.size_bytes > 0
            && self.size_bytes <= MAX_ARTIFACT_TRANSFER_BYTES
            && self.chunk_size == ARTIFACT_TRANSFER_CHUNK_BYTES
            && self.chunk_count
                == u32::try_from(self.size_bytes.div_ceil(u64::from(self.chunk_size)))
                    .unwrap_or_default()
            && self
                .received_chunks
                .iter()
                .chain(&self.missing_chunks)
                .all(|index| *index < self.chunk_count)
    }
}

/// Node-signed proof that an exact content-addressed artifact was authenticated at a specific
/// fencing epoch in response to the controller's fresh challenge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReplicaReceiptV1 {
    pub schema: String,
    pub receipt_id: Uuid,
    pub session_id: Uuid,
    pub lease_id: Uuid,
    pub node_id: Uuid,
    pub digest: String,
    pub size_bytes: u64,
    pub storage_class: StorageClass,
    pub challenge_nonce: String,
    pub verified_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub fencing_epoch: u64,
    pub signature: String,
}

impl ArtifactReplicaReceiptV1 {
    pub const SCHEMA: &'static str = "rampage.artifact-replica-receipt.v1";

    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        self.schema == Self::SCHEMA
            && !self.receipt_id.is_nil()
            && !self.session_id.is_nil()
            && !self.lease_id.is_nil()
            && !self.node_id.is_nil()
            && is_sha256_digest(&self.digest)
            && self.size_bytes > 0
            && self.size_bytes <= MAX_ARTIFACT_TRANSFER_BYTES
            && !self.challenge_nonce.is_empty()
            && self.challenge_nonce.len() <= 128
            && self.challenge_nonce.is_ascii()
            && self.verified_at <= now
            && now < self.expires_at
            && self.expires_at - self.verified_at <= chrono::Duration::minutes(15)
            && !self.signature.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactTransferResponseV2 {
    pub schema: String,
    pub request_id: Uuid,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactRefV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ArtifactTransferProgressV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_digest: Option<String>,
    pub payload_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replica_receipt: Option<ArtifactReplicaReceiptV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ArtifactTransferResponseV2 {
    pub const SCHEMA: &'static str = "rampage.artifact-transfer-response.v2";
}

pub fn expected_artifact_chunk_size(
    size_bytes: u64,
    chunk_size: u32,
    chunk_index: u32,
) -> Option<u64> {
    if size_bytes == 0 || chunk_size == 0 {
        return None;
    }
    let offset = u64::from(chunk_index).checked_mul(u64::from(chunk_size))?;
    if offset >= size_bytes {
        return None;
    }
    Some((size_bytes - offset).min(u64::from(chunk_size)))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentRequestV1 {
    pub schema: String,
    pub invite_id: Uuid,
    pub enrollment_code: String,
    pub identity: NodeIdentityV1,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRequestV1 {
    pub class: ResourceClass,
    pub minimum: u64,
    pub preferred: u64,
    pub unit: String,
    pub required_labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRefV1 {
    pub schema: String,
    pub digest: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub storage_class: StorageClass,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSpecV1 {
    pub schema: String,
    pub job_id: Uuid,
    pub project_id: Uuid,
    pub submitted_by: Uuid,
    pub adapter: String,
    pub operation: String,
    pub arguments: BTreeMap<String, String>,
    pub inputs: Vec<ArtifactRefV1>,
    pub requests: Vec<ResourceRequestV1>,
    pub trust: WorkloadTrust,
    pub restart_tolerant: bool,
    pub network_allowlist: BTreeSet<String>,
    pub deadline: DateTime<Utc>,
    pub idempotency_key: String,
}

/// A bounded collection of independent jobs that may execute concurrently across the fabric.
///
/// V1 intentionally does not describe tensor or address-space sharding. Every member is a normal,
/// independently leased Rampage job with its own receipt and must be safe to retry elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShardSetV1 {
    pub schema: String,
    pub set_id: Uuid,
    pub project_id: Uuid,
    pub submitted_by: Uuid,
    pub shards: Vec<JobSpecV1>,
    pub minimum_successes: u32,
    pub deadline: DateTime<Utc>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityLeaseV1 {
    pub schema: String,
    pub lease_id: Uuid,
    pub job_id: Uuid,
    pub node_id: Uuid,
    pub project_id: Uuid,
    pub adapter: String,
    pub operation: String,
    pub input_digests: BTreeSet<String>,
    pub granted: Vec<ResourceQuantityV1>,
    pub network_allowlist: BTreeSet<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce: String,
    pub fencing_epoch: u64,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReceiptV1 {
    pub schema: String,
    pub receipt_id: Uuid,
    pub lease_id: Uuid,
    pub job_id: Uuid,
    pub node_id: Uuid,
    pub state: JobState,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub outputs: Vec<ArtifactRefV1>,
    pub metrics: BTreeMap<String, f64>,
    pub stdout_digest: Option<String>,
    pub stderr_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkClaimV1 {
    pub schema: String,
    pub job: JobSpecV1,
    pub lease: CapabilityLeaseV1,
    pub governor_public_key: String,
}

impl WorkClaimV1 {
    pub const SCHEMA: &'static str = "rampage.work-claim.v1";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBundleV1 {
    pub schema: String,
    pub evidence_id: Uuid,
    pub subject_digest: String,
    pub preregistration_digest: String,
    pub receipts: Vec<ExecutionReceiptV1>,
    pub holdout_digest: Option<String>,
    pub replication_node_ids: BTreeSet<Uuid>,
    pub gates_passed: BTreeSet<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContractError {
    #[error("unexpected schema {actual}; expected {expected}")]
    WrongSchema {
        expected: &'static str,
        actual: String,
    },
    #[error("deadline has expired")]
    DeadlineExpired,
    #[error("resource request minimum exceeds preferred")]
    InvalidResourceRange,
    #[error("idempotency key is empty")]
    EmptyIdempotencyKey,
    #[error("shard set must contain between 1 and {MAX_SHARDS_PER_SET} jobs")]
    InvalidShardCount,
    #[error("minimum successes must be between 1 and the shard count")]
    InvalidSuccessThreshold,
    #[error("shard identifiers and idempotency keys must be unique")]
    DuplicateShardIdentity,
    #[error("every shard must belong to the shard set project and submitter")]
    ShardOwnershipMismatch,
    #[error("every shard must be restart tolerant and expire no later than its shard set")]
    UnsafeShardLifecycle,
    #[error("model identifier is empty")]
    EmptyModelId,
    #[error("model session size is zero or exceeds the protocol limit")]
    InvalidModelSize,
    #[error("model context token count must be positive")]
    InvalidContextTokens,
    #[error("model session node count must be between 1 and {MAX_MODEL_SESSION_NODES}")]
    InvalidModelNodeCount,
}

fn is_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl JobSpecV1 {
    pub const SCHEMA: &'static str = "rampage.job-spec.v1";

    pub fn validate_at(&self, now: DateTime<Utc>) -> Result<(), ContractError> {
        if self.schema != Self::SCHEMA {
            return Err(ContractError::WrongSchema {
                expected: Self::SCHEMA,
                actual: self.schema.clone(),
            });
        }
        if self.deadline <= now {
            return Err(ContractError::DeadlineExpired);
        }
        if self.idempotency_key.trim().is_empty() {
            return Err(ContractError::EmptyIdempotencyKey);
        }
        if self.requests.iter().any(|r| r.minimum > r.preferred) {
            return Err(ContractError::InvalidResourceRange);
        }
        Ok(())
    }
}

impl ShardSetV1 {
    pub const SCHEMA: &'static str = "rampage.shard-set.v1";

    pub fn validate_at(&self, now: DateTime<Utc>) -> Result<(), ContractError> {
        if self.schema != Self::SCHEMA {
            return Err(ContractError::WrongSchema {
                expected: Self::SCHEMA,
                actual: self.schema.clone(),
            });
        }
        if self.deadline <= now {
            return Err(ContractError::DeadlineExpired);
        }
        if self.idempotency_key.trim().is_empty() {
            return Err(ContractError::EmptyIdempotencyKey);
        }
        if self.shards.is_empty() || self.shards.len() > MAX_SHARDS_PER_SET {
            return Err(ContractError::InvalidShardCount);
        }
        if self.minimum_successes == 0 || self.minimum_successes as usize > self.shards.len() {
            return Err(ContractError::InvalidSuccessThreshold);
        }

        let mut job_ids = BTreeSet::new();
        let mut idempotency_keys = BTreeSet::new();
        for shard in &self.shards {
            shard.validate_at(now)?;
            if shard.project_id != self.project_id || shard.submitted_by != self.submitted_by {
                return Err(ContractError::ShardOwnershipMismatch);
            }
            if !shard.restart_tolerant || shard.deadline > self.deadline {
                return Err(ContractError::UnsafeShardLifecycle);
            }
            if !job_ids.insert(shard.job_id)
                || !idempotency_keys.insert(shard.idempotency_key.as_str())
            {
                return Err(ContractError::DuplicateShardIdentity);
            }
        }
        Ok(())
    }
}

impl CapabilityLeaseV1 {
    pub const SCHEMA: &'static str = "rampage.capability-lease.v1";

    pub fn is_active_at(&self, now: DateTime<Utc>, current_epoch: u64) -> bool {
        self.schema == Self::SCHEMA
            && self.issued_at <= now
            && now < self.expires_at
            && self.fencing_epoch == current_epoch
            && !self.signature.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_contracts_require_canonical_lowercase_hex() {
        assert!(is_sha256_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(!is_sha256_digest(&format!("sha256:{}", "A".repeat(64))));
    }
    use chrono::Duration;

    #[test]
    fn rejects_expired_job() {
        let now = Utc::now();
        let job = JobSpecV1 {
            schema: JobSpecV1::SCHEMA.into(),
            job_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            submitted_by: Uuid::now_v7(),
            adapter: "rampage.echo.v1".into(),
            operation: "echo".into(),
            arguments: BTreeMap::new(),
            inputs: vec![],
            requests: vec![],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline: now - Duration::seconds(1),
            idempotency_key: "one".into(),
        };
        assert_eq!(job.validate_at(now), Err(ContractError::DeadlineExpired));
    }

    #[test]
    fn lease_is_fenced_by_epoch() {
        let now = Utc::now();
        let lease = CapabilityLeaseV1 {
            schema: CapabilityLeaseV1::SCHEMA.into(),
            lease_id: Uuid::now_v7(),
            job_id: Uuid::now_v7(),
            node_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            adapter: "rampage.echo.v1".into(),
            operation: "echo".into(),
            input_digests: BTreeSet::new(),
            granted: vec![],
            network_allowlist: BTreeSet::new(),
            issued_at: now,
            expires_at: now + Duration::minutes(1),
            nonce: "nonce".into(),
            fencing_epoch: 4,
            signature: "signed".into(),
        };
        assert!(lease.is_active_at(now, 4));
        assert!(!lease.is_active_at(now, 5));
    }

    #[test]
    fn link_benchmark_is_scoped_fresh_and_bounded() {
        let now = Utc::now();
        let mut benchmark = LinkBenchmarkV1 {
            schema: LinkBenchmarkV1::SCHEMA.into(),
            controller_endpoint_id: "controller-a".into(),
            observed_at: now,
            expires_at: now + Duration::minutes(2),
            rtt_micros_p50: 2_000,
            uplink_bps: 100_000_000,
            downlink_bps: 200_000_000,
            transfer_bytes: LINK_BENCHMARK_TRANSFER_BYTES,
            samples: 3,
            transport: "authenticated_quic".into(),
        };
        assert!(benchmark.is_valid_for(
            "controller-a",
            now + Duration::seconds(1),
            now + Duration::minutes(1)
        ));
        benchmark.controller_endpoint_id = "controller-b".into();
        assert!(!benchmark.is_valid_for(
            "controller-a",
            now + Duration::seconds(1),
            now + Duration::minutes(1)
        ));
    }

    #[test]
    fn shard_set_requires_independent_restart_tolerant_jobs() {
        let now = Utc::now();
        let project_id = Uuid::now_v7();
        let submitted_by = Uuid::now_v7();
        let shard = JobSpecV1 {
            schema: JobSpecV1::SCHEMA.into(),
            job_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            adapter: "rampage.eval-shard.v1".into(),
            operation: "score".into(),
            arguments: BTreeMap::from([("values".into(), "1,2,3".into())]),
            inputs: vec![],
            requests: vec![],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline: now + Duration::minutes(5),
            idempotency_key: "shard-1".into(),
        };
        let mut set = ShardSetV1 {
            schema: ShardSetV1::SCHEMA.into(),
            set_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            shards: vec![shard],
            minimum_successes: 1,
            deadline: now + Duration::minutes(5),
            idempotency_key: "set-1".into(),
        };
        assert_eq!(set.validate_at(now), Ok(()));
        set.shards[0].restart_tolerant = false;
        assert_eq!(
            set.validate_at(now),
            Err(ContractError::UnsafeShardLifecycle)
        );
    }

    #[test]
    fn shard_set_rejects_duplicate_job_identity() {
        let now = Utc::now();
        let project_id = Uuid::now_v7();
        let submitted_by = Uuid::now_v7();
        let shard = JobSpecV1 {
            schema: JobSpecV1::SCHEMA.into(),
            job_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            adapter: "rampage.hash.v1".into(),
            operation: "hash".into(),
            arguments: BTreeMap::new(),
            inputs: vec![],
            requests: vec![],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline: now + Duration::minutes(5),
            idempotency_key: "duplicate".into(),
        };
        let set = ShardSetV1 {
            schema: ShardSetV1::SCHEMA.into(),
            set_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            shards: vec![shard.clone(), shard],
            minimum_successes: 1,
            deadline: now + Duration::minutes(5),
            idempotency_key: "set-1".into(),
        };
        assert_eq!(
            set.validate_at(now),
            Err(ContractError::DuplicateShardIdentity)
        );
    }

    #[test]
    fn model_session_keeps_strategy_and_capacity_bounded() {
        let now = Utc::now();
        let mut request = ModelSessionRequestV1 {
            schema: ModelSessionRequestV1::SCHEMA.into(),
            session_id: Uuid::now_v7(),
            model_id: "local/large-model".into(),
            estimated_weight_bytes: 40 * 1024 * 1024 * 1024,
            kv_cache_bytes: 4 * 1024 * 1024 * 1024,
            context_tokens: 32_768,
            strategy: ComputeStrategy::MaximumModelSize,
            max_nodes: 8,
            deadline: now + Duration::minutes(10),
            idempotency_key: "model-session-1".into(),
        };
        assert_eq!(request.validate_at(now), Ok(()));
        assert_eq!(request.required_bytes(), 44 * 1024 * 1024 * 1024);
        request.max_nodes = MAX_MODEL_SESSION_NODES + 1;
        assert_eq!(
            request.validate_at(now),
            Err(ContractError::InvalidModelNodeCount)
        );
    }

    #[test]
    fn distributed_runtime_requires_exact_runtime_and_campaign_digests() {
        let mut runtime = ModelRuntimeOfferV1 {
            schema: ModelRuntimeOfferV1::SCHEMA.into(),
            adapter: "rampage.exo-mlx.v1".into(),
            backend: ModelBackend::ExoMlx,
            runtime_version: "pinned".into(),
            runtime_digest: format!("sha256:{}", "a".repeat(64)),
            compatibility_key: "mlx-arm64-v1".into(),
            memory_kind: ModelMemoryKind::Unified,
            available_model_bytes: 64 * 1024 * 1024 * 1024,
            supported_parallelism: BTreeSet::from([
                ModelParallelism::Pipeline,
                ModelParallelism::Tensor,
            ]),
            status: ModelRuntimeStatus::Qualified,
            installed_models: vec![],
            certification_digest: Some(format!("sha256:{}", "b".repeat(64))),
        };
        assert!(runtime.is_qualified_for_distributed());
        runtime.status = ModelRuntimeStatus::Candidate;
        assert!(!runtime.is_qualified_for_distributed());
    }

    #[test]
    fn model_invocation_is_bounded_and_scoped_to_one_worker() {
        let now = Utc::now();
        let node_id = Uuid::now_v7();
        let lease = ModelSessionLeaseV1 {
            schema: ModelSessionLeaseV1::SCHEMA.into(),
            lease_id: Uuid::now_v7(),
            session_id: Uuid::now_v7(),
            node_id,
            controller_endpoint_id: "controller-endpoint".into(),
            model_id: "gemma3:4b".into(),
            model_digest: format!("sha256:{}", "a".repeat(64)),
            backend: ModelBackend::LocalOllama,
            runtime_digest: "shipped-local:1.0".into(),
            parallelism: ModelParallelism::WholeModel,
            max_prompt_bytes: 1024,
            max_output_tokens: 512,
            issued_at: now,
            expires_at: now + Duration::minutes(1),
            nonce: "one-shot".into(),
            fencing_epoch: 7,
            signature: "signed".into(),
        };
        assert!(lease.is_active_at(now, 7));
        let mut request = ModelInvocationRequestV1 {
            schema: ModelInvocationRequestV1::SCHEMA.into(),
            request_id: Uuid::now_v7(),
            lease,
            messages: vec![ModelChatMessageV1 {
                role: "user".into(),
                content: "hello".into(),
            }],
            max_output_tokens: 128,
            stream: true,
            temperature: Some(0.5),
            top_p: Some(0.9),
        };
        assert!(request.is_valid_for(node_id, "controller-endpoint"));
        assert!(!request.is_valid_for(Uuid::now_v7(), "controller-endpoint"));
        request.max_output_tokens = 513;
        assert!(!request.is_valid_for(node_id, "controller-endpoint"));
    }

    #[test]
    fn workload_capability_authority_is_operation_exact_and_candidate_safe() {
        let mut capability = WorkloadCapabilityV1 {
            schema: WorkloadCapabilityV1::SCHEMA.into(),
            adapter: "rampage.render.v1".into(),
            domain: WorkloadDomain::CreativeProduction,
            operations: BTreeSet::from(["render_frame".into()]),
            execution_patterns: BTreeSet::from([ExecutionPattern::IndependentShard]),
            resource_classes: BTreeSet::from([
                ResourceClass::CpuCompute,
                ResourceClass::GpuCompute,
            ]),
            isolation: WorkloadIsolation::DedicatedProcess,
            runtime_digest: "shipped-renderer:test".into(),
            checkpointable: true,
            preemptible: true,
            network_allowlist_required: false,
            status: WorkloadCapabilityStatus::Shipped,
            qualification_digest: None,
        };
        assert!(capability.authorizes("rampage.render.v1", "render_frame"));
        assert!(!capability.authorizes("rampage.render.v1", "encode_video"));
        capability.status = WorkloadCapabilityStatus::Candidate;
        assert!(!capability.authorizes("rampage.render.v1", "render_frame"));
    }

    fn promotion_candidate(now: DateTime<Utc>) -> PromotionCandidateV1 {
        PromotionCandidateV1 {
            schema: PromotionCandidateV1::SCHEMA.into(),
            proposal_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            base_revision: "abc123".into(),
            candidate_digest: format!("sha256:{}", "a".repeat(64)),
            changed_paths: vec!["routing/cache.toml".into()],
            risk: PromotionRiskV1::R0Configuration,
            gates: PromotionCandidateV1::REQUIRED_GATES
                .iter()
                .enumerate()
                .map(|(index, name)| PromotionEvidenceGateV1 {
                    name: (*name).into(),
                    passed: true,
                    evidence_digest: format!("sha256:{index:064x}"),
                    independent: *name == "g5_independent_replication",
                })
                .collect(),
            requested_at: now,
            expires_at: now + chrono::Duration::minutes(10),
        }
    }

    #[test]
    fn promotion_candidate_requires_every_independent_content_addressed_gate() {
        let now = Utc::now();
        let mut candidate = promotion_candidate(now);
        assert!(candidate.is_valid_at(now));
        candidate.gates[5].independent = false;
        assert!(!candidate.is_valid_at(now));
        candidate.gates[5].independent = true;
        candidate.gates.push(candidate.gates[0].clone());
        assert!(!candidate.is_valid_at(now));
        candidate.gates.pop();
        candidate.changed_paths = vec!["../policy.rs".into()];
        assert!(!candidate.is_valid_at(now));
        candidate.changed_paths = vec!["C:\\policy.rs".into()];
        assert!(!candidate.is_valid_at(now));
    }

    #[test]
    fn relay_access_manifest_is_short_lived_bounded_and_endpoint_exact() {
        let now = Utc::now();
        let mut manifest = RelayAccessManifestV1 {
            schema: RelayAccessManifestV1::SCHEMA.into(),
            fabric_id: format!("sha256:{}", "a".repeat(64)),
            generation: 1,
            allowed_endpoint_ids: BTreeSet::from(["b".repeat(64)]),
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(10),
            signature: "signed".into(),
        };
        assert!(manifest.is_valid_at(now));
        manifest
            .allowed_endpoint_ids
            .insert("not-an-endpoint".into());
        assert!(!manifest.is_valid_at(now));
        manifest.allowed_endpoint_ids.remove("not-an-endpoint");
        manifest.expires_at = now + chrono::Duration::minutes(16);
        assert!(!manifest.is_valid_at(now));
    }

    #[test]
    fn artifact_v2_frames_are_chunk_bounded_and_operation_exact() {
        let now = Utc::now();
        let lease = StorageLeaseV1 {
            schema: StorageLeaseV1::SCHEMA.into(),
            lease_id: Uuid::now_v7(),
            node_id: Uuid::now_v7(),
            digest: format!("sha256:{}", "a".repeat(64)),
            operation: ArtifactTransferOperation::Put,
            storage_class: StorageClass::Protected,
            size_bytes: u64::from(ARTIFACT_TRANSFER_CHUNK_BYTES) + 17,
            issued_at: now,
            expires_at: now + chrono::Duration::minutes(2),
            nonce: "one-shot".into(),
            fencing_epoch: 9,
            signature: "signed".into(),
        };
        let mut request = ArtifactTransferRequestV2 {
            schema: ArtifactTransferRequestV2::SCHEMA.into(),
            request_id: Uuid::now_v7(),
            session_id: Uuid::now_v7(),
            lease,
            media_type: "application/octet-stream".into(),
            action: ArtifactTransferActionV2::PutChunk,
            chunk_size: ARTIFACT_TRANSFER_CHUNK_BYTES,
            chunk_index: Some(1),
            chunk_digest: Some(format!("sha256:{}", "b".repeat(64))),
            payload_size: 17,
            challenge_nonce: "challenge".into(),
        };
        assert!(request.is_valid());
        request.payload_size = 18;
        assert!(!request.is_valid());
        request.payload_size = 17;
        request.action = ArtifactTransferActionV2::GetChunk;
        request.chunk_digest = None;
        assert!(!request.is_valid());
        request.lease.operation = ArtifactTransferOperation::Get;
        request.payload_size = 0;
        assert!(request.is_valid());
    }
}
