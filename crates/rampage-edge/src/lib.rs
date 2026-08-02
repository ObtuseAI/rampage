//! In-process, foreground-only mobile contributor runtime.
//!
//! This crate deliberately has no daemon, sidecar, protected-storage offer, model server, or
//! ambient background authority. A native Android/iOS shell supplies direct platform telemetry on
//! every pulse. If the shell stops pulsing, the signed offer expires and no new work can be leased.

use chrono::{Duration, Utc};
use ed25519_dalek::SigningKey;
use rampage_protocol::{
    AvailabilityV1, DeviceKind, EnrollmentInviteV1, EnrollmentRequestV1, ExecutionPattern,
    ExecutionReceiptV1, MeshControlRequestV1, MeshEndpointRecordV1, NodeIdentityV1, ResourceClass,
    ResourceOfferV1, ResourceQuantityV1, WorkClaimV1, WorkloadCapabilityStatus,
    WorkloadCapabilityV1, WorkloadDomain, WorkloadIsolation,
};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub const MINIMUM_BATTERY_PERCENT: u8 = 40;
pub const MINIMUM_THERMAL_HEADROOM_PERCENT: u8 = 35;
pub const OFFER_TTL_SECONDS: i64 = 20;
const MAX_INVITATION_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgeTelemetry {
    pub platform: String,
    pub device_kind: String,
    pub foreground: bool,
    pub donation_requested: bool,
    pub battery_percent: u8,
    pub on_external_power: bool,
    pub low_power_mode: bool,
    pub thermal_headroom_percent: u8,
}

impl EdgeTelemetry {
    pub fn eligible(&self) -> bool {
        self.foreground
            && self.donation_requested
            && !self.low_power_mode
            && (self.on_external_power || self.battery_percent >= MINIMUM_BATTERY_PERCENT)
            && self.thermal_headroom_percent >= MINIMUM_THERMAL_HEADROOM_PERCENT
            && matches!(self.device_kind.as_str(), "phone" | "tablet")
            && matches!(self.platform.as_str(), "android" | "ios")
    }

    fn denial(&self) -> Option<&'static str> {
        if !matches!(self.platform.as_str(), "android" | "ios") {
            Some("unsupported mobile platform")
        } else if !matches!(self.device_kind.as_str(), "phone" | "tablet") {
            Some("native device class is not phone or tablet")
        } else if !self.foreground || !self.donation_requested {
            Some("foreground donation is not active")
        } else if self.low_power_mode {
            Some("low-power mode is active")
        } else if !self.on_external_power && self.battery_percent < MINIMUM_BATTERY_PERCENT {
            Some("battery reserve is below the owner floor")
        } else if self.thermal_headroom_percent < MINIMUM_THERMAL_HEADROOM_PERCENT {
            Some("thermal headroom is below the owner floor")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeSessionSnapshot {
    pub node_id: Uuid,
    pub controller_endpoint_id: String,
    pub enrolled: bool,
    pub eligible: bool,
    pub offer_expires_at: Option<chrono::DateTime<Utc>>,
    pub receipts_submitted: u64,
    pub last_result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedControllerV1 {
    schema: String,
    endpoint: MeshEndpointRecordV1,
    governor_public_key: String,
    enrolled_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistentIdentityV1 {
    schema: String,
    identity: NodeIdentityV1,
}

pub struct EdgeWorker {
    data_dir: PathBuf,
    runtime: tokio::runtime::Runtime,
    endpoint: iroh::Endpoint,
    destination: iroh::EndpointAddr,
    signing_key: SigningKey,
    identity: NodeIdentityV1,
    authority_store: rampage_storage::CasStore,
    controller_endpoint_id: String,
    receipts_submitted: u64,
    offer_expires_at: Option<chrono::DateTime<Utc>>,
    last_result: String,
}

impl EdgeWorker {
    pub fn open(
        data_dir: impl AsRef<Path>,
        invitation_json: Option<&str>,
        display_name: &str,
        telemetry: &EdgeTelemetry,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            telemetry.eligible(),
            telemetry.denial().unwrap_or("edge policy denied")
        );
        let display_name = display_name.trim();
        anyhow::ensure!(
            !display_name.is_empty()
                && display_name.len() <= 80
                && !display_name.contains(['\r', '\n']),
            "display name must contain 1 to 80 single-line characters"
        );
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;
        anyhow::ensure!(
            !data_dir.join("KILL").exists(),
            "local edge STOP latch is active"
        );

        let signing_key =
            SigningKey::from_bytes(&load_or_create_secret(&data_dir.join("edge.key"))?);
        let pin_path = data_dir.join("controller-pin.json");
        let identity_path = data_dir.join("identity.json");
        let existing_pin = read_optional_json::<PinnedControllerV1>(&pin_path, 64 * 1024)?;
        let existing_identity =
            read_optional_json::<PersistentIdentityV1>(&identity_path, 64 * 1024)?;

        let invitation = invitation_json.map(parse_invitation).transpose()?;
        let pin = if let Some(pin) = existing_pin {
            validate_pin(&pin)?;
            pin
        } else {
            let invite = invitation.as_ref().ok_or_else(|| {
                anyhow::anyhow!("a fresh signed invitation is required for first enrollment")
            })?;
            validate_fresh_invitation(invite)?;
            PinnedControllerV1 {
                schema: "rampage.pinned-controller.v1".into(),
                endpoint: invite.controller_mesh.clone().expect("validated endpoint"),
                governor_public_key: invite.governor_public_key.clone().expect("validated key"),
                enrolled_at: Utc::now(),
            }
        };

        let device_kind = parse_device_kind(&telemetry.device_kind)?;
        let identity = if let Some(persistent) = existing_identity {
            anyhow::ensure!(
                persistent.schema == "rampage.persistent-edge-identity.v1",
                "unsupported identity schema"
            );
            anyhow::ensure!(
                persistent.identity.public_key
                    == hex::encode(signing_key.verifying_key().to_bytes()),
                "edge identity key mismatch"
            );
            anyhow::ensure!(
                persistent.identity.device_kind == device_kind,
                "native device class changed after enrollment"
            );
            anyhow::ensure!(
                persistent.identity.platform == telemetry.platform,
                "native platform changed after enrollment"
            );
            persistent.identity
        } else {
            NodeIdentityV1 {
                schema: "rampage.node-identity.v1".into(),
                node_id: Uuid::now_v7(),
                owner_id: Uuid::now_v7(),
                display_name: display_name.into(),
                device_kind,
                platform: telemetry.platform.clone(),
                public_key: hex::encode(signing_key.verifying_key().to_bytes()),
                enrolled_at: Utc::now(),
                fencing_epoch: 0,
            }
        };

        let mesh_config = mesh_config(&pin.endpoint)?;
        let runtime = tokio::runtime::Runtime::new()?;
        let endpoint = runtime.block_on(rampage_mesh::bind_endpoint(
            signing_key.to_bytes(),
            &mesh_config,
        ))?;
        let destination = rampage_mesh::endpoint_addr_from_pinned_record(&pin.endpoint)?;
        let authority_store = rampage_storage::CasStore::open_with_limits(
            data_dir.join("authority"),
            load_or_create_secret(&data_dir.join("authority.key"))?,
            Some(rampage_storage::StorageLimits {
                cache_bytes: 0,
                scratch_bytes: 0,
                protected_bytes: 0,
            }),
        )?;

        let worker = Self {
            data_dir,
            runtime,
            endpoint,
            destination,
            signing_key,
            identity,
            authority_store,
            controller_endpoint_id: pin.endpoint.endpoint_id.clone(),
            receipts_submitted: 0,
            offer_expires_at: None,
            last_result: "ready".into(),
        };

        if !pin_path.is_file() || !identity_path.is_file() {
            let invite = invitation.as_ref().ok_or_else(|| {
                anyhow::anyhow!("first enrollment requires the invitation secret")
            })?;
            worker.enroll(invite)?;
            write_json_atomic(&pin_path, &pin)?;
            write_json_atomic(
                &identity_path,
                &PersistentIdentityV1 {
                    schema: "rampage.persistent-edge-identity.v1".into(),
                    identity: worker.identity.clone(),
                },
            )?;
        }
        Ok(worker)
    }

    pub fn pulse(&mut self, telemetry: &EdgeTelemetry) -> anyhow::Result<EdgeSessionSnapshot> {
        anyhow::ensure!(
            !self.data_dir.join("KILL").exists(),
            "local edge STOP latch is active"
        );
        anyhow::ensure!(
            telemetry.eligible(),
            telemetry.denial().unwrap_or("edge policy denied")
        );
        anyhow::ensure!(
            device_kind_label(self.identity.device_kind) == telemetry.device_kind
                && self.identity.platform == telemetry.platform,
            "native telemetry does not match the enrolled identity"
        );

        self.flush_receipt_outbox()?;
        let offer = self.make_offer(telemetry)?;
        self.post_json("/v1/offers", &offer)?;
        self.offer_expires_at = Some(offer.expires_at);

        let claim: Option<WorkClaimV1> =
            self.get_json(&format!("/v1/work/claim?node_id={}", self.identity.node_id))?;
        if let Some(claim) = claim {
            let receipt = rampage_agent::execute_claim_with_store(
                &claim,
                self.identity.node_id,
                claim.lease.fencing_epoch,
                &self.signing_key,
                &self.authority_store,
            )?;
            self.persist_and_submit_receipt(&receipt)?;
            self.receipts_submitted = self.receipts_submitted.saturating_add(1);
            self.last_result = format!("receipt {} submitted", receipt.receipt_id);
        } else {
            self.last_result = "eligible; no admitted work waiting".into();
        }
        Ok(self.snapshot(true))
    }

    pub fn snapshot(&self, eligible: bool) -> EdgeSessionSnapshot {
        EdgeSessionSnapshot {
            node_id: self.identity.node_id,
            controller_endpoint_id: self.controller_endpoint_id.clone(),
            enrolled: true,
            eligible,
            offer_expires_at: self.offer_expires_at,
            receipts_submitted: self.receipts_submitted,
            last_result: self.last_result.clone(),
        }
    }

    pub fn shutdown(self) {
        self.runtime.block_on(self.endpoint.close());
    }

    fn enroll(&self, invitation: &EnrollmentInviteV1) -> anyhow::Result<()> {
        let (invite_id, secret) = parse_enrollment_code(&invitation.enrollment_code)?;
        anyhow::ensure!(
            invite_id == invitation.invite_id,
            "invitation id and enrollment code differ"
        );
        let mut request = EnrollmentRequestV1 {
            schema: "rampage.enrollment-request.v1".into(),
            invite_id,
            enrollment_code: secret,
            identity: self.identity.clone(),
            signature: String::new(),
        };
        rampage_policy::sign_enrollment(&self.signing_key, &mut request);
        self.post_json("/v1/nodes/enroll", &request)
    }

    fn make_offer(&self, telemetry: &EdgeTelemetry) -> anyhow::Result<ResourceOfferV1> {
        let now = Utc::now();
        let expires_at = now + Duration::seconds(OFFER_TTL_SECONDS);
        let threads = std::thread::available_parallelism().map_or(1, usize::from) as u64;
        let labels = BTreeMap::from([
            ("device_kind".into(), telemetry.device_kind.clone()),
            ("platform".into(), telemetry.platform.clone()),
            ("edge_session".into(), "foreground".into()),
            ("protected_storage".into(), "false".into()),
        ]);
        let adapters = BTreeSet::from([
            "rampage.hash.v1".to_string(),
            "rampage.eval-shard.v1".to_string(),
        ]);
        let address = self.endpoint.addr();
        let mut endpoint = MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: address.id.to_string(),
            direct_addresses: address.ip_addrs().map(ToString::to_string).collect(),
            relay_urls: address.relay_urls().map(ToString::to_string).collect(),
            issued_at: now,
            expires_at,
            signature: String::new(),
        };
        rampage_policy::sign_mesh_endpoint(&self.signing_key, &mut endpoint);
        let mut offer = ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id: self.identity.node_id,
            observed_at: now,
            expires_at,
            resources: vec![ResourceQuantityV1 {
                class: ResourceClass::CpuCompute,
                capacity: threads,
                available: threads.saturating_sub(1).max(1),
                unit: "logical_core".into(),
                labels,
            }],
            availability: AvailabilityV1 {
                on_ac_power: telemetry.on_external_power,
                battery_percent: Some(telemetry.battery_percent),
                thermal_headroom_percent: telemetry.thermal_headroom_percent,
                foreground_allowed: telemetry.foreground && telemetry.donation_requested,
                owner_idle: true,
            },
            adapters: adapters.clone(),
            workload_capabilities: edge_capabilities(&adapters),
            model_runtimes: Vec::new(),
            link_benchmark: None,
            mesh_endpoint: Some(endpoint),
            signature: String::new(),
        };
        rampage_policy::sign_offer(&self.signing_key, &mut offer);
        Ok(offer)
    }

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

    fn post_json<T: Serialize>(&self, path: &str, value: &T) -> anyhow::Result<()> {
        let response = self.request("POST", path, Some(serde_json::to_value(value)?))?;
        anyhow::ensure!(
            (200..300).contains(&response.status),
            "mesh controller rejected request: {} {}",
            response.status,
            response.body
        );
        Ok(())
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let response = self.request("GET", path, None)?;
        anyhow::ensure!(
            (200..300).contains(&response.status),
            "mesh controller rejected request: {} {}",
            response.status,
            response.body
        );
        Ok(serde_json::from_value(response.body)?)
    }

    fn flush_receipt_outbox(&mut self) -> anyhow::Result<()> {
        let path = self.data_dir.join("receipt-outbox.json");
        if !path.is_file() {
            return Ok(());
        }
        let receipt: ExecutionReceiptV1 = read_json(&path, 1024 * 1024)?;
        self.post_json("/v1/receipts", &receipt)?;
        fs::remove_file(path)?;
        self.receipts_submitted = self.receipts_submitted.saturating_add(1);
        Ok(())
    }

    fn persist_and_submit_receipt(&self, receipt: &ExecutionReceiptV1) -> anyhow::Result<()> {
        let path = self.data_dir.join("receipt-outbox.json");
        write_json_atomic(&path, receipt)?;
        self.post_json("/v1/receipts", receipt)?;
        fs::remove_file(path)?;
        Ok(())
    }
}

fn edge_capabilities(adapters: &BTreeSet<String>) -> Vec<WorkloadCapabilityV1> {
    adapters
        .iter()
        .map(|adapter| {
            let (domain, operation) = match adapter.as_str() {
                "rampage.hash.v1" => (WorkloadDomain::DataProcessing, "hash"),
                "rampage.eval-shard.v1" => (WorkloadDomain::AiEvaluation, "score"),
                _ => unreachable!("bounded edge adapter set"),
            };
            WorkloadCapabilityV1 {
                schema: WorkloadCapabilityV1::SCHEMA.into(),
                adapter: adapter.clone(),
                domain,
                operations: BTreeSet::from([operation.into()]),
                execution_patterns: BTreeSet::from([ExecutionPattern::IndependentShard]),
                resource_classes: BTreeSet::from([ResourceClass::CpuCompute]),
                isolation: WorkloadIsolation::AllowlistedInProcess,
                runtime_digest: format!("shipped-edge:{}", env!("CARGO_PKG_VERSION")),
                checkpointable: false,
                preemptible: true,
                network_allowlist_required: false,
                status: WorkloadCapabilityStatus::Shipped,
                qualification_digest: None,
            }
        })
        .collect()
}

fn mesh_config(record: &MeshEndpointRecordV1) -> anyhow::Result<rampage_mesh::MeshConfig> {
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

fn validate_fresh_invitation(invite: &EnrollmentInviteV1) -> anyhow::Result<()> {
    anyhow::ensure!(
        invite.schema == "rampage.enrollment-invite.v1" && invite.expires_at > Utc::now(),
        "invitation is expired or has an unsupported schema"
    );
    let endpoint = invite
        .controller_mesh
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("invitation omitted its controller mesh endpoint"))?;
    let governor_key = invite
        .governor_public_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("invitation omitted its Governor public key"))?;
    rampage_policy::verify_mesh_endpoint_with_key(governor_key, endpoint)
        .map_err(|_| anyhow::anyhow!("invitation controller endpoint signature is invalid"))?;
    let (code_id, secret) = parse_enrollment_code(&invite.enrollment_code)?;
    anyhow::ensure!(
        code_id == invite.invite_id && secret.len() >= 32,
        "enrollment code is malformed"
    );
    Ok(())
}

fn validate_pin(pin: &PinnedControllerV1) -> anyhow::Result<()> {
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

fn parse_invitation(value: &str) -> anyhow::Result<EnrollmentInviteV1> {
    anyhow::ensure!(
        value.len() <= MAX_INVITATION_BYTES,
        "invitation exceeds 256 KiB"
    );
    Ok(serde_json::from_str(value)?)
}

fn parse_enrollment_code(value: &str) -> anyhow::Result<(Uuid, String)> {
    let (invite_id, secret) = value
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("enrollment code has an invalid shape"))?;
    anyhow::ensure!(
        secret.len() == 32 && secret.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "enrollment secret must contain exactly 32 hexadecimal characters"
    );
    Ok((Uuid::parse_str(invite_id)?, secret.to_string()))
}

fn parse_device_kind(value: &str) -> anyhow::Result<DeviceKind> {
    match value {
        "phone" => Ok(DeviceKind::Phone),
        "tablet" => Ok(DeviceKind::Tablet),
        _ => anyhow::bail!("native device class must be phone or tablet"),
    }
}

fn device_kind_label(value: DeviceKind) -> &'static str {
    match value {
        DeviceKind::Phone => "phone",
        DeviceKind::Tablet => "tablet",
        _ => "unsupported",
    }
}

fn load_or_create_secret(path: &Path) -> anyhow::Result<[u8; 32]> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let bytes = hex::decode(fs::read_to_string(path)?.trim())?;
        return bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("edge secret must contain exactly 32 bytes"));
    }
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(hex::encode(bytes).as_bytes())?;
    file.sync_all()?;
    Ok(bytes)
}

fn read_optional_json<T: DeserializeOwned>(
    path: &Path,
    max_bytes: u64,
) -> anyhow::Result<Option<T>> {
    if !path.is_file() {
        return Ok(None);
    }
    read_json(path, max_bytes).map(Some)
}

fn read_json<T: DeserializeOwned>(path: &Path, max_bytes: u64) -> anyhow::Result<T> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.len() <= max_bytes,
        "bounded JSON input is invalid"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= max_bytes,
        "bounded JSON input is oversized"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let temporary = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> EdgeTelemetry {
        EdgeTelemetry {
            platform: "android".into(),
            device_kind: "phone".into(),
            foreground: true,
            donation_requested: true,
            battery_percent: 75,
            on_external_power: false,
            low_power_mode: false,
            thermal_headroom_percent: 80,
        }
    }

    #[test]
    fn telemetry_policy_denies_every_pressure_boundary() {
        assert!(healthy().eligible());
        for denied in [
            EdgeTelemetry {
                foreground: false,
                ..healthy()
            },
            EdgeTelemetry {
                donation_requested: false,
                ..healthy()
            },
            EdgeTelemetry {
                battery_percent: 39,
                ..healthy()
            },
            EdgeTelemetry {
                low_power_mode: true,
                ..healthy()
            },
            EdgeTelemetry {
                thermal_headroom_percent: 34,
                ..healthy()
            },
            EdgeTelemetry {
                platform: "windows".into(),
                ..healthy()
            },
        ] {
            assert!(!denied.eligible(), "{denied:?}");
            assert!(denied.denial().is_some());
        }
    }

    #[test]
    fn edge_capabilities_are_exact_and_exclude_models_and_storage() {
        let adapters = BTreeSet::from([
            "rampage.hash.v1".to_string(),
            "rampage.eval-shard.v1".to_string(),
        ]);
        let capabilities = edge_capabilities(&adapters);
        assert_eq!(capabilities.len(), 2);
        assert!(capabilities.iter().all(WorkloadCapabilityV1::is_valid));
        assert!(capabilities.iter().all(|capability| {
            capability.resource_classes == BTreeSet::from([ResourceClass::CpuCompute])
                && capability.preemptible
                && capability.isolation == WorkloadIsolation::AllowlistedInProcess
        }));
    }
}
