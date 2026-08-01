//! Non-agentic policy Governor. Missing or ambiguous facts deny authority.

use chrono::{Duration, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rampage_protocol::{
    ArtifactTransferOperation, CapabilityLeaseV1, DeviceKind, EnrollmentRequestV1,
    ExecutionReceiptV1, InstalledModelV1, JobSpecV1, MAX_ARTIFACT_TRANSFER_BYTES,
    MAX_MODEL_OUTPUT_TOKENS, MAX_MODEL_PROMPT_BYTES, MeshEndpointRecordV1, ModelBackend,
    ModelExecutionReceiptV1, ModelParallelism, ModelRuntimeOfferV1, ModelRuntimeStatus,
    ModelSessionLeaseV1, NodeIdentityV1, PromotionCanaryLeaseV1, PromotionCandidateV1,
    PromotionRiskV1, ResourceClass, ResourceOfferV1, StorageClass, StorageLeaseV1,
};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    R0Configuration,
    R1AllowlistedSource,
    R2ProtectedChange,
    R3AuthorityCritical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernorConfig {
    pub lease_ttl_seconds: i64,
    pub max_job_ttl_seconds: i64,
    pub trusted_adapters: BTreeSet<String>,
    pub mobile_adapters: BTreeSet<String>,
    pub trusted_autopilot_projects: BTreeSet<Uuid>,
    pub autonomous_protected_projects: BTreeSet<Uuid>,
    pub mobile_min_battery_percent: u8,
    pub mobile_min_thermal_headroom_percent: u8,
    pub killed: bool,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            lease_ttl_seconds: 120,
            max_job_ttl_seconds: 3_600,
            trusted_adapters: BTreeSet::from([
                "rampage.echo.v1".to_string(),
                "rampage.hash.v1".to_string(),
                "rampage.eval-shard.v1".to_string(),
                "rampage.ollama.v1".to_string(),
                "rampage.artifact-hash.v1".to_string(),
            ]),
            mobile_adapters: BTreeSet::from([
                "rampage.hash.v1".to_string(),
                "rampage.eval-shard.v1".to_string(),
                "rampage.artifact-hash.v1".to_string(),
            ]),
            trusted_autopilot_projects: BTreeSet::new(),
            autonomous_protected_projects: BTreeSet::new(),
            mobile_min_battery_percent: 40,
            mobile_min_thermal_headroom_percent: 35,
            killed: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSessionLimits {
    pub max_prompt_bytes: u64,
    pub max_output_tokens: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Denial {
    #[error("owner kill latch is active")]
    KillLatch,
    #[error("adapter is not allowlisted")]
    AdapterDenied,
    #[error("selected offer does not advertise the requested adapter")]
    OfferAdapterMismatch,
    #[error("selected offer does not advertise an executable capability for this operation")]
    WorkloadCapabilityMismatch,
    #[error("selected offer does not satisfy the requested resource contract")]
    OfferResourceMismatch,
    #[error("adapter is not permitted to request this resource class")]
    AdapterResourceDenied,
    #[error("offer has expired")]
    OfferExpired,
    #[error("job contract is invalid: {0}")]
    InvalidJob(String),
    #[error("offer and selected node differ")]
    NodeMismatch,
    #[error("mobile work must be restart tolerant")]
    MobileRequiresRestartTolerance,
    #[error("adapter is not eligible for mobile execution")]
    MobileAdapterDenied,
    #[error("mobile work requires an explicit foreground donation session")]
    MobileForegroundRequired,
    #[error("mobile battery policy denies this work")]
    MobileBatteryDenied,
    #[error("mobile thermal headroom is below the owner safety threshold")]
    MobileThermalDenied,
    #[error("job deadline exceeds the maximum authority window")]
    DeadlineTooFar,
    #[error("risk class is outside the owner-defined autonomous authority envelope")]
    AutonomousAuthorityDenied,
    #[error("autopilot is not enabled for this project")]
    AutopilotNotEnabled,
    #[error("diagnostic action is outside the deterministic operational envelope")]
    DiagnosticActionDenied,
    #[error("promotion evidence is incomplete, stale, ambiguous, or malformed")]
    InvalidPromotionEvidence,
    #[error("declared promotion risk does not match deterministic path classification")]
    PromotionRiskMismatch,
    #[error("lease signature is invalid")]
    InvalidSignature,
    #[error("artifact digest or size is invalid")]
    InvalidArtifact,
    #[error("node did not offer enough storage in the requested class")]
    StorageCapacityDenied,
    #[error("model session does not target an exact shipped local runtime and installed model")]
    ModelRuntimeDenied,
    #[error("model session limits are invalid")]
    InvalidModelSession,
}

pub struct Governor {
    config: GovernorConfig,
    signing_key: SigningKey,
}

impl Governor {
    pub fn ephemeral(config: GovernorConfig) -> Self {
        Self {
            config,
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    pub fn from_signing_key(config: GovernorConfig, signing_key: SigningKey) -> Self {
        Self {
            config,
            signing_key,
        }
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn authorize_job(
        &self,
        job: &JobSpecV1,
        offer: &ResourceOfferV1,
        selected_node: Uuid,
    ) -> Result<CapabilityLeaseV1, Denial> {
        self.authorize_job_at_epoch(job, offer, selected_node, 0)
    }

    pub fn authorize_job_at_epoch(
        &self,
        job: &JobSpecV1,
        offer: &ResourceOfferV1,
        selected_node: Uuid,
        fencing_epoch: u64,
    ) -> Result<CapabilityLeaseV1, Denial> {
        let now = Utc::now();
        self.check_job_at(job, offer, selected_node, now)?;
        let expires_at = std::cmp::min(
            job.deadline,
            now + Duration::seconds(self.config.lease_ttl_seconds.max(1)),
        );
        let mut lease = CapabilityLeaseV1 {
            schema: CapabilityLeaseV1::SCHEMA.into(),
            lease_id: Uuid::now_v7(),
            job_id: job.job_id,
            node_id: selected_node,
            project_id: job.project_id,
            adapter: job.adapter.clone(),
            operation: job.operation.clone(),
            input_digests: job
                .inputs
                .iter()
                .map(|artifact| artifact.digest.clone())
                .collect(),
            granted: job
                .requests
                .iter()
                .map(|request| {
                    let offered = offer
                        .resources
                        .iter()
                        .find(|resource| resource.class == request.class)
                        .expect("authority check proved the requested resource exists");
                    rampage_protocol::ResourceQuantityV1 {
                        class: request.class,
                        capacity: request.minimum,
                        available: request.minimum,
                        unit: request.unit.clone(),
                        labels: offered.labels.clone(),
                    }
                })
                .collect(),
            network_allowlist: job.network_allowlist.clone(),
            issued_at: now,
            expires_at,
            nonce: Uuid::new_v4().simple().to_string(),
            fencing_epoch,
            signature: String::new(),
        };
        lease.signature = self.sign_lease(&lease);
        Ok(lease)
    }

    /// Evaluate the complete deterministic authority policy without minting a capability lease.
    pub fn check_job(
        &self,
        job: &JobSpecV1,
        offer: &ResourceOfferV1,
        selected_node: Uuid,
    ) -> Result<(), Denial> {
        self.check_job_at(job, offer, selected_node, Utc::now())
    }

    fn check_job_at(
        &self,
        job: &JobSpecV1,
        offer: &ResourceOfferV1,
        selected_node: Uuid,
        now: chrono::DateTime<Utc>,
    ) -> Result<(), Denial> {
        if self.config.killed {
            return Err(Denial::KillLatch);
        }
        job.validate_at(now)
            .map_err(|error| Denial::InvalidJob(error.to_string()))?;
        if !self.config.trusted_adapters.contains(&job.adapter) {
            return Err(Denial::AdapterDenied);
        }
        if job
            .requests
            .iter()
            .any(|request| !adapter_allows_resource(&job.adapter, request.class))
        {
            return Err(Denial::AdapterResourceDenied);
        }
        if !offer.adapters.contains(&job.adapter) {
            return Err(Denial::OfferAdapterMismatch);
        }
        if !offer.workload_capabilities.is_empty()
            && !offer
                .workload_capabilities
                .iter()
                .any(|capability| capability.authorizes(&job.adapter, &job.operation))
        {
            return Err(Denial::WorkloadCapabilityMismatch);
        }
        if job.requests.iter().any(|request| {
            offer.resources.iter().all(|resource| {
                resource.class != request.class
                    || resource.unit != request.unit
                    || resource.available < request.minimum
                    || request
                        .required_labels
                        .iter()
                        .any(|(key, value)| resource.labels.get(key) != Some(value))
            })
        }) {
            return Err(Denial::OfferResourceMismatch);
        }
        if offer.expires_at <= now {
            return Err(Denial::OfferExpired);
        }
        if offer.node_id != selected_node {
            return Err(Denial::NodeMismatch);
        }
        if job.deadline > now + Duration::seconds(self.config.max_job_ttl_seconds) {
            return Err(Denial::DeadlineTooFar);
        }
        let device_kind = offer
            .resources
            .first()
            .and_then(|resource| resource.labels.get("device_kind"))
            .map(String::as_str);
        if matches!(device_kind, Some("phone" | "tablet" | "console")) {
            if !job.restart_tolerant {
                return Err(Denial::MobileRequiresRestartTolerance);
            }
            if !self.config.mobile_adapters.contains(&job.adapter) {
                return Err(Denial::MobileAdapterDenied);
            }
            if !offer.availability.foreground_allowed {
                return Err(Denial::MobileForegroundRequired);
            }
            if !offer.availability.on_ac_power
                && offer.availability.battery_percent < Some(self.config.mobile_min_battery_percent)
            {
                return Err(Denial::MobileBatteryDenied);
            }
            if offer.availability.thermal_headroom_percent
                < self.config.mobile_min_thermal_headroom_percent
            {
                return Err(Denial::MobileThermalDenied);
            }
        }
        Ok(())
    }

    pub fn authorize_promotion(&self, project_id: Uuid, risk: RiskClass) -> Result<(), Denial> {
        match risk {
            RiskClass::R0Configuration => Ok(()),
            RiskClass::R1AllowlistedSource
                if self.config.trusted_autopilot_projects.contains(&project_id) =>
            {
                Ok(())
            }
            RiskClass::R1AllowlistedSource => Err(Denial::AutopilotNotEnabled),
            RiskClass::R2ProtectedChange
                if self
                    .config
                    .autonomous_protected_projects
                    .contains(&project_id) =>
            {
                Ok(())
            }
            RiskClass::R2ProtectedChange | RiskClass::R3AuthorityCritical => {
                Err(Denial::AutonomousAuthorityDenied)
            }
        }
    }

    /// Authorize only predeclared, reversible scheduling constraints derived from direct evidence.
    ///
    /// This is the no-per-change-approval operational lane. It can reduce authority by suppressing
    /// unsafe placement, but it cannot add adapters, resources, peers, routes, or filesystem/network
    /// access. Code and policy promotion uses the separate evidence-gated promotion path.
    pub fn authorize_diagnostic_action(&self, action: &str) -> Result<(), Denial> {
        if self.config.killed {
            return Err(Denial::KillLatch);
        }
        match action {
            "suppress_thermally_constrained_node"
            | "suppress_low_battery_node"
            | "suppress_unroutable_node" => Ok(()),
            _ => Err(Denial::DiagnosticActionDenied),
        }
    }

    pub fn authorize_promotion_canary_at_epoch(
        &self,
        candidate: &PromotionCandidateV1,
        fencing_epoch: u64,
    ) -> Result<PromotionCanaryLeaseV1, Denial> {
        let now = Utc::now();
        if !candidate.is_valid_at(now) {
            return Err(Denial::InvalidPromotionEvidence);
        }
        let classified = classify_promotion_paths(&candidate.changed_paths);
        let declared = promotion_risk(candidate.risk);
        if declared != classified {
            return Err(Denial::PromotionRiskMismatch);
        }
        self.authorize_promotion(candidate.project_id, declared)?;
        let max_traffic_basis_points = match candidate.risk {
            PromotionRiskV1::R0Configuration => 1_000,
            PromotionRiskV1::R1AllowlistedSource => 500,
            PromotionRiskV1::R2ProtectedChange => 100,
            PromotionRiskV1::R3AuthorityCritical => {
                return Err(Denial::AutonomousAuthorityDenied);
            }
        };
        let mut lease = PromotionCanaryLeaseV1 {
            schema: PromotionCanaryLeaseV1::SCHEMA.into(),
            canary_id: Uuid::now_v7(),
            proposal_id: candidate.proposal_id,
            project_id: candidate.project_id,
            candidate_digest: candidate.candidate_digest.clone(),
            risk: candidate.risk,
            max_traffic_basis_points,
            max_error_regression_basis_points: 100,
            max_latency_regression_basis_points: 500,
            max_cost_regression_basis_points: 500,
            issued_at: now,
            expires_at: std::cmp::min(candidate.expires_at, now + Duration::minutes(10)),
            nonce: Uuid::new_v4().simple().to_string(),
            fencing_epoch,
            signature: String::new(),
        };
        lease.signature = hex::encode(
            self.signing_key
                .sign(&promotion_canary_message(&lease))
                .to_bytes(),
        );
        Ok(lease)
    }

    pub fn authorize_storage(
        &self,
        offer: &ResourceOfferV1,
        digest: &str,
        size_bytes: u64,
        storage_class: StorageClass,
        operation: ArtifactTransferOperation,
    ) -> Result<StorageLeaseV1, Denial> {
        self.authorize_storage_at_epoch(offer, digest, size_bytes, storage_class, operation, 0)
    }

    pub fn authorize_model_session_at_epoch(
        &self,
        offer: &ResourceOfferV1,
        runtime: &ModelRuntimeOfferV1,
        model: &InstalledModelV1,
        controller_endpoint_id: &str,
        limits: ModelSessionLimits,
        fencing_epoch: u64,
    ) -> Result<ModelSessionLeaseV1, Denial> {
        let now = Utc::now();
        if self.config.killed {
            return Err(Denial::KillLatch);
        }
        if offer.expires_at <= now {
            return Err(Denial::OfferExpired);
        }
        if limits.max_prompt_bytes == 0
            || limits.max_prompt_bytes > MAX_MODEL_PROMPT_BYTES
            || limits.max_output_tokens == 0
            || limits.max_output_tokens > MAX_MODEL_OUTPUT_TOKENS
        {
            return Err(Denial::InvalidModelSession);
        }
        let exact_runtime = offer.model_runtimes.iter().any(|candidate| {
            candidate.schema == ModelRuntimeOfferV1::SCHEMA
                && candidate.adapter == runtime.adapter
                && candidate.backend == runtime.backend
                && candidate.runtime_digest == runtime.runtime_digest
                && candidate.compatibility_key == runtime.compatibility_key
                && candidate.status == runtime.status
                && candidate
                    .installed_models
                    .iter()
                    .any(|candidate| candidate == model)
        });
        let guarded_model_bytes = model
            .artifact_size_bytes
            .saturating_add(model.artifact_size_bytes / 5);
        if controller_endpoint_id.is_empty()
            || !controller_endpoint_id.is_ascii()
            || offer.mesh_endpoint.is_none()
            || !offer.adapters.contains("rampage.ollama.v1")
            || runtime.adapter != "rampage.ollama.v1"
            || runtime.backend != ModelBackend::LocalOllama
            || runtime.status != ModelRuntimeStatus::ShippedLocal
            || !runtime
                .supported_parallelism
                .contains(&ModelParallelism::WholeModel)
            || !model.is_valid()
            || runtime.available_model_bytes < guarded_model_bytes
            || !exact_runtime
            || !offer.availability.foreground_allowed
            || offer.availability.thermal_headroom_percent < 15
            || (!offer.availability.on_ac_power
                && offer.availability.battery_percent.unwrap_or(100) < 50)
        {
            return Err(Denial::ModelRuntimeDenied);
        }
        let mut lease = ModelSessionLeaseV1 {
            schema: ModelSessionLeaseV1::SCHEMA.into(),
            lease_id: Uuid::now_v7(),
            session_id: Uuid::now_v7(),
            node_id: offer.node_id,
            controller_endpoint_id: controller_endpoint_id.into(),
            model_id: model.model_id.clone(),
            model_digest: model.artifact_digest.clone(),
            backend: runtime.backend,
            runtime_digest: runtime.runtime_digest.clone(),
            parallelism: ModelParallelism::WholeModel,
            max_prompt_bytes: limits.max_prompt_bytes,
            max_output_tokens: limits.max_output_tokens,
            issued_at: now,
            expires_at: now + Duration::seconds(self.config.lease_ttl_seconds.max(1)),
            nonce: Uuid::new_v4().simple().to_string(),
            fencing_epoch,
            signature: String::new(),
        };
        lease.signature = hex::encode(
            self.signing_key
                .sign(&model_session_lease_message(&lease))
                .to_bytes(),
        );
        Ok(lease)
    }

    pub fn authorize_storage_at_epoch(
        &self,
        offer: &ResourceOfferV1,
        digest: &str,
        size_bytes: u64,
        storage_class: StorageClass,
        operation: ArtifactTransferOperation,
        fencing_epoch: u64,
    ) -> Result<StorageLeaseV1, Denial> {
        let now = Utc::now();
        if self.config.killed {
            return Err(Denial::KillLatch);
        }
        if offer.expires_at <= now {
            return Err(Denial::OfferExpired);
        }
        let valid_digest = digest.strip_prefix("sha256:").is_some_and(|value| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if !valid_digest || size_bytes > MAX_ARTIFACT_TRANSFER_BYTES {
            return Err(Denial::InvalidArtifact);
        }
        if operation == ArtifactTransferOperation::Put {
            let required_class = match storage_class {
                StorageClass::Cache => ResourceClass::StorageCache,
                StorageClass::Scratch => ResourceClass::StorageScratch,
                StorageClass::Protected => ResourceClass::ProtectedStore,
            };
            if !offer.resources.iter().any(|resource| {
                resource.class == required_class && resource.available >= size_bytes
            }) {
                return Err(Denial::StorageCapacityDenied);
            }
        }
        let mut lease = StorageLeaseV1 {
            schema: StorageLeaseV1::SCHEMA.into(),
            lease_id: Uuid::now_v7(),
            node_id: offer.node_id,
            digest: digest.into(),
            operation,
            storage_class,
            size_bytes,
            issued_at: now,
            expires_at: now + Duration::seconds(self.config.lease_ttl_seconds.max(1)),
            nonce: Uuid::new_v4().simple().to_string(),
            fencing_epoch,
            signature: String::new(),
        };
        lease.signature = hex::encode(
            self.signing_key
                .sign(&storage_lease_message(&lease))
                .to_bytes(),
        );
        Ok(lease)
    }

    pub fn verify_lease(&self, lease: &CapabilityLeaseV1) -> Result<(), Denial> {
        let signature_bytes =
            hex::decode(&lease.signature).map_err(|_| Denial::InvalidSignature)?;
        let signature =
            Signature::from_slice(&signature_bytes).map_err(|_| Denial::InvalidSignature)?;
        self.verifying_key()
            .verify(&lease_message(lease), &signature)
            .map_err(|_| Denial::InvalidSignature)
    }

    pub fn sign_mesh_endpoint(&self, endpoint: &mut MeshEndpointRecordV1) {
        endpoint.signature.clear();
        endpoint.signature = hex::encode(
            self.signing_key
                .sign(&contract_message(endpoint))
                .to_bytes(),
        );
    }

    fn sign_lease(&self, lease: &CapabilityLeaseV1) -> String {
        hex::encode(self.signing_key.sign(&lease_message(lease)).to_bytes())
    }
}

fn adapter_allows_resource(adapter: &str, class: rampage_protocol::ResourceClass) -> bool {
    use rampage_protocol::ResourceClass;
    match adapter {
        "rampage.echo.v1" | "rampage.hash.v1" | "rampage.eval-shard.v1" => matches!(
            class,
            ResourceClass::CpuCompute | ResourceClass::RamWorkingSet | ResourceClass::RamCache
        ),
        "rampage.ollama.v1" => matches!(
            class,
            ResourceClass::CpuCompute
                | ResourceClass::GpuCompute
                | ResourceClass::GpuMemory
                | ResourceClass::RamWorkingSet
                | ResourceClass::RamCache
                | ResourceClass::StorageCache
        ),
        "rampage.artifact-hash.v1" => matches!(
            class,
            ResourceClass::CpuCompute
                | ResourceClass::RamWorkingSet
                | ResourceClass::RamCache
                | ResourceClass::StorageCache
        ),
        _ => false,
    }
}

fn promotion_risk(risk: PromotionRiskV1) -> RiskClass {
    match risk {
        PromotionRiskV1::R0Configuration => RiskClass::R0Configuration,
        PromotionRiskV1::R1AllowlistedSource => RiskClass::R1AllowlistedSource,
        PromotionRiskV1::R2ProtectedChange => RiskClass::R2ProtectedChange,
        PromotionRiskV1::R3AuthorityCritical => RiskClass::R3AuthorityCritical,
    }
}

fn classify_promotion_paths(paths: &[String]) -> RiskClass {
    let normalized = paths
        .iter()
        .map(|path| path.replace('\\', "/").to_ascii_lowercase())
        .collect::<Vec<_>>();
    let authority_critical = [
        "crates/rampage-policy/",
        "crates/rampage-protocol/",
        "crates/rampage-controller/",
        "crates/rampage-agent/",
        "crates/rampage-mesh/",
        "crates/rampage-storage/",
        "policies/",
        "evals/holdouts/",
        "signing/",
        "updater/",
        "services/intelligence/src/rampage_intelligence/promotion.py",
        ".github/workflows/",
    ];
    if normalized.iter().any(|path| {
        authority_critical
            .iter()
            .any(|prefix| path.starts_with(prefix))
    }) {
        return RiskClass::R3AuthorityCritical;
    }
    if normalized.iter().any(|path| {
        path.ends_with("pyproject.toml")
            || path.ends_with("Cargo.toml")
            || path.ends_with("package.json")
            || path.contains("/migrations/")
            || path.starts_with("contracts/")
    }) {
        return RiskClass::R2ProtectedChange;
    }
    if normalized.iter().all(|path| {
        path.starts_with("prompts/") || path.starts_with("routing/") || path.starts_with("cache/")
    }) {
        return RiskClass::R0Configuration;
    }
    RiskClass::R1AllowlistedSource
}

fn lease_message(lease: &CapabilityLeaseV1) -> Vec<u8> {
    let mut unsigned = lease.clone();
    unsigned.signature.clear();
    let bytes = serde_json::to_vec(&unsigned).expect("lease contract is serializable");
    Sha256::digest(bytes).to_vec()
}

pub fn sign_offer(signing_key: &SigningKey, offer: &mut ResourceOfferV1) {
    offer.signature.clear();
    offer.signature = hex::encode(signing_key.sign(&contract_message(offer)).to_bytes());
}

pub fn verify_offer(identity: &NodeIdentityV1, offer: &ResourceOfferV1) -> Result<(), Denial> {
    if identity.node_id != offer.node_id {
        return Err(Denial::NodeMismatch);
    }
    verify_contract_signature(&identity.public_key, &offer.signature, &{
        let mut unsigned = offer.clone();
        unsigned.signature.clear();
        contract_message(&unsigned)
    })
}

pub fn verify_lease_with_key(public_key: &str, lease: &CapabilityLeaseV1) -> Result<(), Denial> {
    verify_contract_signature(public_key, &lease.signature, &lease_message(lease))
}

pub fn verify_storage_lease_with_key(
    public_key: &str,
    lease: &StorageLeaseV1,
) -> Result<(), Denial> {
    verify_contract_signature(public_key, &lease.signature, &storage_lease_message(lease))
}

pub fn verify_model_session_lease_with_key(
    public_key: &str,
    lease: &ModelSessionLeaseV1,
) -> Result<(), Denial> {
    verify_contract_signature(
        public_key,
        &lease.signature,
        &model_session_lease_message(lease),
    )
}

fn model_session_lease_message(lease: &ModelSessionLeaseV1) -> Vec<u8> {
    let mut unsigned = lease.clone();
    unsigned.signature.clear();
    contract_message(&unsigned)
}

fn promotion_canary_message(lease: &PromotionCanaryLeaseV1) -> Vec<u8> {
    let mut unsigned = lease.clone();
    unsigned.signature.clear();
    contract_message(&unsigned)
}

pub fn verify_promotion_canary_with_key(
    public_key: &str,
    lease: &PromotionCanaryLeaseV1,
) -> Result<(), Denial> {
    verify_contract_signature(
        public_key,
        &lease.signature,
        &promotion_canary_message(lease),
    )
}

fn storage_lease_message(lease: &StorageLeaseV1) -> Vec<u8> {
    let mut unsigned = lease.clone();
    unsigned.signature.clear();
    contract_message(&unsigned)
}

pub fn sign_mesh_endpoint(signing_key: &SigningKey, endpoint: &mut MeshEndpointRecordV1) {
    endpoint.signature.clear();
    endpoint.signature = hex::encode(signing_key.sign(&contract_message(endpoint)).to_bytes());
}

pub fn verify_mesh_endpoint_with_key(
    public_key: &str,
    endpoint: &MeshEndpointRecordV1,
) -> Result<(), Denial> {
    if endpoint.schema != MeshEndpointRecordV1::SCHEMA
        || endpoint.expires_at <= Utc::now()
        || endpoint.issued_at > Utc::now()
    {
        return Err(Denial::InvalidSignature);
    }
    verify_contract_signature(public_key, &endpoint.signature, &{
        let mut unsigned = endpoint.clone();
        unsigned.signature.clear();
        contract_message(&unsigned)
    })
}

pub fn sign_receipt(signing_key: &SigningKey, receipt: &mut ExecutionReceiptV1) {
    receipt.signature.clear();
    receipt.signature = hex::encode(signing_key.sign(&contract_message(receipt)).to_bytes());
}

pub fn verify_receipt(
    identity: &NodeIdentityV1,
    receipt: &ExecutionReceiptV1,
) -> Result<(), Denial> {
    if identity.node_id != receipt.node_id {
        return Err(Denial::NodeMismatch);
    }
    verify_contract_signature(&identity.public_key, &receipt.signature, &{
        let mut unsigned = receipt.clone();
        unsigned.signature.clear();
        contract_message(&unsigned)
    })
}

pub fn sign_model_receipt(signing_key: &SigningKey, receipt: &mut ModelExecutionReceiptV1) {
    receipt.signature.clear();
    receipt.signature = hex::encode(signing_key.sign(&contract_message(receipt)).to_bytes());
}

pub fn verify_model_receipt(
    identity: &NodeIdentityV1,
    receipt: &ModelExecutionReceiptV1,
) -> Result<(), Denial> {
    if identity.node_id != receipt.node_id {
        return Err(Denial::NodeMismatch);
    }
    verify_contract_signature(&identity.public_key, &receipt.signature, &{
        let mut unsigned = receipt.clone();
        unsigned.signature.clear();
        contract_message(&unsigned)
    })
}

pub fn sign_enrollment(signing_key: &SigningKey, request: &mut EnrollmentRequestV1) {
    request.signature.clear();
    request.signature = hex::encode(signing_key.sign(&contract_message(request)).to_bytes());
}

pub fn verify_enrollment(request: &EnrollmentRequestV1) -> Result<(), Denial> {
    verify_contract_signature(&request.identity.public_key, &request.signature, &{
        let mut unsigned = request.clone();
        unsigned.signature.clear();
        contract_message(&unsigned)
    })
}

fn contract_message<T: Serialize>(contract: &T) -> Vec<u8> {
    let bytes = serde_json::to_vec(contract).expect("protocol contract is serializable");
    Sha256::digest(bytes).to_vec()
}

fn verify_contract_signature(
    public_key: &str,
    signature: &str,
    message: &[u8],
) -> Result<(), Denial> {
    let public_key_bytes = hex::decode(public_key).map_err(|_| Denial::InvalidSignature)?;
    let public_key_array: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| Denial::InvalidSignature)?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_key_array).map_err(|_| Denial::InvalidSignature)?;
    let signature_bytes = hex::decode(signature).map_err(|_| Denial::InvalidSignature)?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| Denial::InvalidSignature)?;
    verifying_key
        .verify(message, &signature)
        .map_err(|_| Denial::InvalidSignature)
}

pub fn device_kind_label(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Desktop => "desktop",
        DeviceKind::Laptop => "laptop",
        DeviceKind::Server => "server",
        DeviceKind::SteamDeck => "steam_deck",
        DeviceKind::Phone => "phone",
        DeviceKind::Tablet => "tablet",
        DeviceKind::Console => "console",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rampage_protocol::{
        AvailabilityV1, ExecutionPattern, ModelMemoryKind, PromotionEvidenceGateV1, ResourceClass,
        ResourceQuantityV1, ResourceRequestV1, WorkloadCapabilityStatus, WorkloadCapabilityV1,
        WorkloadDomain, WorkloadIsolation, WorkloadTrust,
    };
    use std::collections::BTreeMap;

    fn fixtures(kind: &str, restart_tolerant: bool) -> (JobSpecV1, ResourceOfferV1) {
        let now = Utc::now();
        let node_id = Uuid::now_v7();
        let project_id = Uuid::now_v7();
        let mut labels = BTreeMap::new();
        labels.insert("device_kind".into(), kind.into());
        let resource = ResourceQuantityV1 {
            class: ResourceClass::CpuCompute,
            capacity: 8,
            available: 6,
            unit: "logical_core".into(),
            labels,
        };
        (
            JobSpecV1 {
                schema: JobSpecV1::SCHEMA.into(),
                job_id: Uuid::now_v7(),
                project_id,
                submitted_by: Uuid::now_v7(),
                adapter: "rampage.hash.v1".into(),
                operation: "hash".into(),
                arguments: BTreeMap::new(),
                inputs: vec![],
                requests: vec![ResourceRequestV1 {
                    class: ResourceClass::CpuCompute,
                    minimum: 1,
                    preferred: 2,
                    unit: "logical_core".into(),
                    required_labels: BTreeMap::new(),
                }],
                trust: WorkloadTrust::NativeTrusted,
                restart_tolerant,
                network_allowlist: BTreeSet::new(),
                deadline: now + Duration::minutes(5),
                idempotency_key: "test".into(),
            },
            ResourceOfferV1 {
                schema: "rampage.resource-offer.v1".into(),
                offer_id: Uuid::now_v7(),
                node_id,
                observed_at: now,
                expires_at: now + Duration::minutes(1),
                resources: vec![resource],
                availability: AvailabilityV1 {
                    on_ac_power: true,
                    battery_percent: None,
                    thermal_headroom_percent: 80,
                    foreground_allowed: true,
                    owner_idle: true,
                },
                adapters: BTreeSet::from(["rampage.hash.v1".into()]),
                workload_capabilities: Vec::new(),
                model_runtimes: Vec::new(),
                link_benchmark: None,
                mesh_endpoint: None,
                signature: "agent-signed".into(),
            },
        )
    }

    #[test]
    fn issues_verifiable_scoped_lease() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (job, offer) = fixtures("desktop", true);
        let lease = governor.authorize_job(&job, &offer, offer.node_id).unwrap();
        assert_eq!(lease.adapter, job.adapter);
        assert_eq!(lease.granted.len(), 1);
        assert_eq!(lease.granted[0].capacity, 1);
        assert_eq!(lease.granted[0].available, 1);
        assert!(governor.verify_lease(&lease).is_ok());
    }

    #[test]
    fn issued_authority_carries_the_signed_fencing_epoch() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (job, mut offer) = fixtures("desktop", true);
        offer.resources.push(ResourceQuantityV1 {
            class: ResourceClass::StorageCache,
            capacity: 4096,
            available: 4096,
            unit: "byte".into(),
            labels: BTreeMap::new(),
        });
        let lease = governor
            .authorize_job_at_epoch(&job, &offer, offer.node_id, 23)
            .unwrap();
        assert_eq!(lease.fencing_epoch, 23);
        assert!(governor.verify_lease(&lease).is_ok());

        let storage_lease = governor
            .authorize_storage_at_epoch(
                &offer,
                &format!("sha256:{}", "b".repeat(64)),
                1024,
                StorageClass::Cache,
                ArtifactTransferOperation::Put,
                23,
            )
            .unwrap();
        assert_eq!(storage_lease.fencing_epoch, 23);
        assert!(
            verify_storage_lease_with_key(
                &hex::encode(governor.verifying_key().to_bytes()),
                &storage_lease
            )
            .is_ok()
        );
    }

    #[test]
    fn model_session_authority_is_exact_signed_and_tamper_evident() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (_, mut offer) = fixtures("desktop", true);
        offer.adapters = BTreeSet::from(["rampage.ollama.v1".into()]);
        offer.resources.push(ResourceQuantityV1 {
            class: ResourceClass::RamWorkingSet,
            capacity: 4 * 1024 * 1024 * 1024,
            available: 4 * 1024 * 1024 * 1024,
            unit: "byte".into(),
            labels: BTreeMap::new(),
        });
        offer.mesh_endpoint = Some(MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: "worker".into(),
            direct_addresses: vec!["127.0.0.1:1".into()],
            relay_urls: vec![],
            issued_at: Utc::now(),
            expires_at: offer.expires_at,
            signature: "signed".into(),
        });
        let model = InstalledModelV1 {
            schema: InstalledModelV1::SCHEMA.into(),
            model_id: "gemma3:4b".into(),
            artifact_digest: format!("sha256:{}", "d".repeat(64)),
            artifact_size_bytes: 1024 * 1024 * 1024,
        };
        let runtime = ModelRuntimeOfferV1 {
            schema: ModelRuntimeOfferV1::SCHEMA.into(),
            adapter: "rampage.ollama.v1".into(),
            backend: ModelBackend::LocalOllama,
            runtime_version: "test".into(),
            runtime_digest: "shipped-local:test".into(),
            compatibility_key: "ollama-test".into(),
            memory_kind: ModelMemoryKind::Host,
            available_model_bytes: 2 * 1024 * 1024 * 1024,
            supported_parallelism: BTreeSet::from([ModelParallelism::WholeModel]),
            status: ModelRuntimeStatus::ShippedLocal,
            installed_models: vec![model.clone()],
            certification_digest: None,
        };
        offer.model_runtimes = vec![runtime.clone()];
        let lease = governor
            .authorize_model_session_at_epoch(
                &offer,
                &runtime,
                &model,
                "controller",
                ModelSessionLimits {
                    max_prompt_bytes: 4096,
                    max_output_tokens: 512,
                },
                9,
            )
            .unwrap();
        let governor_key = hex::encode(governor.verifying_key().to_bytes());
        assert!(lease.is_active_at(Utc::now(), 9));
        assert!(verify_model_session_lease_with_key(&governor_key, &lease).is_ok());
        let mut tampered = lease;
        tampered.model_id = "other".into();
        assert_eq!(
            verify_model_session_lease_with_key(&governor_key, &tampered),
            Err(Denial::InvalidSignature)
        );
    }

    #[test]
    fn governor_rechecks_offer_adapter_and_resource_contract() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (job, mut offer) = fixtures("desktop", true);
        offer.adapters.clear();
        assert_eq!(
            governor.check_job(&job, &offer, offer.node_id),
            Err(Denial::OfferAdapterMismatch)
        );
        offer.adapters.insert(job.adapter.clone());
        offer.resources[0].available = 0;
        assert_eq!(
            governor.check_job(&job, &offer, offer.node_id),
            Err(Denial::OfferResourceMismatch)
        );
    }

    #[test]
    fn governor_denies_candidate_or_wrong_operation_capabilities() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (job, mut offer) = fixtures("desktop", true);
        offer.workload_capabilities = vec![WorkloadCapabilityV1 {
            schema: WorkloadCapabilityV1::SCHEMA.into(),
            adapter: job.adapter.clone(),
            domain: WorkloadDomain::DataProcessing,
            operations: BTreeSet::from([job.operation.clone()]),
            execution_patterns: BTreeSet::from([ExecutionPattern::WholeWorkload]),
            resource_classes: BTreeSet::from([ResourceClass::CpuCompute]),
            isolation: WorkloadIsolation::AllowlistedInProcess,
            runtime_digest: "shipped-agent:test".into(),
            checkpointable: false,
            preemptible: true,
            network_allowlist_required: false,
            status: WorkloadCapabilityStatus::Candidate,
            qualification_digest: None,
        }];
        assert_eq!(
            governor.check_job(&job, &offer, offer.node_id),
            Err(Denial::WorkloadCapabilityMismatch)
        );
        offer.workload_capabilities[0].status = WorkloadCapabilityStatus::Shipped;
        assert_eq!(governor.check_job(&job, &offer, offer.node_id), Ok(()));
        offer.workload_capabilities[0].operations = BTreeSet::from(["other".into()]);
        assert_eq!(
            governor.check_job(&job, &offer, offer.node_id),
            Err(Denial::WorkloadCapabilityMismatch)
        );
    }

    #[test]
    fn rejects_non_restartable_mobile_work() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (job, offer) = fixtures("phone", false);
        assert_eq!(
            governor
                .authorize_job(&job, &offer, offer.node_id)
                .unwrap_err(),
            Denial::MobileRequiresRestartTolerance
        );
    }

    #[test]
    fn rejects_mobile_work_that_would_drain_a_low_battery() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (job, mut offer) = fixtures("phone", true);
        offer.availability.on_ac_power = false;
        offer.availability.battery_percent = Some(20);
        assert!(matches!(
            governor.authorize_job(&job, &offer, offer.node_id),
            Err(Denial::MobileBatteryDenied)
        ));
    }

    #[test]
    fn authority_critical_promotion_is_automatically_denied() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        assert_eq!(
            governor.authorize_promotion(Uuid::now_v7(), RiskClass::R3AuthorityCritical),
            Err(Denial::AutonomousAuthorityDenied)
        );
    }

    #[test]
    fn protected_promotion_requires_a_preconfigured_project_envelope() {
        let project_id = Uuid::now_v7();
        let mut config = GovernorConfig::default();
        config.autonomous_protected_projects.insert(project_id);
        let governor = Governor::ephemeral(config);
        assert_eq!(
            governor.authorize_promotion(project_id, RiskClass::R2ProtectedChange),
            Ok(())
        );
        assert_eq!(
            governor.authorize_promotion(Uuid::now_v7(), RiskClass::R2ProtectedChange),
            Err(Denial::AutonomousAuthorityDenied)
        );
    }

    #[test]
    fn diagnostic_autonomy_can_only_reduce_scheduling_authority() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        assert_eq!(
            governor.authorize_diagnostic_action("suppress_thermally_constrained_node"),
            Ok(())
        );
        assert_eq!(
            governor.authorize_diagnostic_action("enroll_peer"),
            Err(Denial::DiagnosticActionDenied)
        );
    }

    fn valid_promotion_candidate() -> PromotionCandidateV1 {
        let now = Utc::now();
        PromotionCandidateV1 {
            schema: PromotionCandidateV1::SCHEMA.into(),
            proposal_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            base_revision: "abc123".into(),
            candidate_digest: format!("sha256:{}", "b".repeat(64)),
            changed_paths: vec!["routing/placement.toml".into()],
            risk: PromotionRiskV1::R0Configuration,
            gates: PromotionCandidateV1::REQUIRED_GATES
                .iter()
                .enumerate()
                .map(|(index, name)| PromotionEvidenceGateV1 {
                    name: (*name).into(),
                    passed: true,
                    evidence_digest: format!("sha256:{:064x}", index + 1),
                    independent: *name == "g5_independent_replication",
                })
                .collect(),
            requested_at: now,
            expires_at: now + Duration::minutes(10),
        }
    }

    #[test]
    fn promotion_canary_requires_full_evidence_and_is_signed_and_bounded() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let candidate = valid_promotion_candidate();
        let lease = governor
            .authorize_promotion_canary_at_epoch(&candidate, 7)
            .unwrap();
        let public_key = hex::encode(governor.verifying_key().to_bytes());
        assert!(lease.is_active_at(Utc::now(), 7));
        assert_eq!(lease.max_traffic_basis_points, 1_000);
        assert!(verify_promotion_canary_with_key(&public_key, &lease).is_ok());
        let mut tampered = lease;
        tampered.max_traffic_basis_points = 10_000;
        assert_eq!(
            verify_promotion_canary_with_key(&public_key, &tampered),
            Err(Denial::InvalidSignature)
        );
    }

    #[test]
    fn promotion_canary_denies_risk_understatement_and_missing_gate_evidence() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let mut candidate = valid_promotion_candidate();
        candidate.changed_paths = vec!["crates/rampage-policy/src/lib.rs".into()];
        assert_eq!(
            governor.authorize_promotion_canary_at_epoch(&candidate, 1),
            Err(Denial::PromotionRiskMismatch)
        );
        candidate.changed_paths = vec!["CRATES/RAMPAGE-POLICY/src/lib.rs".into()];
        assert_eq!(
            governor.authorize_promotion_canary_at_epoch(&candidate, 1),
            Err(Denial::PromotionRiskMismatch)
        );
        candidate.changed_paths = vec!["routing/placement.toml".into()];
        candidate.gates[0].passed = false;
        assert_eq!(
            governor.authorize_promotion_canary_at_epoch(&candidate, 1),
            Err(Denial::InvalidPromotionEvidence)
        );
    }

    #[test]
    fn signed_offer_detects_tampering() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let (_, mut offer) = fixtures("desktop", true);
        let identity = NodeIdentityV1 {
            schema: "rampage.node-identity.v1".into(),
            node_id: offer.node_id,
            owner_id: Uuid::now_v7(),
            display_name: "test node".into(),
            device_kind: DeviceKind::Desktop,
            platform: "windows-x86_64".into(),
            public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            enrolled_at: Utc::now(),
            fencing_epoch: 0,
        };
        sign_offer(&signing_key, &mut offer);
        assert!(verify_offer(&identity, &offer).is_ok());
        offer.resources[0].available += 1;
        assert_eq!(
            verify_offer(&identity, &offer),
            Err(Denial::InvalidSignature)
        );
    }

    #[test]
    fn worker_can_verify_governor_lease_without_governor_secret() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (job, offer) = fixtures("desktop", true);
        let lease = governor.authorize_job(&job, &offer, offer.node_id).unwrap();
        let public_key = hex::encode(governor.verifying_key().to_bytes());
        assert!(verify_lease_with_key(&public_key, &lease).is_ok());
        let mut tampered = lease;
        tampered.operation = "not-authorized".into();
        assert_eq!(
            verify_lease_with_key(&public_key, &tampered),
            Err(Denial::InvalidSignature)
        );
    }

    #[test]
    fn storage_lease_is_capacity_bounded_signed_and_tamper_evident() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (_, mut offer) = fixtures("desktop", true);
        offer.resources.push(ResourceQuantityV1 {
            class: ResourceClass::StorageCache,
            capacity: 4096,
            available: 2048,
            unit: "byte".into(),
            labels: BTreeMap::new(),
        });
        let digest = format!("sha256:{}", "a".repeat(64));
        let lease = governor
            .authorize_storage(
                &offer,
                &digest,
                1024,
                StorageClass::Cache,
                ArtifactTransferOperation::Put,
            )
            .unwrap();
        let governor_key = hex::encode(governor.verifying_key().to_bytes());
        assert!(verify_storage_lease_with_key(&governor_key, &lease).is_ok());
        let mut tampered = lease;
        tampered.size_bytes += 1;
        assert_eq!(
            verify_storage_lease_with_key(&governor_key, &tampered),
            Err(Denial::InvalidSignature)
        );
        assert_eq!(
            governor
                .authorize_storage(
                    &offer,
                    &digest,
                    2049,
                    StorageClass::Cache,
                    ArtifactTransferOperation::Put,
                )
                .unwrap_err(),
            Denial::StorageCapacityDenied
        );
    }

    #[test]
    fn cpu_adapter_cannot_claim_gpu_memory_it_does_not_use() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let (mut job, offer) = fixtures("desktop", true);
        job.requests[0].class = ResourceClass::GpuMemory;
        job.requests[0].unit = "byte".into();
        assert_eq!(
            governor
                .authorize_job(&job, &offer, offer.node_id)
                .unwrap_err(),
            Denial::AdapterResourceDenied
        );
    }

    #[test]
    fn signed_mesh_endpoint_detects_address_tampering() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        let now = Utc::now();
        let mut endpoint = MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: "endpoint".into(),
            direct_addresses: vec!["192.0.2.10:4000".into()],
            relay_urls: vec![],
            issued_at: now,
            expires_at: now + Duration::minutes(10),
            signature: String::new(),
        };
        governor.sign_mesh_endpoint(&mut endpoint);
        let public_key = hex::encode(governor.verifying_key().to_bytes());
        assert!(verify_mesh_endpoint_with_key(&public_key, &endpoint).is_ok());
        endpoint.direct_addresses[0] = "192.0.2.99:4000".into();
        assert_eq!(
            verify_mesh_endpoint_with_key(&public_key, &endpoint),
            Err(Denial::InvalidSignature)
        );
    }
}
