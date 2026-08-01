//! Non-agentic policy Governor. Missing or ambiguous facts deny authority.

use chrono::{Duration, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rampage_protocol::{
    ArtifactTransferOperation, CapabilityLeaseV1, DeviceKind, EnrollmentRequestV1,
    ExecutionReceiptV1, JobSpecV1, MAX_ARTIFACT_TRANSFER_BYTES, MeshEndpointRecordV1,
    NodeIdentityV1, ResourceClass, ResourceOfferV1, StorageClass, StorageLeaseV1,
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
            mobile_min_battery_percent: 40,
            mobile_min_thermal_headroom_percent: 35,
            killed: false,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Denial {
    #[error("owner kill latch is active")]
    KillLatch,
    #[error("adapter is not allowlisted")]
    AdapterDenied,
    #[error("selected offer does not advertise the requested adapter")]
    OfferAdapterMismatch,
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
    #[error("risk class requires human review")]
    HumanReviewRequired,
    #[error("autopilot is not enabled for this project")]
    AutopilotNotEnabled,
    #[error("lease signature is invalid")]
    InvalidSignature,
    #[error("artifact digest or size is invalid")]
    InvalidArtifact,
    #[error("node did not offer enough storage in the requested class")]
    StorageCapacityDenied,
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
            fencing_epoch: 0,
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
            RiskClass::R2ProtectedChange | RiskClass::R3AuthorityCritical => {
                Err(Denial::HumanReviewRequired)
            }
        }
    }

    pub fn authorize_storage(
        &self,
        offer: &ResourceOfferV1,
        digest: &str,
        size_bytes: u64,
        storage_class: StorageClass,
        operation: ArtifactTransferOperation,
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
            fencing_epoch: 0,
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
        AvailabilityV1, ResourceClass, ResourceQuantityV1, ResourceRequestV1, WorkloadTrust,
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
    fn authority_critical_promotion_requires_human() {
        let governor = Governor::ephemeral(GovernorConfig::default());
        assert_eq!(
            governor.authorize_promotion(Uuid::now_v7(), RiskClass::R3AuthorityCritical),
            Err(Denial::HumanReviewRequired)
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
