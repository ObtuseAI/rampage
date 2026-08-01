//! Fail-closed worker execution for the small native adapter set.

use chrono::Utc;
use ed25519_dalek::SigningKey;
use rampage_policy::{sign_receipt, verify_lease_with_key};
use rampage_protocol::{ArtifactRefV1, ExecutionReceiptV1, JobState, StorageClass, WorkClaimV1};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExecutionError {
    #[error("unsupported work-claim schema")]
    WrongSchema,
    #[error("claim is addressed to another node")]
    WrongNode,
    #[error("job and lease do not describe the same authority")]
    LeaseMismatch,
    #[error("lease is expired or fenced")]
    InactiveLease,
    #[error("lease signature is invalid")]
    InvalidLease,
    #[error("adapter or operation is not implemented by this worker")]
    UnsupportedAdapter,
    #[error("required argument {0} is absent")]
    MissingArgument(&'static str),
    #[error("adapter argument is invalid: {0}")]
    InvalidArgument(&'static str),
    #[error("local adapter backend is unavailable: {0}")]
    BackendUnavailable(String),
}

pub fn execute_claim(
    claim: &WorkClaimV1,
    node_id: Uuid,
    fencing_epoch: u64,
    signing_key: &SigningKey,
) -> Result<ExecutionReceiptV1, ExecutionError> {
    execute_claim_inner(claim, node_id, fencing_epoch, signing_key, None)
}

pub fn execute_claim_with_store(
    claim: &WorkClaimV1,
    node_id: Uuid,
    fencing_epoch: u64,
    signing_key: &SigningKey,
    store: &rampage_storage::CasStore,
) -> Result<ExecutionReceiptV1, ExecutionError> {
    execute_claim_inner(claim, node_id, fencing_epoch, signing_key, Some(store))
}

fn execute_claim_inner(
    claim: &WorkClaimV1,
    node_id: Uuid,
    fencing_epoch: u64,
    signing_key: &SigningKey,
    store: Option<&rampage_storage::CasStore>,
) -> Result<ExecutionReceiptV1, ExecutionError> {
    if claim.schema != WorkClaimV1::SCHEMA {
        return Err(ExecutionError::WrongSchema);
    }
    if claim.lease.node_id != node_id {
        return Err(ExecutionError::WrongNode);
    }
    if claim.lease.job_id != claim.job.job_id
        || claim.lease.project_id != claim.job.project_id
        || claim.lease.adapter != claim.job.adapter
        || claim.lease.operation != claim.job.operation
    {
        return Err(ExecutionError::LeaseMismatch);
    }
    let now = Utc::now();
    if !claim.lease.is_active_at(now, fencing_epoch) {
        return Err(ExecutionError::InactiveLease);
    }
    verify_lease_with_key(&claim.governor_public_key, &claim.lease)
        .map_err(|_| ExecutionError::InvalidLease)?;

    let started_at = Utc::now();
    let execution = run_adapter(claim, store);
    let finished_at = Utc::now();
    let (state, result, stdout_digest, stderr_digest, output_bytes, outputs) = match execution {
        Ok(output) => (
            JobState::Succeeded,
            Some(serde_json::Value::String(output.text.clone())),
            Some(format!(
                "sha256:{}",
                hex::encode(Sha256::digest(output.text.as_bytes()))
            )),
            None,
            output.text.len(),
            output.artifacts,
        ),
        Err(error) => {
            let message = error.to_string();
            (
                JobState::Failed,
                None,
                None,
                Some(format!(
                    "sha256:{}",
                    hex::encode(Sha256::digest(message.as_bytes()))
                )),
                0,
                Vec::new(),
            )
        }
    };
    let mut metrics = BTreeMap::new();
    metrics.insert(
        "wall_time_ms".into(),
        (finished_at - started_at).num_microseconds().unwrap_or(0) as f64 / 1_000.0,
    );
    metrics.insert("output_bytes".into(), output_bytes as f64);
    let mut receipt = ExecutionReceiptV1 {
        schema: "rampage.execution-receipt.v1".into(),
        receipt_id: Uuid::now_v7(),
        lease_id: claim.lease.lease_id,
        job_id: claim.job.job_id,
        node_id,
        state,
        started_at,
        finished_at,
        outputs,
        metrics,
        stdout_digest,
        stderr_digest,
        result,
        signature: String::new(),
    };
    sign_receipt(signing_key, &mut receipt);
    Ok(receipt)
}

struct AdapterOutput {
    text: String,
    artifacts: Vec<ArtifactRefV1>,
}

impl AdapterOutput {
    fn text(value: String) -> Self {
        Self {
            text: value,
            artifacts: Vec::new(),
        }
    }
}

fn run_adapter(
    claim: &WorkClaimV1,
    store: Option<&rampage_storage::CasStore>,
) -> Result<AdapterOutput, ExecutionError> {
    match (claim.job.adapter.as_str(), claim.job.operation.as_str()) {
        ("rampage.echo.v1", "echo") => Ok(AdapterOutput::text(
            claim
                .job
                .arguments
                .get("value")
                .cloned()
                .ok_or(ExecutionError::MissingArgument("value"))?,
        )),
        ("rampage.hash.v1", "hash") => {
            let value = claim
                .job
                .arguments
                .get("value")
                .ok_or(ExecutionError::MissingArgument("value"))?;
            Ok(AdapterOutput::text(hex::encode(Sha256::digest(
                value.as_bytes(),
            ))))
        }
        ("rampage.eval-shard.v1", "score") => {
            let values = claim
                .job
                .arguments
                .get("values")
                .ok_or(ExecutionError::MissingArgument("values"))?;
            let mut count = 0_u64;
            let mut sum = 0.0_f64;
            for value in values.split(',') {
                sum += value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| ExecutionError::UnsupportedAdapter)?;
                count += 1;
            }
            if count == 0 {
                return Err(ExecutionError::MissingArgument("values"));
            }
            Ok(AdapterOutput::text(format!("{:.12}", sum / count as f64)))
        }
        ("rampage.ollama.v1", "generate") => run_ollama(claim).map(AdapterOutput::text),
        ("rampage.artifact-hash.v1", "hash_artifact") => {
            let store = store.ok_or_else(|| {
                ExecutionError::BackendUnavailable("artifact store is unavailable".into())
            })?;
            let input = claim
                .job
                .inputs
                .first()
                .filter(|_| claim.job.inputs.len() == 1)
                .ok_or(ExecutionError::MissingArgument("one input artifact"))?;
            let stored = store
                .head(&input.digest)
                .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?;
            if stored.size_bytes != input.size_bytes || stored.media_type != input.media_type {
                return Err(ExecutionError::InvalidArgument("input artifact metadata"));
            }
            let payload = store
                .get(&input.digest)
                .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?;
            let observed_digest = format!("sha256:{}", hex::encode(Sha256::digest(&payload)));
            if observed_digest != input.digest {
                return Err(ExecutionError::InvalidArgument("input artifact digest"));
            }
            let report = serde_json::json!({
                "schema": "rampage.artifact-hash-result.v1",
                "input_digest": input.digest,
                "input_size_bytes": input.size_bytes,
                "observed_digest": observed_digest
            })
            .to_string();
            let output = store
                .put(
                    report.as_bytes(),
                    rampage_storage::PutOptions {
                        media_type: "application/json".into(),
                        storage_class: StorageClass::Cache,
                        required_replicas: 1,
                    },
                )
                .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?;
            Ok(AdapterOutput {
                text: report,
                artifacts: vec![output],
            })
        }
        _ => Err(ExecutionError::UnsupportedAdapter),
    }
}

fn run_ollama(claim: &WorkClaimV1) -> Result<String, ExecutionError> {
    let model = claim
        .job
        .arguments
        .get("model")
        .ok_or(ExecutionError::MissingArgument("model"))?;
    let prompt = claim
        .job
        .arguments
        .get("prompt")
        .ok_or(ExecutionError::MissingArgument("prompt"))?;
    if model.is_empty()
        || model.len() > 200
        || !model
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".:_/-".contains(character))
    {
        return Err(ExecutionError::InvalidArgument("model"));
    }
    if prompt.len() > 128 * 1024 {
        return Err(ExecutionError::InvalidArgument("prompt"));
    }
    let num_predict = claim
        .job
        .arguments
        .get("max_tokens")
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| ExecutionError::InvalidArgument("max_tokens"))?
        .unwrap_or(512)
        .clamp(1, 4096);
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?
        .post("http://127.0.0.1:11434/api/generate")
        .json(&serde_json::json!({
            "model": model,
            "prompt": prompt,
            "stream": false,
            "options": {"num_predict": num_predict}
        }))
        .send()
        .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?
        .error_for_status()
        .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?;
    let body: serde_json::Value = response
        .json()
        .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?;
    body.get("response")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ExecutionError::BackendUnavailable("response text is missing".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rampage_policy::{Governor, GovernorConfig};
    use rampage_protocol::{
        AvailabilityV1, CapabilityLeaseV1, JobSpecV1, ResourceOfferV1, WorkloadTrust,
    };
    use std::collections::{BTreeMap, BTreeSet};

    fn claim(signing_key: &SigningKey, node_id: Uuid) -> WorkClaimV1 {
        let now = Utc::now();
        let job = JobSpecV1 {
            schema: JobSpecV1::SCHEMA.into(),
            job_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            submitted_by: Uuid::now_v7(),
            adapter: "rampage.echo.v1".into(),
            operation: "echo".into(),
            arguments: BTreeMap::from([("value".into(), "hello".into())]),
            inputs: vec![],
            requests: vec![],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline: now + Duration::minutes(2),
            idempotency_key: "agent-exec-test".into(),
        };
        let offer = ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id,
            observed_at: now,
            expires_at: now + Duration::minutes(1),
            resources: vec![],
            availability: AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 90,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::from(["rampage.echo.v1".into()]),
            model_runtimes: Vec::new(),
            link_benchmark: None,
            mesh_endpoint: None,
            signature: "test-offer".into(),
        };
        let governor = Governor::ephemeral(GovernorConfig::default());
        let lease: CapabilityLeaseV1 = governor.authorize_job(&job, &offer, node_id).unwrap();
        let claim = WorkClaimV1 {
            schema: WorkClaimV1::SCHEMA.into(),
            job,
            lease,
            governor_public_key: hex::encode(governor.verifying_key().to_bytes()),
        };
        assert_eq!(signing_key.verifying_key().to_bytes().len(), 32);
        claim
    }

    #[test]
    fn executes_only_a_signed_scoped_claim() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let node_id = Uuid::now_v7();
        let claim = claim(&key, node_id);
        let receipt = execute_claim(&claim, node_id, 0, &key).unwrap();
        assert_eq!(receipt.state, JobState::Succeeded);
        assert!(receipt.stdout_digest.unwrap().starts_with("sha256:"));
    }

    #[test]
    fn rejects_tampered_claim() {
        let key = SigningKey::generate(&mut rand::rngs::OsRng);
        let node_id = Uuid::now_v7();
        let mut claim = claim(&key, node_id);
        claim.lease.operation = "different".into();
        assert!(matches!(
            execute_claim(&claim, node_id, 0, &key),
            Err(ExecutionError::LeaseMismatch)
        ));
    }
}
