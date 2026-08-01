use anyhow::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use rampage_protocol::{
    ArtifactRefV1, ComputeStrategy, JobSpecV1, ModelSessionRequestV1, ResourceClass,
    ResourceRequestV1, ShardSetV1, WorkloadTrust,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "rampage",
    version,
    about = "Operate the Rampage personal compute fabric"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:47831")]
    controller: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check local prerequisites and controller reachability.
    Doctor,
    /// Show controller health.
    Status,
    /// Create a ten-minute, one-time enrollment code.
    Invite {
        /// Write the complete signed mesh invitation to a file for another device.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Build a read-only project twin for any local repository or folder.
    Discover { path: PathBuf },
    /// Enroll this machine using the co-located Rampage agent.
    Join {
        /// A complete invite JSON file for remote QUIC, or a local enrollment code.
        invitation: String,
        #[arg(long, default_value = "This Device")]
        name: String,
        #[arg(long, default_value = "desktop")]
        device_kind: String,
    },
    /// Route a local Ollama generation through OnePool and return its signed receipt.
    Generate {
        model: String,
        prompt: String,
        #[arg(long, default_value_t = 0)]
        gpu_memory_gb: u64,
        #[arg(long, default_value_t = 512)]
        max_tokens: u32,
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
    /// Preview a governed local or distributed LLM placement without issuing execution authority.
    ModelPlan {
        #[arg(default_value = "local/target-model")]
        model: String,
        #[arg(long, default_value_t = 40)]
        weights_gib: u64,
        #[arg(long, default_value_t = 4)]
        kv_cache_gib: u64,
        #[arg(long, default_value_t = 32_768)]
        context_tokens: u32,
        #[arg(long, value_enum, default_value_t = StrategyArg::MaximumModelSize)]
        strategy: StrategyArg,
        #[arg(long, default_value_t = 8)]
        max_nodes: u16,
    },
    /// Store a local file in the controller's encrypted content-addressed store.
    ArtifactPut {
        path: PathBuf,
        #[arg(long, default_value = "cache")]
        storage_class: String,
    },
    /// Materialize an artifact from the controller's encrypted store.
    ArtifactGet { digest: String, output: PathBuf },
    /// Copy a controller artifact into an enrolled worker's donated drive capacity.
    ArtifactReplicate {
        digest: String,
        node_id: Uuid,
        #[arg(long, default_value = "cache")]
        storage_class: String,
        #[arg(long, default_value = "application/octet-stream")]
        media_type: String,
    },
    /// Retrieve and verify a recorded worker replica.
    ArtifactRetrieve {
        digest: String,
        node_id: Uuid,
        output: PathBuf,
    },
    /// Stage an artifact to a worker, hash it there, and return a retrievable output artifact.
    ArtifactHash {
        digest: String,
        #[arg(long, default_value_t = 120)]
        timeout_seconds: u64,
    },
    /// Preview placement without issuing authority or mutating controller state.
    Plan {
        #[arg(long, default_value = "rampage.echo.v1")]
        adapter: String,
        #[arg(long, default_value = "echo")]
        operation: String,
        #[arg(long, default_value = "hello from Rampage")]
        value: String,
        #[arg(long, default_value_t = 1)]
        cores: u64,
    },
    /// Show signed link benchmarks and resource offers used by topology-aware placement.
    Topology,
    /// Preview independent shards across the pooled fabric without issuing leases.
    ShardPlan {
        /// One comma-separated numeric partition per independent shard.
        #[arg(required = true)]
        values: Vec<String>,
        #[arg(long, default_value_t = 1)]
        cores_per_shard: u64,
    },
    /// Run independent evaluation shards across every admissible machine and wait for receipts.
    ShardRun {
        /// One comma-separated numeric partition per independent shard.
        #[arg(required = true)]
        values: Vec<String>,
        #[arg(long, default_value_t = 1)]
        cores_per_shard: u64,
        /// Required successful receipts; defaults to every shard.
        #[arg(long)]
        minimum_successes: Option<u32>,
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
    /// Show durable progress and per-machine results for one shard set.
    ShardStatus { set_id: Uuid },
    /// Submit a bounded native job and receive its signed capability lease.
    Run {
        #[arg(long, default_value = "rampage.echo.v1")]
        adapter: String,
        #[arg(long, default_value = "echo")]
        operation: String,
        #[arg(long, default_value = "hello from Rampage")]
        value: String,
        #[arg(long, default_value_t = 1)]
        cores: u64,
        #[arg(long)]
        project_id: Option<Uuid>,
    },
    /// Show every evidence event for one subject identifier.
    Explain { subject_id: String },
    /// Display the ordered evidence stream from a sequence.
    Replay {
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    /// Verify controller startup checks and show its evidence stream.
    Verify,
    /// Set the owner kill latch.
    Stop,
    /// Remove the kill latch only with an exact owner confirmation flag.
    Resume {
        #[arg(long)]
        confirm_owner_resume: bool,
    },
    /// Verify and display the durable event stream.
    Events {
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StrategyArg {
    MaximumModelSize,
    SpeedBoost,
    MaximumThroughput,
    Efficiency,
    AutonomousBalanced,
}

impl From<StrategyArg> for ComputeStrategy {
    fn from(value: StrategyArg) -> Self {
        match value {
            StrategyArg::MaximumModelSize => Self::MaximumModelSize,
            StrategyArg::SpeedBoost => Self::SpeedBoost,
            StrategyArg::MaximumThroughput => Self::MaximumThroughput,
            StrategyArg::Efficiency => Self::Efficiency,
            StrategyArg::AutonomousBalanced => Self::AutonomousBalanced,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Doctor => doctor(&cli.controller).await,
        Command::Status => print_json(fetch_json(&format!("{}/health", cli.controller)).await?),
        Command::Invite { output } => {
            let invite = post_json(
                &format!("{}/v1/enrollment/invites", cli.controller),
                &json!({}),
            )
            .await?;
            if let Some(path) = output {
                std::fs::write(&path, serde_json::to_vec_pretty(&invite)?)?;
                println!("Signed Rampage invite written to {}", path.display());
                Ok(())
            } else {
                print_json(invite)
            }
        }
        Command::Discover { path } => print_json(
            post_json(
                &format!("{}/v1/projects/discover", cli.controller),
                &json!({"path": path}),
            )
            .await?,
        ),
        Command::Join {
            invitation,
            name,
            device_kind,
        } => join(&cli.controller, &invitation, &name, &device_kind),
        Command::Generate {
            model,
            prompt,
            gpu_memory_gb,
            max_tokens,
            timeout_seconds,
        } => {
            let job = make_generation_job(model, prompt, gpu_memory_gb, max_tokens);
            let job_id = job.job_id;
            let lease = post_json(&format!("{}/v1/jobs", cli.controller), &job).await?;
            eprintln!("Rampage lease: {}", serde_json::to_string(&lease)?);
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(timeout_seconds.max(1));
            loop {
                let receipts =
                    fetch_json(&format!("{}/v1/receipts?job_id={job_id}", cli.controller)).await?;
                if let Some(receipt) = receipts.as_array().and_then(|items| items.last()) {
                    break print_json(receipt.clone());
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "generation did not finish before the timeout"
                );
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        Command::ModelPlan {
            model,
            weights_gib,
            kv_cache_gib,
            context_tokens,
            strategy,
            max_nodes,
        } => {
            let request = make_model_session_request(
                model,
                weights_gib,
                kv_cache_gib,
                context_tokens,
                strategy.into(),
                max_nodes,
            )?;
            print_json(
                post_json(
                    &format!("{}/v1/model-sessions/plan", cli.controller),
                    &request,
                )
                .await?,
            )
        }
        Command::ArtifactPut {
            path,
            storage_class,
        } => {
            let payload = std::fs::read(&path)?;
            anyhow::ensure!(
                payload.len() as u64 <= rampage_protocol::MAX_ARTIFACT_TRANSFER_BYTES,
                "artifact exceeds the 64 MiB transfer limit"
            );
            print_json(
                post_json(
                    &format!("{}/v1/artifacts/put", cli.controller),
                    &json!({
                        "data_base64": BASE64.encode(payload),
                        "media_type": "application/octet-stream",
                        "storage_class": parse_storage_class(&storage_class)?
                    }),
                )
                .await?,
            )
        }
        Command::ArtifactGet { digest, output } => {
            let response = fetch_json(&format!(
                "{}/v1/artifacts/get?digest={digest}",
                cli.controller
            ))
            .await?;
            write_artifact_payload(&response, &output)?;
            print_json(json!({"digest": digest, "output": output}))
        }
        Command::ArtifactReplicate {
            digest,
            node_id,
            storage_class,
            media_type,
        } => print_json(
            post_json(
                &format!("{}/v1/artifacts/replicate", cli.controller),
                &json!({
                    "digest": digest,
                    "node_id": node_id,
                    "media_type": media_type,
                    "storage_class": parse_storage_class(&storage_class)?
                }),
            )
            .await?,
        ),
        Command::ArtifactRetrieve {
            digest,
            node_id,
            output,
        } => {
            let response = post_json(
                &format!("{}/v1/artifacts/retrieve", cli.controller),
                &json!({"digest": digest, "node_id": node_id}),
            )
            .await?;
            write_artifact_payload(&response, &output)?;
            print_json(json!({"digest": digest, "node_id": node_id, "output": output}))
        }
        Command::ArtifactHash {
            digest,
            timeout_seconds,
        } => {
            let stored = fetch_json(&format!(
                "{}/v1/artifacts/get?digest={digest}",
                cli.controller
            ))
            .await?;
            let input: ArtifactRefV1 = serde_json::from_value(
                stored
                    .get("artifact")
                    .cloned()
                    .context("artifact metadata is missing")?,
            )?;
            let job = make_artifact_hash_job(input);
            let job_id = job.job_id;
            let lease = post_json(&format!("{}/v1/jobs", cli.controller), &job).await?;
            eprintln!("Rampage lease: {}", serde_json::to_string(&lease)?);
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(timeout_seconds.max(1));
            loop {
                let receipts =
                    fetch_json(&format!("{}/v1/receipts?job_id={job_id}", cli.controller)).await?;
                if let Some(receipt) = receipts.as_array().and_then(|items| items.last()) {
                    break print_json(receipt.clone());
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "artifact job did not finish before the timeout"
                );
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        Command::Plan {
            adapter,
            operation,
            value,
            cores,
        } => {
            let job = make_job(adapter, operation, value, cores, None);
            print_json(post_json(&format!("{}/v1/jobs/plan", cli.controller), &job).await?)
        }
        Command::Topology => {
            print_json(fetch_json(&format!("{}/v1/offers", cli.controller)).await?)
        }
        Command::ShardPlan {
            values,
            cores_per_shard,
        } => {
            let set = make_shard_set(values, cores_per_shard, None)?;
            print_json(post_json(&format!("{}/v1/shard-sets/plan", cli.controller), &set).await?)
        }
        Command::ShardRun {
            values,
            cores_per_shard,
            minimum_successes,
            timeout_seconds,
        } => {
            let set = make_shard_set(values, cores_per_shard, minimum_successes)?;
            let set_id = set.set_id;
            let admission = post_json(&format!("{}/v1/shard-sets", cli.controller), &set).await?;
            eprintln!(
                "Rampage shard admission: {}",
                serde_json::to_string(&admission)?
            );
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(timeout_seconds.max(1));
            loop {
                let status =
                    fetch_json(&format!("{}/v1/shard-sets/{set_id}", cli.controller)).await?;
                if status.get("status").and_then(Value::as_str) != Some("running") {
                    let succeeded =
                        status.get("status").and_then(Value::as_str) == Some("succeeded");
                    print_json(status)?;
                    anyhow::ensure!(succeeded, "shard set finished below its success threshold");
                    break Ok(());
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "shard set did not finish before the timeout; resume with `rampage shard-status {set_id}`"
                );
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        Command::ShardStatus { set_id } => {
            print_json(fetch_json(&format!("{}/v1/shard-sets/{set_id}", cli.controller)).await?)
        }
        Command::Run {
            adapter,
            operation,
            value,
            cores,
            project_id,
        } => {
            let job = make_job(adapter, operation, value, cores, project_id);
            print_json(post_json(&format!("{}/v1/jobs", cli.controller), &job).await?)
        }
        Command::Explain { subject_id } => {
            let events =
                fetch_json(&format!("{}/v1/events?after=0&limit=10000", cli.controller)).await?;
            let filtered = events
                .as_array()
                .into_iter()
                .flatten()
                .filter(|event| {
                    event.get("subject_id").and_then(Value::as_str) == Some(subject_id.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            print_json(Value::Array(filtered))
        }
        Command::Replay { after } | Command::Events { after } => {
            print_json(fetch_json(&format!("{}/v1/events?after={after}", cli.controller)).await?)
        }
        Command::Verify => {
            let health = fetch_json(&format!("{}/health", cli.controller)).await?;
            let events =
                fetch_json(&format!("{}/v1/events?after=0&limit=10000", cli.controller)).await?;
            print_json(json!({
                "controller": health,
                "ledger_verified_on_controller_start": true,
                "event_count": events.as_array().map_or(0, Vec::len)
            }))
        }
        Command::Stop => {
            print_json(post_json(&format!("{}/v1/stop", cli.controller), &json!({})).await?)
        }
        Command::Resume {
            confirm_owner_resume,
        } => {
            anyhow::ensure!(
                confirm_owner_resume,
                "resume refused; pass --confirm-owner-resume after checking the fabric"
            );
            print_json(
                post_json(
                    &format!("{}/v1/resume", cli.controller),
                    &json!({"confirmation": "OWNER_RESUME"}),
                )
                .await?,
            )
        }
    }
}

fn make_job(
    adapter: String,
    operation: String,
    value: String,
    cores: u64,
    project_id: Option<Uuid>,
) -> JobSpecV1 {
    JobSpecV1 {
        schema: JobSpecV1::SCHEMA.into(),
        job_id: Uuid::now_v7(),
        project_id: project_id.unwrap_or_else(Uuid::now_v7),
        submitted_by: Uuid::now_v7(),
        adapter,
        operation,
        arguments: BTreeMap::from([("value".into(), value)]),
        inputs: vec![],
        requests: vec![ResourceRequestV1 {
            class: ResourceClass::CpuCompute,
            minimum: cores,
            preferred: cores,
            unit: "logical_core".into(),
            required_labels: BTreeMap::new(),
        }],
        trust: WorkloadTrust::NativeTrusted,
        restart_tolerant: true,
        network_allowlist: BTreeSet::new(),
        deadline: Utc::now() + Duration::minutes(10),
        idempotency_key: Uuid::now_v7().to_string(),
    }
}

fn make_generation_job(
    model: String,
    prompt: String,
    gpu_memory_gb: u64,
    max_tokens: u32,
) -> JobSpecV1 {
    let mut requests = vec![ResourceRequestV1 {
        class: ResourceClass::CpuCompute,
        minimum: 1,
        preferred: 2,
        unit: "logical_core".into(),
        required_labels: BTreeMap::new(),
    }];
    if gpu_memory_gb > 0 {
        let bytes = gpu_memory_gb.saturating_mul(1024 * 1024 * 1024);
        requests.push(ResourceRequestV1 {
            class: ResourceClass::GpuMemory,
            minimum: bytes,
            preferred: bytes,
            unit: "byte".into(),
            required_labels: BTreeMap::new(),
        });
    }
    JobSpecV1 {
        schema: JobSpecV1::SCHEMA.into(),
        job_id: Uuid::now_v7(),
        project_id: Uuid::now_v7(),
        submitted_by: Uuid::now_v7(),
        adapter: "rampage.ollama.v1".into(),
        operation: "generate".into(),
        arguments: BTreeMap::from([
            ("model".into(), model),
            ("prompt".into(), prompt),
            ("max_tokens".into(), max_tokens.clamp(1, 4096).to_string()),
        ]),
        inputs: vec![],
        requests,
        trust: WorkloadTrust::NativeTrusted,
        restart_tolerant: true,
        network_allowlist: BTreeSet::new(),
        deadline: Utc::now() + Duration::minutes(10),
        idempotency_key: Uuid::now_v7().to_string(),
    }
}

fn make_model_session_request(
    model: String,
    weights_gib: u64,
    kv_cache_gib: u64,
    context_tokens: u32,
    strategy: ComputeStrategy,
    max_nodes: u16,
) -> anyhow::Result<ModelSessionRequestV1> {
    let gib = 1024_u64 * 1024 * 1024;
    let request = ModelSessionRequestV1 {
        schema: ModelSessionRequestV1::SCHEMA.into(),
        session_id: Uuid::now_v7(),
        model_id: model,
        estimated_weight_bytes: weights_gib
            .checked_mul(gib)
            .context("model weight estimate overflowed")?,
        kv_cache_bytes: kv_cache_gib
            .checked_mul(gib)
            .context("KV-cache estimate overflowed")?,
        context_tokens,
        strategy,
        max_nodes,
        deadline: Utc::now() + Duration::minutes(10),
        idempotency_key: Uuid::now_v7().to_string(),
    };
    request.validate_at(Utc::now())?;
    Ok(request)
}

fn make_artifact_hash_job(input: ArtifactRefV1) -> JobSpecV1 {
    JobSpecV1 {
        schema: JobSpecV1::SCHEMA.into(),
        job_id: Uuid::now_v7(),
        project_id: Uuid::now_v7(),
        submitted_by: Uuid::now_v7(),
        adapter: "rampage.artifact-hash.v1".into(),
        operation: "hash_artifact".into(),
        arguments: BTreeMap::new(),
        requests: vec![
            ResourceRequestV1 {
                class: ResourceClass::CpuCompute,
                minimum: 1,
                preferred: 1,
                unit: "logical_core".into(),
                required_labels: BTreeMap::new(),
            },
            ResourceRequestV1 {
                class: ResourceClass::StorageCache,
                minimum: input.size_bytes,
                preferred: input.size_bytes,
                unit: "byte".into(),
                required_labels: BTreeMap::new(),
            },
        ],
        inputs: vec![input],
        trust: WorkloadTrust::NativeTrusted,
        restart_tolerant: true,
        network_allowlist: BTreeSet::new(),
        deadline: Utc::now() + Duration::minutes(10),
        idempotency_key: Uuid::now_v7().to_string(),
    }
}

fn make_shard_set(
    values: Vec<String>,
    cores_per_shard: u64,
    minimum_successes: Option<u32>,
) -> anyhow::Result<ShardSetV1> {
    anyhow::ensure!(!values.is_empty(), "at least one shard is required");
    anyhow::ensure!(
        values.iter().all(|partition| !partition.trim().is_empty()),
        "shard value partitions cannot be empty"
    );
    let set_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let submitted_by = Uuid::now_v7();
    let deadline = Utc::now() + Duration::minutes(10);
    let shards = values
        .into_iter()
        .enumerate()
        .map(|(index, partition)| JobSpecV1 {
            schema: JobSpecV1::SCHEMA.into(),
            job_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            adapter: "rampage.eval-shard.v1".into(),
            operation: "score".into(),
            arguments: BTreeMap::from([("values".into(), partition)]),
            inputs: vec![],
            requests: vec![ResourceRequestV1 {
                class: ResourceClass::CpuCompute,
                minimum: cores_per_shard.max(1),
                preferred: cores_per_shard.max(1),
                unit: "logical_core".into(),
                required_labels: BTreeMap::new(),
            }],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline,
            idempotency_key: format!("{set_id}:shard:{index}"),
        })
        .collect::<Vec<_>>();
    let minimum_successes = minimum_successes.unwrap_or(shards.len() as u32);
    let set = ShardSetV1 {
        schema: ShardSetV1::SCHEMA.into(),
        set_id,
        project_id,
        submitted_by,
        shards,
        minimum_successes,
        deadline,
        idempotency_key: format!("{set_id}:set"),
    };
    set.validate_at(Utc::now())?;
    Ok(set)
}

fn join(controller: &str, invitation: &str, name: &str, kind: &str) -> anyhow::Result<()> {
    let current = std::env::current_exe()?;
    let sibling = current.with_file_name(if cfg!(windows) {
        "rampage-agent.exe"
    } else {
        "rampage-agent"
    });
    let agent = if sibling.is_file() {
        sibling
    } else {
        "rampage-agent".into()
    };
    let invitation_path = PathBuf::from(invitation);
    let mut command = std::process::Command::new(agent);
    if invitation_path.is_file() {
        command.args(["--invite-file", invitation, "--serve"]);
    } else {
        command.args([
            "--controller",
            controller,
            "--enrollment-code",
            invitation,
            "--register",
        ]);
    }
    let status = command
        .args(["--display-name", name, "--device-kind", kind])
        .status()
        .context("could not start rampage-agent; install the complete Rampage bundle")?;
    anyhow::ensure!(status.success(), "rampage-agent enrollment failed");
    Ok(())
}

async fn doctor(controller: &str) -> anyhow::Result<()> {
    println!("Rampage doctor");
    println!(
        "  platform: {}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    match fetch_json(&format!("{controller}/health")).await {
        Ok(health) => {
            println!("  controller: reachable");
            print_json(health)
        }
        Err(error) => {
            println!("  controller: unavailable ({error})");
            println!("  next: start the Rampage desktop app or rampage-controller");
            Ok(())
        }
    }
}

async fn fetch_json(url: &str) -> anyhow::Result<Value> {
    let client = reqwest::Client::new();
    let mut request = client.get(url);
    if let Some(token) = local_token() {
        request = request.header("x-rampage-token", token);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("request failed: {url}"))?;
    decode_json_response(response, url).await
}

async fn post_json<T: serde::Serialize + ?Sized>(url: &str, body: &T) -> anyhow::Result<Value> {
    let client = reqwest::Client::new();
    let mut request = client.post(url).json(body);
    if let Some(token) = local_token() {
        request = request.header("x-rampage-token", token);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("request failed: {url}"))?;
    decode_json_response(response, url).await
}

async fn decode_json_response(response: reqwest::Response, url: &str) -> anyhow::Result<Value> {
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("could not read response body: {url}"))?;
    if !status.is_success() {
        let reason = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| body.trim().to_owned());
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            anyhow::bail!(
                "controller rejected authentication ({status}): {reason}; expected token at {} or RAMPAGE_TOKEN",
                default_token_path().display()
            );
        }
        anyhow::bail!("controller request failed ({status}): {reason}");
    }
    serde_json::from_str(&body).context("response was not valid JSON")
}

fn local_token() -> Option<String> {
    std::env::var("RAMPAGE_TOKEN")
        .ok()
        .or_else(|| std::fs::read_to_string(default_token_path()).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_token_path() -> PathBuf {
    if let Some(data_dir) = std::env::var_os("RAMPAGE_DATA_DIR") {
        return PathBuf::from(data_dir).join("controller.token");
    }
    #[cfg(target_os = "windows")]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        return PathBuf::from(app_data)
            .join("ai.obtuse.rampage")
            .join("runtime")
            .join("controller.token");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("ai.obtuse.rampage")
            .join("runtime")
            .join("controller.token");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(data_home)
                .join("ai.obtuse.rampage")
                .join("runtime")
                .join("controller.token");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("ai.obtuse.rampage")
                .join("runtime")
                .join("controller.token");
        }
    }
    PathBuf::from(".rampage/runtime/controller.token")
}

fn parse_storage_class(value: &str) -> anyhow::Result<&'static str> {
    match value {
        "cache" => Ok("cache"),
        "scratch" => Ok("scratch"),
        "protected" => Ok("protected"),
        _ => anyhow::bail!("storage class must be cache, scratch, or protected"),
    }
}

fn write_artifact_payload(response: &Value, output: &PathBuf) -> anyhow::Result<()> {
    let encoded = response
        .get("data_base64")
        .and_then(Value::as_str)
        .context("artifact response omitted data_base64")?;
    let payload = BASE64
        .decode(encoded)
        .context("artifact response was not valid base64")?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, payload)?;
    Ok(())
}

fn print_json(value: Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
