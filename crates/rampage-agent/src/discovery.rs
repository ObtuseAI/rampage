use rampage_protocol::{
    ExecutionPattern, InstalledModelV1, ModelBackend, ModelMemoryKind, ModelParallelism,
    ModelRuntimeOfferV1, ModelRuntimeStatus, ResourceClass, ResourceQuantityV1,
    WorkloadCapabilityStatus, WorkloadCapabilityV1, WorkloadDomain, WorkloadIsolation,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    process::Command,
};
use sysinfo::{Disks, System};

const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug)]
pub struct DiscoverySnapshot {
    pub resources: Vec<ResourceQuantityV1>,
    pub on_ac_power: bool,
    pub battery_percent: Option<u8>,
    pub thermal_headroom_percent: u8,
    pub owner_idle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NvidiaGpu {
    name: String,
    uuid: String,
    memory_total_mib: u64,
    memory_free_mib: u64,
    utilization_percent: u64,
    temperature_c: u64,
}

pub fn discover(mut labels: BTreeMap<String, String>, data_dir: &Path) -> DiscoverySnapshot {
    let mut system = System::new_all();
    system.refresh_all();
    let owner_idle = system.global_cpu_usage() < 20.0;
    let mut resources = vec![
        ResourceQuantityV1 {
            class: ResourceClass::CpuCompute,
            capacity: system.cpus().len() as u64,
            available: system.cpus().len().saturating_sub(1).max(1) as u64,
            unit: "logical_core".into(),
            labels: labels.clone(),
        },
        ResourceQuantityV1 {
            class: ResourceClass::RamWorkingSet,
            capacity: system.total_memory(),
            available: system.available_memory().saturating_mul(8) / 10,
            unit: "byte".into(),
            labels: labels.clone(),
        },
        ResourceQuantityV1 {
            class: ResourceClass::RamCache,
            capacity: system.available_memory(),
            available: system.available_memory().saturating_mul(6) / 10,
            unit: "byte".into(),
            labels: labels.clone(),
        },
    ];

    let gpus = discover_nvidia_gpus();
    let thermal_headroom_percent = gpus
        .iter()
        .map(|gpu| gpu.temperature_c)
        .max()
        .map(|temperature| {
            90_u64
                .saturating_sub(temperature)
                .saturating_mul(2)
                .min(100) as u8
        })
        .unwrap_or(70);
    if !gpus.is_empty() {
        labels.insert("gpu_vendor".into(), "nvidia".into());
        labels.insert(
            "gpu_models".into(),
            gpus.iter()
                .map(|gpu| gpu.name.as_str())
                .collect::<Vec<_>>()
                .join(" | "),
        );
        labels.insert(
            "gpu_uuids".into(),
            gpus.iter()
                .map(|gpu| gpu.uuid.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        labels.insert("gpu_count".into(), gpus.len().to_string());
        resources.push(ResourceQuantityV1 {
            class: ResourceClass::GpuCompute,
            capacity: gpus.len() as u64 * 100,
            available: gpus
                .iter()
                .map(|gpu| 100_u64.saturating_sub(gpu.utilization_percent.min(100)))
                .sum(),
            unit: "percent_device".into(),
            labels: labels.clone(),
        });
        resources.push(ResourceQuantityV1 {
            class: ResourceClass::GpuMemory,
            capacity: gpus.iter().map(|gpu| gpu.memory_total_mib).sum::<u64>() * 1024 * 1024,
            available: gpus.iter().map(|gpu| gpu.memory_free_mib).sum::<u64>() * 1024 * 1024,
            unit: "byte".into(),
            labels: labels.clone(),
        });
    }

    resources.extend(discover_storage(&labels, data_dir));
    let (on_ac_power, battery_percent) = power_status();
    DiscoverySnapshot {
        resources,
        on_ac_power,
        battery_percent,
        thermal_headroom_percent,
        owner_idle,
    }
}

pub fn ollama_available(base_url: &str) -> bool {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(750))
        .build()
        .and_then(|client| client.get(format!("{base_url}/api/tags")).send())
        .is_ok_and(|response| response.status().is_success())
}

pub fn discover_workload_capabilities(
    adapters: &BTreeSet<String>,
    model_runtimes: &[ModelRuntimeOfferV1],
) -> Vec<WorkloadCapabilityV1> {
    let shipped_runtime = format!("shipped-agent:{}", env!("CARGO_PKG_VERSION"));
    let mut capabilities = adapters
        .iter()
        .filter_map(|adapter| {
            let (domain, operations, patterns, resources, isolation, runtime_digest, preemptible) =
                match adapter.as_str() {
                    "rampage.echo.v1" => (
                        WorkloadDomain::EdgeUtility,
                        BTreeSet::from(["echo".into()]),
                        BTreeSet::from([ExecutionPattern::WholeWorkload]),
                        BTreeSet::from([ResourceClass::CpuCompute]),
                        WorkloadIsolation::AllowlistedInProcess,
                        shipped_runtime.clone(),
                        true,
                    ),
                    "rampage.hash.v1" => (
                        WorkloadDomain::DataProcessing,
                        BTreeSet::from(["hash".into()]),
                        BTreeSet::from([
                            ExecutionPattern::WholeWorkload,
                            ExecutionPattern::IndependentShard,
                        ]),
                        BTreeSet::from([ResourceClass::CpuCompute]),
                        WorkloadIsolation::AllowlistedInProcess,
                        shipped_runtime.clone(),
                        true,
                    ),
                    "rampage.eval-shard.v1" => (
                        WorkloadDomain::AiEvaluation,
                        BTreeSet::from(["score".into()]),
                        BTreeSet::from([ExecutionPattern::IndependentShard]),
                        BTreeSet::from([ResourceClass::CpuCompute]),
                        WorkloadIsolation::AllowlistedInProcess,
                        shipped_runtime.clone(),
                        true,
                    ),
                    "rampage.artifact-hash.v1" => (
                        WorkloadDomain::Storage,
                        BTreeSet::from(["hash_artifact".into()]),
                        BTreeSet::from([
                            ExecutionPattern::WholeWorkload,
                            ExecutionPattern::IndependentShard,
                        ]),
                        BTreeSet::from([ResourceClass::CpuCompute, ResourceClass::StorageCache]),
                        WorkloadIsolation::AllowlistedInProcess,
                        shipped_runtime.clone(),
                        true,
                    ),
                    "rampage.ollama.v1" => {
                        let runtime_digest = model_runtimes
                            .iter()
                            .find(|runtime| runtime.adapter == *adapter)
                            .map(|runtime| runtime.runtime_digest.clone())?;
                        (
                            WorkloadDomain::AiInference,
                            BTreeSet::from(["generate".into(), "chat".into()]),
                            BTreeSet::from([
                                ExecutionPattern::WholeWorkload,
                                ExecutionPattern::Replica,
                                ExecutionPattern::StreamingService,
                            ]),
                            BTreeSet::from([
                                ResourceClass::CpuCompute,
                                ResourceClass::GpuCompute,
                                ResourceClass::GpuMemory,
                                ResourceClass::RamWorkingSet,
                            ]),
                            WorkloadIsolation::ExternalService,
                            runtime_digest,
                            false,
                        )
                    }
                    _ => return None,
                };
            Some(WorkloadCapabilityV1 {
                schema: WorkloadCapabilityV1::SCHEMA.into(),
                adapter: adapter.clone(),
                domain,
                operations,
                execution_patterns: patterns,
                resource_classes: resources,
                isolation,
                runtime_digest,
                checkpointable: false,
                preemptible,
                network_allowlist_required: false,
                status: WorkloadCapabilityStatus::Shipped,
                qualification_digest: None,
            })
        })
        .collect::<Vec<_>>();
    capabilities.sort_by(|left, right| left.adapter.cmp(&right.adapter));
    capabilities
}

#[derive(Debug, Deserialize)]
struct OllamaTags {
    #[serde(default)]
    models: Vec<OllamaTag>,
}

#[derive(Debug, Deserialize)]
struct OllamaTag {
    name: Option<String>,
    model: Option<String>,
    size: u64,
    digest: String,
}

fn discover_ollama_models(base_url: &str) -> Result<Vec<InstalledModelV1>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|error| format!("could not build Ollama discovery client: {error}"))?;
    let response = client
        .get(format!("{base_url}/api/tags"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("could not list local Ollama models: {error}"))?;
    let mut payload = Vec::new();
    response
        .take(1024 * 1024 + 1)
        .read_to_end(&mut payload)
        .map_err(|error| format!("could not read local Ollama model list: {error}"))?;
    if payload.len() > 1024 * 1024 {
        return Err("local Ollama model list exceeds the one MiB safety limit".into());
    }
    parse_ollama_models(&payload)
}

fn parse_ollama_models(payload: &[u8]) -> Result<Vec<InstalledModelV1>, String> {
    let payload: OllamaTags = serde_json::from_slice(payload)
        .map_err(|error| format!("local Ollama model list is invalid: {error}"))?;
    if payload.models.len() > 128 {
        return Err("local Ollama model list exceeds the 128-model safety limit".into());
    }
    let mut models = Vec::with_capacity(payload.models.len());
    let mut names = BTreeSet::new();
    for candidate in payload.models {
        let model_id = candidate
            .model
            .filter(|value| !value.trim().is_empty())
            .or(candidate.name)
            .ok_or_else(|| "local Ollama model omitted its identifier".to_string())?;
        let normalized_id = model_id.trim().to_ascii_lowercase();
        if normalized_id.ends_with(":cloud") || normalized_id.ends_with("-cloud") {
            continue;
        }
        let digest = candidate.digest.trim().to_ascii_lowercase();
        let artifact_digest = if digest.starts_with("sha256:") {
            digest
        } else {
            format!("sha256:{digest}")
        };
        let model = InstalledModelV1 {
            schema: InstalledModelV1::SCHEMA.into(),
            model_id,
            artifact_digest,
            artifact_size_bytes: candidate.size,
        };
        if !model.is_valid() || !names.insert(model.model_id.clone()) {
            return Err("local Ollama model list contains a malformed or duplicate entry".into());
        }
        models.push(model);
    }
    models.sort_by(|left, right| left.model_id.cmp(&right.model_id));
    Ok(models)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelRuntimeManifestV1 {
    schema: String,
    profiles: Vec<ModelRuntimeOfferV1>,
}

/// Advertise only runtimes that are already local or backed by an explicit qualification
/// manifest. Merely finding a GPU never implies that cross-host model sharding is safe.
pub fn discover_model_runtimes(
    resources: &[ResourceQuantityV1],
    has_ollama: bool,
    ollama_base_url: &str,
) -> Result<Vec<ModelRuntimeOfferV1>, String> {
    let mut profiles = Vec::new();
    if has_ollama {
        let gpu_bytes = resource_bytes(resources, ResourceClass::GpuMemory);
        let ram_bytes = resource_bytes(resources, ResourceClass::RamWorkingSet);
        let (memory_kind, available_model_bytes) = if gpu_bytes > 0 && ram_bytes > 0 {
            (ModelMemoryKind::Hybrid, gpu_bytes.saturating_add(ram_bytes))
        } else if gpu_bytes > 0 {
            (ModelMemoryKind::DedicatedGpu, gpu_bytes)
        } else {
            (ModelMemoryKind::Host, ram_bytes)
        };
        if available_model_bytes > 0 {
            let runtime_version =
                ollama_version(ollama_base_url).unwrap_or_else(|| "detected-local".into());
            let installed_models = discover_ollama_models(ollama_base_url)?;
            profiles.push(ModelRuntimeOfferV1 {
                schema: ModelRuntimeOfferV1::SCHEMA.into(),
                adapter: "rampage.ollama.v1".into(),
                backend: ModelBackend::LocalOllama,
                runtime_version: runtime_version.clone(),
                runtime_digest: format!("shipped-local:{runtime_version}"),
                compatibility_key: format!(
                    "ollama-{}-{}-{runtime_version}",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
                memory_kind,
                available_model_bytes,
                supported_parallelism: BTreeSet::from([
                    ModelParallelism::WholeModel,
                    ModelParallelism::Replica,
                ]),
                status: ModelRuntimeStatus::ShippedLocal,
                installed_models,
                certification_digest: None,
            });
        }
    }

    let Some(path) = std::env::var_os("RAMPAGE_MODEL_RUNTIME_MANIFEST") else {
        return Ok(profiles);
    };
    let payload = std::fs::read(&path).map_err(|error| {
        format!(
            "could not read model runtime manifest {}: {error}",
            Path::new(&path).display()
        )
    })?;
    let manifest: ModelRuntimeManifestV1 = serde_json::from_slice(&payload)
        .map_err(|error| format!("model runtime manifest is invalid: {error}"))?;
    if manifest.schema != "rampage.model-runtime-manifest.v1" {
        return Err("model runtime manifest has an unsupported schema".into());
    }
    if manifest.profiles.len() > 16 {
        return Err("model runtime manifest exceeds the 16-profile limit".into());
    }
    let mut identities = BTreeSet::new();
    for profile in manifest.profiles {
        let adapter_matches_backend = matches!(
            (profile.backend, profile.adapter.as_str()),
            (ModelBackend::ExoMlx, "rampage.exo-mlx.v1")
                | (ModelBackend::VllmRay, "rampage.vllm-ray.v1")
        );
        if profile.schema != ModelRuntimeOfferV1::SCHEMA
            || profile.adapter.trim().is_empty()
            || profile.runtime_version.trim().is_empty()
            || profile.compatibility_key.trim().is_empty()
            || profile.available_model_bytes == 0
            || profile.status == ModelRuntimeStatus::ShippedLocal
            || !adapter_matches_backend
            || !profile.installed_models.is_empty()
        {
            return Err("model runtime manifest contains a malformed profile".into());
        }
        if !identities.insert((profile.backend, profile.compatibility_key.clone())) {
            return Err("model runtime manifest contains a duplicate compatibility group".into());
        }
        if profile.status == ModelRuntimeStatus::Qualified
            && !profile.is_qualified_for_distributed()
        {
            return Err(
                "qualified model runtime profile lacks exact runtime/campaign digests or distributed parallelism"
                    .into(),
            );
        }
        profiles.push(profile);
    }
    Ok(profiles)
}

fn resource_bytes(resources: &[ResourceQuantityV1], class: ResourceClass) -> u64 {
    resources
        .iter()
        .find(|resource| resource.class == class && resource.unit == "byte")
        .map_or(0, |resource| resource.available)
}

fn ollama_version(base_url: &str) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(750))
        .build()
        .ok()?;
    let payload = client
        .get(format!("{base_url}/api/version"))
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json::<serde_json::Value>()
        .ok()?;
    payload
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn discover_nvidia_gpus() -> Vec<NvidiaGpu> {
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,uuid,memory.total,memory.free,utilization.gpu,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            parse_nvidia_smi(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Vec::new(),
    }
}

fn parse_nvidia_smi(output: &str) -> Vec<NvidiaGpu> {
    output
        .lines()
        .filter_map(|line| {
            let columns = line.split(',').map(str::trim).collect::<Vec<_>>();
            if columns.len() != 6 {
                return None;
            }
            Some(NvidiaGpu {
                name: columns[0].to_string(),
                uuid: columns[1].to_string(),
                memory_total_mib: columns[2].parse().ok()?,
                memory_free_mib: columns[3].parse().ok()?,
                utilization_percent: columns[4].parse().ok()?,
                temperature_c: columns[5].parse().ok()?,
            })
        })
        .collect()
}

fn discover_storage(labels: &BTreeMap<String, String>, data_dir: &Path) -> Vec<ResourceQuantityV1> {
    let disks = Disks::new_with_refreshed_list();
    let selected = disks
        .iter()
        .filter(|disk| data_dir.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .or_else(|| disks.iter().max_by_key(|disk| disk.available_space()));
    let Some(disk) = selected else {
        return Vec::new();
    };
    let configured_gib = std::env::var("RAMPAGE_STORAGE_CONTRIBUTION_GB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10)
        .min(1024);
    let budget = disk
        .available_space()
        .min(configured_gib.saturating_mul(GIB));
    if budget < 512 * 1024 * 1024 {
        return Vec::new();
    }
    let mut storage_labels = labels.clone();
    storage_labels.insert(
        "storage_root".into(),
        disk.mount_point().to_string_lossy().into_owned(),
    );
    storage_labels.insert(
        "filesystem".into(),
        disk.file_system().to_string_lossy().into_owned(),
    );
    let protected = std::env::var("RAMPAGE_ALLOW_PROTECTED_STORAGE")
        .is_ok_and(|value| value.eq_ignore_ascii_case("true"));
    let (cache, scratch, protected_bytes) = if protected {
        (budget / 2, budget * 3 / 10, budget / 5)
    } else {
        (budget * 3 / 5, budget * 2 / 5, 0)
    };
    let mut resources = vec![
        storage_resource(ResourceClass::StorageCache, cache, &storage_labels),
        storage_resource(ResourceClass::StorageScratch, scratch, &storage_labels),
    ];
    if protected_bytes > 0 {
        resources.push(storage_resource(
            ResourceClass::ProtectedStore,
            protected_bytes,
            &storage_labels,
        ));
    }
    resources
}

fn storage_resource(
    class: ResourceClass,
    bytes: u64,
    labels: &BTreeMap<String, String>,
) -> ResourceQuantityV1 {
    ResourceQuantityV1 {
        class,
        capacity: bytes,
        available: bytes,
        unit: "byte".into(),
        labels: labels.clone(),
    }
}

#[cfg(windows)]
fn power_status() -> (bool, Option<u8>) {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) }.is_ok() {
        let battery = (status.BatteryLifePercent <= 100).then_some(status.BatteryLifePercent);
        (status.ACLineStatus == 1, battery)
    } else {
        (true, None)
    }
}

#[cfg(not(windows))]
fn power_status() -> (bool, Option<u8>) {
    (true, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_inventory_without_guessing_missing_rows() {
        let parsed = parse_nvidia_smi(
            "NVIDIA RTX 4090, GPU-a, 24564, 20100, 17, 54\ninvalid,row\nNVIDIA RTX 3060, GPU-b, 12288, 8000, 40, 66",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].memory_free_mib, 20_100);
        assert_eq!(parsed[1].utilization_percent, 40);
    }

    #[test]
    fn parses_bounded_ollama_inventory_with_content_digests() {
        let models = parse_ollama_models(
            serde_json::json!({"models": [{
                "name": "gemma3:4b",
                "model": "gemma3:4b",
                "size": 3_338_801_804_u64,
                "digest": "a2af6cc3eb7fa8be8504abaf9b04e88f17a119ec3f04a3addf55f92841195f5a",
                "details": {"format": "gguf"}
            }]})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model_id, "gemma3:4b");
        assert_eq!(
            models[0].artifact_digest,
            "sha256:a2af6cc3eb7fa8be8504abaf9b04e88f17a119ec3f04a3addf55f92841195f5a"
        );
    }

    #[test]
    fn rejects_duplicate_ollama_aliases() {
        let payload = serde_json::json!({"models": [
            {"name": "same", "model": "same", "size": 1, "digest": "a".repeat(64)},
            {"name": "same", "model": "same", "size": 1, "digest": "b".repeat(64)}
        ]});
        assert!(parse_ollama_models(payload.to_string().as_bytes()).is_err());
    }

    #[test]
    fn excludes_ollama_cloud_aliases_from_local_execution_inventory() {
        let payload = serde_json::json!({"models": [
            {"name": "remote:cloud", "model": "remote:cloud", "size": 1, "digest": "a".repeat(64)},
            {"name": "local:latest", "model": "local:latest", "size": 2, "digest": "b".repeat(64)}
        ]});
        let models = parse_ollama_models(payload.to_string().as_bytes()).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model_id, "local:latest");
    }

    #[test]
    fn advertises_only_shipped_operation_exact_workload_capabilities() {
        let adapters = BTreeSet::from([
            "rampage.hash.v1".into(),
            "rampage.eval-shard.v1".into(),
            "unknown.adapter".into(),
        ]);
        let capabilities = discover_workload_capabilities(&adapters, &[]);
        assert_eq!(capabilities.len(), 2);
        assert!(capabilities[0].is_valid());
        assert!(
            capabilities
                .iter()
                .any(|capability| capability.authorizes("rampage.eval-shard.v1", "score"))
        );
        assert!(
            !capabilities
                .iter()
                .any(|capability| capability.adapter == "unknown.adapter")
        );
    }
}
