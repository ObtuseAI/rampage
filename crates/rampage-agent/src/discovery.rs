use rampage_protocol::{
    ModelBackend, ModelMemoryKind, ModelParallelism, ModelRuntimeOfferV1, ModelRuntimeStatus,
    ResourceClass, ResourceQuantityV1,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
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

pub fn ollama_available() -> bool {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(750))
        .build()
        .and_then(|client| client.get("http://127.0.0.1:11434/api/tags").send())
        .is_ok_and(|response| response.status().is_success())
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
) -> Result<Vec<ModelRuntimeOfferV1>, String> {
    let mut profiles = Vec::new();
    if has_ollama {
        let gpu_bytes = resource_bytes(resources, ResourceClass::GpuMemory);
        let ram_bytes = resource_bytes(resources, ResourceClass::RamWorkingSet);
        let (memory_kind, available_model_bytes) = if gpu_bytes > 0 {
            (ModelMemoryKind::DedicatedGpu, gpu_bytes)
        } else {
            (ModelMemoryKind::Host, ram_bytes)
        };
        if available_model_bytes > 0 {
            let runtime_version = ollama_version().unwrap_or_else(|| "detected-local".into());
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

fn ollama_version() -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(750))
        .build()
        .ok()?;
    let payload = client
        .get("http://127.0.0.1:11434/api/version")
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
}
