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
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
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
            certification_digest: Some(format!("sha256:{}", "b".repeat(64))),
        };
        assert!(runtime.is_qualified_for_distributed());
        runtime.status = ModelRuntimeStatus::Candidate;
        assert!(!runtime.is_qualified_for_distributed());
    }
}
