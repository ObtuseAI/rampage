//! Fail-closed worker execution for the small native adapter set.

use chrono::Utc;
use ed25519_dalek::SigningKey;
use rampage_policy::{sign_receipt, verify_lease_with_key};
use rampage_protocol::{
    ArtifactRefV1, ExecutionReceiptV1, JobState, ResourceClass, StorageClass, WorkClaimV1,
};
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
    #[error("durable authority state rejected the lease: {0}")]
    AuthorityState(String),
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
    verify_lease_with_key(&claim.governor_public_key, &claim.lease)
        .map_err(|_| ExecutionError::InvalidLease)?;
    let now = Utc::now();
    if !claim.lease.is_active_at(now, fencing_epoch) {
        return Err(ExecutionError::InactiveLease);
    }
    if let Some(store) = store {
        store
            .accept_authority(
                "governor",
                claim.lease.fencing_epoch,
                &claim.lease.nonce,
                claim.lease.expires_at,
            )
            .map_err(|error| ExecutionError::AuthorityState(error.to_string()))?;
    }

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
        ("rampage.benchmark.v1", "sha256_chain") => {
            run_cpu_benchmark(claim).map(AdapterOutput::text)
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

fn run_cpu_benchmark(claim: &WorkClaimV1) -> Result<String, ExecutionError> {
    const MAX_LANES: u64 = 64;
    const MAX_ITERATIONS_PER_LANE: u64 = 20_000_000;
    const MAX_TOTAL_ITERATIONS: u64 = 400_000_000;

    let lanes = claim
        .lease
        .granted
        .iter()
        .find(|resource| {
            resource.class == ResourceClass::CpuCompute && resource.unit == "logical_core"
        })
        .map(|resource| resource.available)
        .filter(|lanes| (1..=MAX_LANES).contains(lanes))
        .ok_or(ExecutionError::InvalidArgument("benchmark CPU grant"))?;
    let iterations_per_lane = claim
        .job
        .arguments
        .get("iterations_per_lane")
        .ok_or(ExecutionError::MissingArgument("iterations_per_lane"))?
        .parse::<u64>()
        .map_err(|_| ExecutionError::InvalidArgument("iterations_per_lane"))?;
    if !(1..=MAX_ITERATIONS_PER_LANE).contains(&iterations_per_lane)
        || lanes.saturating_mul(iterations_per_lane) > MAX_TOTAL_ITERATIONS
    {
        return Err(ExecutionError::InvalidArgument("iterations_per_lane"));
    }

    let started = std::time::Instant::now();
    let lane_digests = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(lanes as usize);
        for lane in 0..lanes {
            let job_id = claim.job.job_id;
            handles.push(scope.spawn(move || {
                let mut digest = Sha256::new()
                    .chain_update(b"rampage.cpu-benchmark.v1\0")
                    .chain_update(job_id.as_bytes())
                    .chain_update(lane.to_le_bytes())
                    .finalize()
                    .to_vec();
                for iteration in 0..iterations_per_lane {
                    digest = Sha256::new()
                        .chain_update(&digest)
                        .chain_update(lane.to_le_bytes())
                        .chain_update(iteration.to_le_bytes())
                        .finalize()
                        .to_vec();
                }
                digest
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle.join().map_err(|_| {
                    ExecutionError::BackendUnavailable("benchmark worker thread panicked".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    let elapsed = started.elapsed();
    let total_hashes = lanes.saturating_mul(iterations_per_lane);
    let elapsed_seconds = elapsed.as_secs_f64().max(f64::EPSILON);
    let hashes_per_second = (total_hashes as f64 / elapsed_seconds).round() as u64;
    let result_digest = format!(
        "sha256:{}",
        hex::encode(
            Sha256::new()
                .chain_update(b"rampage.cpu-benchmark-result.v1\0")
                .chain_update(lane_digests.concat())
                .finalize()
        )
    );
    Ok(serde_json::json!({
        "schema": "rampage.cpu-benchmark-result.v1",
        "lanes": lanes,
        "iterations_per_lane": iterations_per_lane,
        "total_hashes": total_hashes,
        "elapsed_ms": elapsed.as_secs_f64() * 1_000.0,
        "hashes_per_second": hashes_per_second,
        "result_digest": result_digest
    })
    .to_string())
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
            "think": false,
            "options": {"num_predict": num_predict}
        }))
        .send()
        .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?
        .error_for_status()
        .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?;
    let body: serde_json::Value = response
        .json()
        .map_err(|error| ExecutionError::BackendUnavailable(error.to_string()))?;
    required_ollama_response(&body)
}

fn required_ollama_response(body: &serde_json::Value) -> Result<String, ExecutionError> {
    body.get("response")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            ExecutionError::BackendUnavailable(
                "Ollama completed without user-visible answer text".into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{TryRng as _, rngs::SysRng};

    fn system_signing_key() -> SigningKey {
        let mut secret = [0_u8; 32];
        SysRng
            .try_fill_bytes(&mut secret)
            .expect("system randomness is required for test identities");
        SigningKey::from_bytes(&secret)
    }
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
            workload_capabilities: Vec::new(),
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
        let key = system_signing_key();
        let node_id = Uuid::now_v7();
        let claim = claim(&key, node_id);
        let receipt = execute_claim(&claim, node_id, 0, &key).unwrap();
        assert_eq!(receipt.state, JobState::Succeeded);
        assert!(receipt.stdout_digest.unwrap().starts_with("sha256:"));
    }

    #[test]
    fn rejects_tampered_claim() {
        let key = system_signing_key();
        let node_id = Uuid::now_v7();
        let mut claim = claim(&key, node_id);
        claim.lease.operation = "different".into();
        assert!(matches!(
            execute_claim(&claim, node_id, 0, &key),
            Err(ExecutionError::LeaseMismatch)
        ));
    }

    #[test]
    fn durable_worker_authority_rejects_lease_replay() {
        let key = system_signing_key();
        let node_id = Uuid::now_v7();
        let claim = claim(&key, node_id);
        let temp = tempfile::tempdir().unwrap();
        let store = rampage_storage::CasStore::open(temp.path(), [8_u8; 32]).unwrap();
        execute_claim_with_store(&claim, node_id, claim.lease.fencing_epoch, &key, &store).unwrap();
        assert!(matches!(
            execute_claim_with_store(&claim, node_id, claim.lease.fencing_epoch, &key, &store),
            Err(ExecutionError::AuthorityState(_))
        ));
    }

    #[test]
    fn sustained_benchmark_is_bounded_by_the_signed_cpu_grant() {
        let key = system_signing_key();
        let node_id = Uuid::now_v7();
        let mut claim = claim(&key, node_id);
        claim.job.adapter = "rampage.benchmark.v1".into();
        claim.job.operation = "sha256_chain".into();
        claim.job.arguments = BTreeMap::from([("iterations_per_lane".into(), "10".into())]);
        claim.lease.adapter = claim.job.adapter.clone();
        claim.lease.operation = claim.job.operation.clone();
        claim.lease.granted = vec![rampage_protocol::ResourceQuantityV1 {
            class: ResourceClass::CpuCompute,
            capacity: 2,
            available: 2,
            unit: "logical_core".into(),
            labels: BTreeMap::new(),
        }];
        let output = run_cpu_benchmark(&claim).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["lanes"], 2);
        assert_eq!(parsed["total_hashes"], 20);
        assert!(
            parsed["result_digest"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );

        claim.job.arguments.insert(
            "iterations_per_lane".into(),
            (MAX_TEST_ITERATIONS + 1).to_string(),
        );
        assert!(matches!(
            run_cpu_benchmark(&claim),
            Err(ExecutionError::InvalidArgument("iterations_per_lane"))
        ));
    }

    #[test]
    fn ollama_receipts_require_user_visible_answer_text() {
        assert_eq!(
            required_ollama_response(&serde_json::json!({"response": "verified"})).unwrap(),
            "verified"
        );
        assert!(matches!(
            required_ollama_response(&serde_json::json!({"response": "  "})),
            Err(ExecutionError::BackendUnavailable(message))
                if message.contains("without user-visible answer text")
        ));
        assert!(required_ollama_response(&serde_json::json!({})).is_err());
    }

    const MAX_TEST_ITERATIONS: u64 = 20_000_000;
}
