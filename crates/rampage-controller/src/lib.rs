//! OnePool admission and deterministic scheduling.

use chrono::{DateTime, Utc};
use rampage_protocol::{
    BreakEvenDecisionV1, BreakEvenPlanV1, BreakEvenRequestV1, ComputeStrategy,
    FabricBenchmarkResultV1, JobSpecV1, ModelBackend, ModelMemoryKind, ModelParallelism,
    ModelRuntimeOfferV1, ModelRuntimeStatus, ModelSessionRequestV1, NetworkAutopilotStatusV1,
    NetworkNodeAutopilotV1, NetworkPathKindV1, ResourceClass, ResourceOfferV1, ShardSetV1,
    TrafficAdmissionV1, TrafficClassV1, WorkloadClassV1,
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, HashMap},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementScore {
    pub node_id: Uuid,
    pub satisfies: bool,
    pub data_local_inputs: u32,
    pub missing_input_bytes: u64,
    pub capacity_headroom: u64,
    pub capacity_fit_milli: u64,
    pub link_rtt_micros: u64,
    pub link_downlink_bps: u64,
    pub estimated_transfer_millis: u64,
    pub utility_milli: i64,
    pub topology_confidence: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct AdmissionPolicy {
    pub protected_owner_reserve: BTreeMap<ResourceClass, u64>,
    pub safety_guardband_percent: u8,
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        Self {
            protected_owner_reserve: BTreeMap::from([
                (ResourceClass::CpuCompute, 1),
                (ResourceClass::RamWorkingSet, 512 * 1024 * 1024),
            ]),
            safety_guardband_percent: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceReservation {
    pub job_id: Uuid,
    pub node_id: Uuid,
    pub class: ResourceClass,
    pub amount: u64,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardPlacement {
    pub job_id: Uuid,
    pub node_id: Uuid,
    pub score: PlacementScore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardPlanningFailure {
    pub blocked_job_id: Uuid,
    pub planned_shards: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPlanState {
    Ready,
    QualificationRequired,
    CapacityBlocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelNodePlacement {
    pub node_id: Uuid,
    pub rank: u16,
    pub assigned_bytes: u64,
    pub available_model_bytes: u64,
    pub role: String,
    pub topology_confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSessionPlan {
    pub schema: String,
    pub session_id: Uuid,
    pub strategy: ComputeStrategy,
    pub state: ModelPlanState,
    pub backend: Option<ModelBackend>,
    pub parallelism: Option<ModelParallelism>,
    pub distributed: bool,
    pub required_bytes: u64,
    pub observed_fabric_bytes: u64,
    pub maximum_supported_bytes: u64,
    pub predicted_speedup_milli: u64,
    pub placements: Vec<ModelNodePlacement>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub proposed_local_endpoint: Option<String>,
    pub execution_authority: String,
    pub reason: String,
}

impl ModelSessionPlan {
    pub const SCHEMA: &'static str = "rampage.model-session-plan.v1";
}

#[derive(Clone, Copy)]
struct RuntimeCandidate<'a> {
    offer: &'a ResourceOfferV1,
    runtime: &'a ModelRuntimeOfferV1,
    available_bytes: u64,
}

fn resource_available(offer: &ResourceOfferV1, class: ResourceClass) -> u64 {
    offer
        .resources
        .iter()
        .find(|resource| resource.class == class && resource.unit == "byte")
        .map_or(0, |resource| resource.available)
}

fn observed_model_memory(offer: &ResourceOfferV1) -> u64 {
    resource_available(offer, ResourceClass::GpuMemory)
        .max(resource_available(offer, ResourceClass::RamWorkingSet))
}

fn runtime_capacity(offer: &ResourceOfferV1, runtime: &ModelRuntimeOfferV1) -> u64 {
    let observed = match runtime.memory_kind {
        ModelMemoryKind::DedicatedGpu => resource_available(offer, ResourceClass::GpuMemory),
        ModelMemoryKind::Unified | ModelMemoryKind::Host => {
            resource_available(offer, ResourceClass::RamWorkingSet)
        }
        ModelMemoryKind::Hybrid => resource_available(offer, ResourceClass::GpuMemory)
            .saturating_add(resource_available(offer, ResourceClass::RamWorkingSet)),
    };
    runtime.available_model_bytes.min(observed)
}

fn topology_confidence(offer: &ResourceOfferV1) -> &'static str {
    if offer.mesh_endpoint.is_none() {
        "controller_local"
    } else if offer.link_benchmark.is_some() {
        "measured"
    } else {
        "unmeasured"
    }
}

fn topology_ready(candidate: RuntimeCandidate<'_>) -> bool {
    candidate.offer.mesh_endpoint.is_none()
        || candidate.offer.link_benchmark.as_ref().is_some_and(|link| {
            link.rtt_micros_p50 <= 25_000
                && link.uplink_bps >= 250_000_000
                && link.downlink_bps >= 250_000_000
        })
}

fn speed_topology_ready(candidate: RuntimeCandidate<'_>) -> bool {
    candidate.offer.mesh_endpoint.is_none()
        || candidate.offer.link_benchmark.as_ref().is_some_and(|link| {
            link.rtt_micros_p50 <= 5_000
                && link.uplink_bps >= 1_000_000_000
                && link.downlink_bps >= 1_000_000_000
        })
}

fn predicted_tensor_speedup_milli(candidates: &[RuntimeCandidate<'_>]) -> u64 {
    if candidates.len() < 2 || !candidates.iter().copied().all(speed_topology_ready) {
        return 1_000;
    }
    let worst_rtt = candidates
        .iter()
        .filter_map(|candidate| candidate.offer.link_benchmark.as_ref())
        .map(|link| link.rtt_micros_p50)
        .max()
        .unwrap_or(0);
    let slowest_link = candidates
        .iter()
        .filter_map(|candidate| candidate.offer.link_benchmark.as_ref())
        .map(|link| link.uplink_bps.min(link.downlink_bps))
        .min()
        .unwrap_or(u64::MAX);
    let efficiency_milli = if worst_rtt <= 1_000 && slowest_link >= 10_000_000_000 {
        850
    } else if worst_rtt <= 2_500 && slowest_link >= 2_500_000_000 {
        650
    } else {
        400
    };
    (candidates.len() as u64)
        .saturating_mul(efficiency_milli)
        .max(1_000)
}

fn allocate_pipeline(
    candidates: &[RuntimeCandidate<'_>],
    required_bytes: u64,
) -> Vec<ModelNodePlacement> {
    let total = candidates
        .iter()
        .map(|candidate| candidate.available_bytes)
        .sum::<u64>();
    let mut remaining = required_bytes;
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let assigned = if index + 1 == candidates.len() {
                remaining.min(candidate.available_bytes)
            } else {
                let proportional = ((required_bytes as u128)
                    .saturating_mul(candidate.available_bytes as u128)
                    / total.max(1) as u128) as u64;
                proportional.min(candidate.available_bytes).min(remaining)
            };
            remaining = remaining.saturating_sub(assigned);
            ModelNodePlacement {
                node_id: candidate.offer.node_id,
                rank: index as u16,
                assigned_bytes: assigned,
                available_model_bytes: candidate.available_bytes,
                role: if index == 0 {
                    "coordinator_rank".into()
                } else {
                    "model_rank".into()
                },
                topology_confidence: topology_confidence(candidate.offer).into(),
            }
        })
        .collect()
}

/// Build a read-only model placement preview. This function never issues a lease or starts a
/// backend. Distributed readiness requires an exact, qualified runtime offer on every selected
/// node plus measured controller links; backend launch remains a separate Governor action.
pub fn plan_model_session(
    request: &ModelSessionRequestV1,
    offers: &[ResourceOfferV1],
    now: DateTime<Utc>,
) -> ModelSessionPlan {
    let required_bytes = request.required_bytes();
    let live_offers = offers
        .iter()
        .filter(|offer| {
            offer.expires_at > now
                && offer.availability.foreground_allowed
                && offer.availability.thermal_headroom_percent >= 15
                && (offer.availability.on_ac_power
                    || offer.availability.battery_percent.unwrap_or(100) >= 50)
        })
        .collect::<Vec<_>>();
    let observed_fabric_bytes = live_offers
        .iter()
        .map(|offer| observed_model_memory(offer))
        .sum();
    let mut groups: BTreeMap<(ModelBackend, String), Vec<RuntimeCandidate<'_>>> = BTreeMap::new();
    for offer in &live_offers {
        for runtime in &offer.model_runtimes {
            if runtime.schema != ModelRuntimeOfferV1::SCHEMA
                || runtime.available_model_bytes == 0
                || !offer.adapters.contains(&runtime.adapter)
            {
                continue;
            }
            let available_bytes = runtime_capacity(offer, runtime);
            if available_bytes == 0 {
                continue;
            }
            groups
                .entry((runtime.backend, runtime.compatibility_key.clone()))
                .or_default()
                .push(RuntimeCandidate {
                    offer,
                    runtime,
                    available_bytes,
                });
        }
    }
    for candidates in groups.values_mut() {
        candidates.sort_by_key(|candidate| Reverse(candidate.available_bytes));
        candidates.truncate(request.max_nodes as usize);
    }

    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    if groups.is_empty() {
        blockers.push(
            "No signed model runtime profiles are advertised; install or qualify a governed backend bundle."
                .into(),
        );
        if observed_fabric_bytes > 0 {
            warnings.push(
                "Raw RAM/VRAM is visible, but memory is not combinable without one compatible distributed runtime."
                    .into(),
            );
        }
        return ModelSessionPlan {
            schema: ModelSessionPlan::SCHEMA.into(),
            session_id: request.session_id,
            strategy: request.strategy,
            state: ModelPlanState::CapacityBlocked,
            backend: None,
            parallelism: None,
            distributed: false,
            required_bytes,
            observed_fabric_bytes,
            maximum_supported_bytes: 0,
            predicted_speedup_milli: 1_000,
            placements: Vec::new(),
            blockers,
            warnings,
            proposed_local_endpoint: None,
            execution_authority: "none_preview_only".into(),
            reason: "The fabric has resources but no compatible model runtime group.".into(),
        };
    }

    let mut ranked = groups.into_iter().collect::<Vec<_>>();
    ranked.sort_by_key(|(_, candidates)| {
        Reverse(
            candidates
                .iter()
                .map(|candidate| candidate.available_bytes)
                .sum::<u64>(),
        )
    });

    let (backend_key, mut selected, parallelism, predicted_speedup_milli) = match request.strategy {
        ComputeStrategy::MaximumModelSize => {
            let (key, candidates) = ranked.remove(0);
            let parallelism = if candidates.len() > 1
                && candidates.iter().all(|candidate| {
                    candidate
                        .runtime
                        .supported_parallelism
                        .contains(&ModelParallelism::Pipeline)
                }) {
                ModelParallelism::Pipeline
            } else {
                ModelParallelism::WholeModel
            };
            (key, candidates, parallelism, 1_000)
        }
        ComputeStrategy::SpeedBoost => {
            let mut best_single: Option<((ModelBackend, String), RuntimeCandidate<'_>)> = None;
            let mut best_distributed: Option<(
                (ModelBackend, String),
                Vec<RuntimeCandidate<'_>>,
                u64,
            )> = None;
            for (key, candidates) in &ranked {
                if let Some(candidate) = candidates
                    .iter()
                    .copied()
                    .find(|candidate| candidate.available_bytes >= required_bytes)
                    && best_single.as_ref().is_none_or(|(_, current)| {
                        candidate.available_bytes > current.available_bytes
                    })
                {
                    best_single = Some((key.clone(), candidate));
                }
                if candidates.len() > 1
                    && candidates.iter().all(|candidate| {
                        candidate
                            .runtime
                            .supported_parallelism
                            .contains(&ModelParallelism::Tensor)
                            && candidate.runtime.is_qualified_for_distributed()
                    })
                {
                    let speedup = predicted_tensor_speedup_milli(candidates);
                    let shard_bytes = required_bytes.div_ceil(candidates.len() as u64);
                    if speedup > 1_100
                        && candidates
                            .iter()
                            .all(|candidate| candidate.available_bytes >= shard_bytes)
                        && best_distributed
                            .as_ref()
                            .is_none_or(|(_, _, current)| speedup > *current)
                    {
                        best_distributed = Some((key.clone(), candidates.clone(), speedup));
                    }
                }
            }
            if let Some((key, candidates, speedup)) = best_distributed {
                (key, candidates, ModelParallelism::Tensor, speedup)
            } else if let Some((key, candidate)) = best_single {
                (key, vec![candidate], ModelParallelism::WholeModel, 1_000)
            } else {
                let (key, candidates) = ranked.remove(0);
                (key, candidates, ModelParallelism::Pipeline, 1_000)
            }
        }
        ComputeStrategy::MaximumThroughput => {
            let mut best = ranked
                .iter()
                .map(|(key, candidates)| {
                    (
                        key.clone(),
                        candidates
                            .iter()
                            .copied()
                            .filter(|candidate| candidate.available_bytes >= required_bytes)
                            .collect::<Vec<_>>(),
                    )
                })
                .max_by_key(|(_, replicas)| replicas.len())
                .unwrap_or_else(|| ranked.remove(0));
            if best.1.is_empty() {
                best = ranked.remove(0);
            }
            let speedup = best.1.len().max(1) as u64 * 1_000;
            (best.0, best.1, ModelParallelism::Replica, speedup)
        }
        ComputeStrategy::Efficiency => {
            let (key, candidate) = ranked
                .iter()
                .flat_map(|(key, candidates)| {
                    candidates
                        .iter()
                        .copied()
                        .filter(|candidate| candidate.available_bytes >= required_bytes)
                        .map(|candidate| (key.clone(), candidate))
                })
                .min_by_key(|(_, candidate)| candidate.available_bytes)
                .unwrap_or_else(|| (ranked[0].0.clone(), ranked[0].1[0]));
            (key, vec![candidate], ModelParallelism::WholeModel, 1_000)
        }
        ComputeStrategy::AutonomousBalanced => {
            if let Some((key, candidate)) = ranked.iter().find_map(|(key, candidates)| {
                candidates
                    .iter()
                    .copied()
                    .find(|candidate| candidate.available_bytes >= required_bytes)
                    .map(|candidate| (key.clone(), candidate))
            }) {
                (key, vec![candidate], ModelParallelism::WholeModel, 1_000)
            } else {
                let (key, candidates) = ranked.remove(0);
                (key, candidates, ModelParallelism::Pipeline, 1_000)
            }
        }
    };

    let maximum_supported_bytes = match parallelism {
        ModelParallelism::Replica => selected
            .iter()
            .map(|candidate| candidate.available_bytes)
            .max()
            .unwrap_or(0),
        _ => selected
            .iter()
            .map(|candidate| candidate.available_bytes)
            .sum(),
    };
    let capacity_fits = match parallelism {
        ModelParallelism::Replica | ModelParallelism::WholeModel => selected
            .iter()
            .any(|candidate| candidate.available_bytes >= required_bytes),
        ModelParallelism::Tensor => {
            let shard_bytes = required_bytes.div_ceil(selected.len().max(1) as u64);
            selected
                .iter()
                .all(|candidate| candidate.available_bytes >= shard_bytes)
        }
        _ => maximum_supported_bytes >= required_bytes,
    };
    let distributed = selected.len() > 1 && parallelism != ModelParallelism::Replica;
    let qualified = if distributed {
        selected.iter().all(|candidate| {
            candidate.runtime.is_qualified_for_distributed() && topology_ready(*candidate)
        })
    } else {
        selected.iter().all(|candidate| {
            matches!(
                candidate.runtime.status,
                ModelRuntimeStatus::ShippedLocal | ModelRuntimeStatus::Qualified
            )
        })
    };
    if !capacity_fits {
        blockers.push(format!(
            "The best compatible runtime group exposes {} bytes, below the requested {} bytes.",
            maximum_supported_bytes, required_bytes
        ));
    }
    if distributed && !qualified {
        blockers.push(
            "Distributed launch is fenced until every selected runtime and measured topology has a valid qualification digest."
                .into(),
        );
    }
    if request.strategy == ComputeStrategy::SpeedBoost && predicted_speedup_milli == 1_000 {
        warnings.push(
            "No distributed topology cleared the speedup gate; the plan avoids claiming that more nodes are faster."
                .into(),
        );
    }
    if parallelism == ModelParallelism::Replica {
        warnings.push(
            "Replica placement increases concurrent throughput; it does not make one model request faster or larger."
                .into(),
        );
    }

    if parallelism == ModelParallelism::WholeModel {
        selected.truncate(1);
    }
    let placements = if parallelism == ModelParallelism::Replica {
        selected
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.available_bytes >= required_bytes)
            .map(|(index, candidate)| ModelNodePlacement {
                node_id: candidate.offer.node_id,
                rank: index as u16,
                assigned_bytes: required_bytes,
                available_model_bytes: candidate.available_bytes,
                role: "replica".into(),
                topology_confidence: topology_confidence(candidate.offer).into(),
            })
            .collect()
    } else if parallelism == ModelParallelism::Tensor {
        let shard_bytes = required_bytes.div_ceil(selected.len().max(1) as u64);
        selected
            .iter()
            .enumerate()
            .map(|(index, candidate)| ModelNodePlacement {
                node_id: candidate.offer.node_id,
                rank: index as u16,
                assigned_bytes: shard_bytes,
                available_model_bytes: candidate.available_bytes,
                role: if index == 0 {
                    "coordinator_rank".into()
                } else {
                    "tensor_rank".into()
                },
                topology_confidence: topology_confidence(candidate.offer).into(),
            })
            .collect()
    } else {
        allocate_pipeline(&selected, required_bytes)
    };
    let state = if !capacity_fits {
        ModelPlanState::CapacityBlocked
    } else if qualified {
        ModelPlanState::Ready
    } else {
        ModelPlanState::QualificationRequired
    };
    let reason = match (request.strategy, state) {
        (_, ModelPlanState::CapacityBlocked) => {
            "No compatible placement can fit the requested model and KV cache.".into()
        }
        (ComputeStrategy::MaximumModelSize, ModelPlanState::Ready) => {
            "Largest compatible qualified memory group selected for model fit.".into()
        }
        (ComputeStrategy::SpeedBoost, ModelPlanState::Ready) if distributed => format!(
            "Low-latency tensor group passed the conservative {:.2}x speedup gate.",
            predicted_speedup_milli as f64 / 1000.0
        ),
        (ComputeStrategy::SpeedBoost, ModelPlanState::Ready) => {
            "Fastest safe whole-model node selected; slower links were excluded.".into()
        }
        (ComputeStrategy::MaximumThroughput, ModelPlanState::Ready) => {
            "Independent whole-model replicas selected for concurrent request throughput.".into()
        }
        (ComputeStrategy::Efficiency, ModelPlanState::Ready) => {
            "Smallest qualified whole-model placement that fits was selected.".into()
        }
        (ComputeStrategy::AutonomousBalanced, ModelPlanState::Ready) => {
            "A qualified whole-model placement was preferred before distributed complexity.".into()
        }
        (_, ModelPlanState::QualificationRequired) => {
            "The capacity fits in theory, but backend or topology evidence is incomplete.".into()
        }
    };
    ModelSessionPlan {
        schema: ModelSessionPlan::SCHEMA.into(),
        session_id: request.session_id,
        strategy: request.strategy,
        state,
        backend: Some(backend_key.0),
        parallelism: Some(parallelism),
        distributed,
        required_bytes,
        observed_fabric_bytes,
        maximum_supported_bytes,
        predicted_speedup_milli,
        placements,
        blockers,
        warnings,
        proposed_local_endpoint: None,
        execution_authority: "none_preview_only".into(),
        reason,
    }
}

fn denied_score(node_id: Uuid, reason: String) -> PlacementScore {
    PlacementScore {
        node_id,
        satisfies: false,
        data_local_inputs: 0,
        missing_input_bytes: 0,
        capacity_headroom: 0,
        capacity_fit_milli: 0,
        link_rtt_micros: 0,
        link_downlink_bps: 0,
        estimated_transfer_millis: 0,
        utility_milli: i64::MIN,
        topology_confidence: "inadmissible".into(),
        reason,
    }
}

pub fn score_offer(job: &JobSpecV1, offer: &ResourceOfferV1) -> PlacementScore {
    score_offer_with_admission(
        job,
        offer,
        &[],
        &AdmissionPolicy {
            protected_owner_reserve: BTreeMap::new(),
            safety_guardband_percent: 0,
        },
        Utc::now(),
    )
}

pub fn score_offer_with_admission(
    job: &JobSpecV1,
    offer: &ResourceOfferV1,
    reservations: &[ResourceReservation],
    policy: &AdmissionPolicy,
    now: DateTime<Utc>,
) -> PlacementScore {
    if !offer.adapters.contains(&job.adapter) {
        return denied_score(offer.node_id, "required adapter is unavailable".into());
    }
    let resources: BTreeMap<_, _> = offer
        .resources
        .iter()
        .map(|resource| (resource.class as u8, resource))
        .collect();
    let mut headroom = 0_u64;
    let mut capacity_fit_milli = u64::MAX;
    for request in &job.requests {
        let Some(resource) = resources.get(&(request.class as u8)) else {
            return denied_score(
                offer.node_id,
                format!("missing resource {:?}", request.class),
            );
        };
        let labels_match = request
            .required_labels
            .iter()
            .all(|(key, value)| resource.labels.get(key) == Some(value));
        let pending = reservations
            .iter()
            .filter(|reservation| {
                reservation.node_id == offer.node_id
                    && reservation.class == request.class
                    && reservation.expires_at > now
            })
            .map(|reservation| reservation.amount)
            .fold(0_u64, u64::saturating_add);
        let owner_reserve = policy
            .protected_owner_reserve
            .get(&request.class)
            .copied()
            .unwrap_or(0);
        let guardband = resource
            .capacity
            .saturating_mul(policy.safety_guardband_percent.min(100) as u64)
            / 100;
        let admissible = resource
            .available
            .saturating_sub(pending)
            .saturating_sub(owner_reserve)
            .saturating_sub(guardband);
        if !labels_match || admissible < request.minimum || resource.unit != request.unit {
            return denied_score(
                offer.node_id,
                format!("insufficient or incompatible {:?}", request.class),
            );
        }
        headroom = headroom.saturating_add(admissible - request.minimum);
        let preferred = request.preferred.max(request.minimum).max(1);
        let fit = admissible
            .saturating_mul(1_000)
            .checked_div(preferred)
            .unwrap_or(0);
        capacity_fit_milli = capacity_fit_milli.min(fit.min(4_000));
    }
    if capacity_fit_milli == u64::MAX {
        capacity_fit_milli = 1_000;
    }
    PlacementScore {
        node_id: offer.node_id,
        satisfies: true,
        data_local_inputs: 0,
        missing_input_bytes: job.inputs.iter().map(|input| input.size_bytes).sum(),
        capacity_headroom: headroom,
        capacity_fit_milli,
        link_rtt_micros: 0,
        link_downlink_bps: 0,
        estimated_transfer_millis: 0,
        utility_milli: capacity_fit_milli as i64,
        topology_confidence: "unmeasured".into(),
        reason: "whole-workload placement satisfies hard constraints".into(),
    }
}

pub fn score_offer_with_topology(
    job: &JobSpecV1,
    offer: &ResourceOfferV1,
    reservations: &[ResourceReservation],
    policy: &AdmissionPolicy,
    now: DateTime<Utc>,
    local_input_digests: &BTreeSet<String>,
) -> PlacementScore {
    let mut score = score_offer_with_admission(job, offer, reservations, policy, now);
    if !score.satisfies {
        return score;
    }
    score.data_local_inputs = job
        .inputs
        .iter()
        .filter(|input| local_input_digests.contains(&input.digest))
        .count() as u32;
    score.missing_input_bytes = job
        .inputs
        .iter()
        .filter(|input| !local_input_digests.contains(&input.digest))
        .map(|input| input.size_bytes)
        .fold(0_u64, u64::saturating_add);

    let is_controller_local = offer.mesh_endpoint.is_none();
    let (rtt_micros, downlink_bps, confidence) = if is_controller_local {
        (0, u64::MAX, "controller_local")
    } else if let Some(link) = offer.link_benchmark.as_ref().filter(|link| {
        link.schema == rampage_protocol::LinkBenchmarkV1::SCHEMA
            && link.expires_at > now
            && link.downlink_bps > 0
    }) {
        (link.rtt_micros_p50, link.downlink_bps, "measured")
    } else {
        // Missing measurements never become optimistic evidence. Twenty megabits and fifty
        // milliseconds are a deliberately conservative fallback for an unknown trusted-circle link.
        (50_000, 20_000_000, "conservative_fallback")
    };
    score.link_rtt_micros = rtt_micros;
    score.link_downlink_bps = downlink_bps;
    score.topology_confidence = confidence.into();
    score.estimated_transfer_millis = if score.missing_input_bytes == 0 {
        0
    } else {
        score
            .missing_input_bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .div_ceil(downlink_bps.max(1))
            .saturating_add(rtt_micros.div_ceil(1_000))
    };
    let transfer_penalty = score.estimated_transfer_millis.div_ceil(10).min(10_000) as i64;
    score.utility_milli = score.capacity_fit_milli as i64 - transfer_penalty;
    score.reason = format!(
        "admissible; {} of {} inputs local; estimated staging {} ms; topology {}",
        score.data_local_inputs,
        job.inputs.len(),
        score.estimated_transfer_millis,
        score.topology_confidence
    );
    score
}

fn placement_rank(score: &PlacementScore) -> (i64, u32, u64, Reverse<u64>, u64) {
    (
        score.utility_milli,
        score.data_local_inputs,
        score.capacity_fit_milli,
        Reverse(score.estimated_transfer_millis),
        score.capacity_headroom,
    )
}

pub fn choose_offer_with_admission<'a>(
    job: &JobSpecV1,
    offers: &'a [ResourceOfferV1],
    reservations: &[ResourceReservation],
    policy: &AdmissionPolicy,
    now: DateTime<Utc>,
) -> Option<(&'a ResourceOfferV1, PlacementScore)> {
    offers
        .iter()
        .map(|offer| {
            (
                offer,
                score_offer_with_admission(job, offer, reservations, policy, now),
            )
        })
        .filter(|(_, score)| score.satisfies)
        .max_by_key(|(_, score)| placement_rank(score))
}

pub fn choose_offer_with_topology<'a>(
    job: &JobSpecV1,
    offers: &'a [ResourceOfferV1],
    reservations: &[ResourceReservation],
    policy: &AdmissionPolicy,
    now: DateTime<Utc>,
    local_inputs_by_node: &HashMap<Uuid, BTreeSet<String>>,
) -> Option<(&'a ResourceOfferV1, PlacementScore)> {
    offers
        .iter()
        .map(|offer| {
            let local = local_inputs_by_node
                .get(&offer.node_id)
                .cloned()
                .unwrap_or_default();
            (
                offer,
                score_offer_with_topology(job, offer, reservations, policy, now, &local),
            )
        })
        .filter(|(_, score)| score.satisfies)
        .max_by_key(|(_, score)| placement_rank(score))
}

pub fn choose_offer<'a>(
    job: &JobSpecV1,
    offers: &'a [ResourceOfferV1],
) -> Option<(&'a ResourceOfferV1, PlacementScore)> {
    offers
        .iter()
        .map(|offer| (offer, score_offer(job, offer)))
        .filter(|(_, score)| score.satisfies)
        .max_by_key(|(_, score)| placement_rank(score))
}

/// Decide whether measured fabric throughput is likely to repay its distribution overhead.
///
/// This planner never issues authority and never substitutes synthetic capacity for evidence. It
/// requires a recent, internally consistent dividend plus fresh signed offers and link benchmarks
/// for every participating remote node. The estimate is intentionally pessimistic: complete input
/// and output sizes are charged to every remote participant and p90 safety factors are applied to
/// compute, startup, transfer, and retry cost.
pub fn plan_break_even(
    request: &BreakEvenRequestV1,
    dividend: Option<(&FabricBenchmarkResultV1, DateTime<Utc>)>,
    offers: &[ResourceOfferV1],
    now: DateTime<Utc>,
) -> BreakEvenPlanV1 {
    let required_gain = request
        .minimum_gain_percent
        .max(default_minimum_gain(request.workload_class));
    let mut plan = BreakEvenPlanV1 {
        schema: BreakEvenPlanV1::SCHEMA.into(),
        decision: BreakEvenDecisionV1::InsufficientEvidence,
        workload_class: request.workload_class,
        baseline_node_id: None,
        selected_node_ids: Vec::new(),
        p90_baseline_ms: conservative_baseline_ms(request),
        p90_fabric_ms: None,
        estimated_gain_percent: None,
        required_gain_percent: required_gain,
        evidence_set_id: None,
        evidence_age_seconds: None,
        topology_confidence: "insufficient".into(),
        reason:
            "No recent signed Compute Dividend is available; run the sustained benchmark first."
                .into(),
        claim_boundary: BreakEvenPlanV1::CLAIM_BOUNDARY.into(),
    };
    if !request.is_valid() {
        plan.reason = "The break-even request is outside its bounded planning contract.".into();
        return plan;
    }
    if request.workload_class == WorkloadClassV1::ArtifactMovement {
        plan.reason = "Artifact movement is network-bound and cannot use a CPU dividend as performance evidence."
            .into();
        return plan;
    }
    if !request.restart_tolerant {
        plan.reason =
            "This workload is not restart tolerant, so automatic multi-node execution is fenced."
                .into();
        return plan;
    }
    let Some((dividend, recorded_at)) = dividend else {
        return plan;
    };
    if !dividend.is_internally_consistent() {
        plan.reason = "The latest Compute Dividend failed contract validation.".into();
        return plan;
    }
    let evidence_age = now.signed_duration_since(recorded_at).num_seconds().max(0) as u64;
    plan.evidence_set_id = Some(dividend.set_id);
    plan.evidence_age_seconds = Some(evidence_age);
    if evidence_age > 24 * 60 * 60 {
        plan.reason = "The latest Compute Dividend is older than 24 hours; remeasure before changing placement."
            .into();
        return plan;
    }
    let Some(baseline) = dividend
        .nodes
        .iter()
        .max_by_key(|node| node.hashes_per_second)
    else {
        return plan;
    };
    plan.baseline_node_id = Some(baseline.node_id);
    if dividend.nodes.len() < 2 {
        plan.decision = BreakEvenDecisionV1::StayOnFastestNode;
        plan.selected_node_ids = vec![baseline.node_id];
        plan.topology_confidence = "measured_single_node".into();
        plan.reason =
            "Only one node contributed verified work, so there is no measured fabric gain to use."
                .into();
        return plan;
    }

    let offers_by_node = offers
        .iter()
        .filter(|offer| offer.expires_at > now)
        .map(|offer| (offer.node_id, offer))
        .collect::<HashMap<_, _>>();
    let mut max_transfer_ms = 0_u64;
    for node in &dividend.nodes {
        let Some(offer) = offers_by_node.get(&node.node_id) else {
            plan.reason = format!(
                "{} no longer has a fresh signed resource offer; the measured topology changed.",
                node.name
            );
            return plan;
        };
        if offer.mesh_endpoint.is_none() {
            continue;
        }
        let Some(link) = offer.link_benchmark.as_ref().filter(|link| {
            link.expires_at > now
                && link.rtt_micros_p50 > 0
                && link.uplink_bps > 0
                && link.downlink_bps > 0
        }) else {
            plan.reason = format!(
                "{} lacks a fresh signed link benchmark; unknown topology cannot justify distribution.",
                node.name
            );
            return plan;
        };
        let input_ms = transfer_millis(request.input_bytes, link.downlink_bps);
        let output_ms = transfer_millis(request.output_bytes, link.uplink_bps);
        let round_trips_ms = link.rtt_micros_p50.div_ceil(1_000).saturating_mul(2);
        max_transfer_ms = max_transfer_ms.max(
            input_ms
                .saturating_add(output_ms)
                .saturating_add(round_trips_ms),
        );
    }

    let (compute_factor, startup_factor, network_factor, retry_factor) =
        p90_factors(request.workload_class);
    let distributed_compute_ms =
        request.fastest_node_compute_ms as f64 / dividend.effective_scale_over_fastest_node;
    let before_retry = distributed_compute_ms * compute_factor
        + request.startup_ms as f64 * startup_factor
        + max_transfer_ms as f64 * network_factor;
    let p90_fabric_ms = (before_retry * (1.0 + retry_factor)).ceil().max(1.0) as u64;
    let gain = (1.0 - p90_fabric_ms as f64 / plan.p90_baseline_ms.max(1) as f64) * 100.0;
    plan.p90_fabric_ms = Some(p90_fabric_ms);
    plan.estimated_gain_percent = Some(gain);
    plan.topology_confidence = "fresh_signed_compute_and_link_evidence".into();
    if gain >= required_gain && p90_fabric_ms < plan.p90_baseline_ms {
        plan.decision = BreakEvenDecisionV1::UseFabric;
        plan.selected_node_ids = dividend.nodes.iter().map(|node| node.node_id).collect();
        plan.reason = format!(
            "Conservative p90 clears the {:.1}% gain threshold with {:.1}% projected headroom.",
            required_gain, gain
        );
    } else {
        plan.decision = BreakEvenDecisionV1::StayOnFastestNode;
        plan.selected_node_ids = vec![baseline.node_id];
        plan.reason = format!(
            "Distribution projects {:.1}% gain, below the {:.1}% safety threshold; stay on the fastest node.",
            gain, required_gain
        );
    }
    plan
}

fn conservative_baseline_ms(request: &BreakEvenRequestV1) -> u64 {
    (request.fastest_node_compute_ms as f64 * 1.2 + request.startup_ms as f64 * 1.1)
        .ceil()
        .max(1.0) as u64
}

fn transfer_millis(bytes: u64, bits_per_second: u64) -> u64 {
    bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .div_ceil(bits_per_second.max(1))
}

fn default_minimum_gain(class: WorkloadClassV1) -> f64 {
    match class {
        WorkloadClassV1::InteractiveAi => 20.0,
        WorkloadClassV1::BatchAi => 10.0,
        WorkloadClassV1::BuildTest => 12.0,
        WorkloadClassV1::RenderTranscode => 8.0,
        WorkloadClassV1::ArtifactMovement => 5.0,
    }
}

fn p90_factors(class: WorkloadClassV1) -> (f64, f64, f64, f64) {
    match class {
        WorkloadClassV1::InteractiveAi => (1.25, 1.6, 2.0, 0.10),
        WorkloadClassV1::BatchAi => (1.2, 1.4, 1.6, 0.08),
        WorkloadClassV1::BuildTest => (1.2, 1.35, 1.5, 0.08),
        WorkloadClassV1::RenderTranscode => (1.2, 1.3, 1.4, 0.06),
        WorkloadClassV1::ArtifactMovement => (1.2, 1.3, 1.4, 0.06),
    }
}

/// Project the safest usable network path and traffic classes from current signed offers.
///
/// A signed endpoint advertisement proves a candidate, not the route currently carrying packets.
/// Consequently this status names `direct_candidate` and `owner_relay_bootstrap` explicitly and
/// only admits performance-sensitive classes after a fresh end-to-end link benchmark.
pub fn network_autopilot_status(
    offers: &[ResourceOfferV1],
    now: DateTime<Utc>,
) -> NetworkAutopilotStatusV1 {
    let mut nodes = offers
        .iter()
        .filter(|offer| offer.expires_at > now)
        .map(|offer| {
            let direct_candidates = offer
                .mesh_endpoint
                .as_ref()
                .map_or(0, |endpoint| endpoint.direct_addresses.len());
            let owner_relays = offer
                .mesh_endpoint
                .as_ref()
                .map_or(0, |endpoint| endpoint.relay_urls.len());
            let link = offer.link_benchmark.as_ref().filter(|link| {
                link.expires_at > now
                    && link.rtt_micros_p50 > 0
                    && link.uplink_bps > 0
                    && link.downlink_bps > 0
            });
            let (preferred_path, evidence) = if offer.mesh_endpoint.is_none() {
                (
                    NetworkPathKindV1::ControllerLocal,
                    "controller-local offer; no network hop required",
                )
            } else if link.is_some_and(|link| {
                link.observed_path == Some(rampage_protocol::ObservedLinkPathV1::Direct)
            }) {
                (
                    NetworkPathKindV1::DirectMeasured,
                    "transport reports an active direct path and the end-to-end benchmark is fresh",
                )
            } else if link.is_some_and(|link| {
                link.observed_path == Some(rampage_protocol::ObservedLinkPathV1::OwnerRelay)
            }) {
                (
                    NetworkPathKindV1::OwnerRelayMeasured,
                    "transport reports an active owner relay path and the end-to-end benchmark is fresh",
                )
            } else if link.is_some() && direct_candidates > 0 {
                (
                    NetworkPathKindV1::DirectCandidate,
                    "fresh signed link benchmark and signed direct candidate",
                )
            } else if owner_relays > 0 {
                (
                    NetworkPathKindV1::OwnerRelayBootstrap,
                    "owner-operated relay retained while direct-path evidence is unavailable",
                )
            } else {
                (
                    NetworkPathKindV1::Recovering,
                    "signed endpoint exists but no fresh measured path or owner relay is available",
                )
            };
            let traffic = [
                TrafficClassV1::AuthorityControl,
                TrafficClassV1::InteractiveAi,
                TrafficClassV1::RemoteMedia,
                TrafficClassV1::Artifact,
                TrafficClassV1::BulkBackground,
            ]
            .into_iter()
            .map(|traffic_class| {
                admit_traffic(
                    traffic_class,
                    preferred_path,
                    link,
                    direct_candidates + owner_relays > 0,
                )
            })
            .collect();
            NetworkNodeAutopilotV1 {
                node_id: offer.node_id,
                preferred_path,
                evidence: evidence.into(),
                direct_candidates,
                owner_relays,
                rtt_millis_p50: link.map(|link| link.rtt_micros_p50 as f64 / 1_000.0),
                uplink_mbps: link.map(|link| link.uplink_bps as f64 / 1_000_000.0),
                downlink_mbps: link.map(|link| link.downlink_bps as f64 / 1_000_000.0),
                link_expires_at: link.map(|link| link.expires_at),
                traffic,
            }
        })
        .collect::<Vec<_>>();
    nodes.sort_by_key(|node| node.node_id);
    NetworkAutopilotStatusV1 {
        schema: NetworkAutopilotStatusV1::SCHEMA.into(),
        generated_at: now,
        mode: "automatic_evidence_gated".into(),
        nodes,
        policy: NetworkAutopilotStatusV1::POLICY.into(),
    }
}

fn admit_traffic(
    traffic_class: TrafficClassV1,
    path: NetworkPathKindV1,
    link: Option<&rampage_protocol::LinkBenchmarkV1>,
    has_candidate: bool,
) -> TrafficAdmissionV1 {
    if path == NetworkPathKindV1::ControllerLocal {
        return TrafficAdmissionV1 {
            traffic_class,
            admitted: true,
            reason: "controller-local path".into(),
        };
    }
    if traffic_class == TrafficClassV1::AuthorityControl {
        return TrafficAdmissionV1 {
            traffic_class,
            admitted: has_candidate,
            reason: if has_candidate {
                "bounded authority traffic may use any authenticated candidate"
            } else {
                "no authenticated path candidate"
            }
            .into(),
        };
    }
    let Some(link) = link else {
        return TrafficAdmissionV1 {
            traffic_class,
            admitted: false,
            reason: "performance traffic waits for a fresh signed link benchmark".into(),
        };
    };
    let rtt_ms = link.rtt_micros_p50 as f64 / 1_000.0;
    let up_mbps = link.uplink_bps as f64 / 1_000_000.0;
    let down_mbps = link.downlink_bps as f64 / 1_000_000.0;
    let (admitted, reason) = match traffic_class {
        TrafficClassV1::AuthorityControl => unreachable!(),
        TrafficClassV1::InteractiveAi => (
            rtt_ms <= 75.0 && up_mbps >= 2.0 && down_mbps >= 10.0,
            "requires p50 RTT <= 75 ms, uplink >= 2 Mbps, and downlink >= 10 Mbps",
        ),
        TrafficClassV1::RemoteMedia => (
            rtt_ms <= 50.0 && up_mbps >= 10.0 && down_mbps >= 5.0,
            "requires p50 RTT <= 50 ms, uplink >= 10 Mbps, and downlink >= 5 Mbps",
        ),
        TrafficClassV1::Artifact => (
            up_mbps >= 5.0 && down_mbps >= 5.0,
            "requires bidirectional throughput >= 5 Mbps",
        ),
        TrafficClassV1::BulkBackground => (
            up_mbps >= 20.0 && down_mbps >= 20.0,
            "requires bidirectional throughput >= 20 Mbps",
        ),
    };
    TrafficAdmissionV1 {
        traffic_class,
        admitted,
        reason: reason.into(),
    }
}

/// Plan an independent shard set against one evolving reservation book.
///
/// The function is deliberately pure and all-or-nothing: callers receive placements for every
/// shard or an exact blocked member, and no authority is issued here. Adding each provisional
/// reservation before planning the next shard prevents a batch from overcommitting an offer.
pub fn plan_shard_set(
    set: &ShardSetV1,
    offers: &[ResourceOfferV1],
    reservations: &[ResourceReservation],
    policy: &AdmissionPolicy,
    now: DateTime<Utc>,
    local_inputs_by_job_and_node: &HashMap<(Uuid, Uuid), BTreeSet<String>>,
) -> Result<Vec<ShardPlacement>, ShardPlanningFailure> {
    let mut provisional = reservations
        .iter()
        .filter(|reservation| reservation.expires_at > now)
        .cloned()
        .collect::<Vec<_>>();
    let mut placements = Vec::with_capacity(set.shards.len());

    for job in &set.shards {
        let locality = offers
            .iter()
            .map(|offer| {
                (
                    offer.node_id,
                    local_inputs_by_job_and_node
                        .get(&(job.job_id, offer.node_id))
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect::<HashMap<_, _>>();
        let Some((offer, score)) =
            choose_offer_with_topology(job, offers, &provisional, policy, now, &locality)
        else {
            return Err(ShardPlanningFailure {
                blocked_job_id: job.job_id,
                planned_shards: placements.len(),
                reason: "no admissible offer after provisional shard reservations".into(),
            });
        };
        provisional.extend(job.requests.iter().map(|request| ResourceReservation {
            job_id: job.job_id,
            node_id: offer.node_id,
            class: request.class,
            amount: request.minimum,
            expires_at: set.deadline.min(job.deadline),
        }));
        placements.push(ShardPlacement {
            job_id: job.job_id,
            node_id: offer.node_id,
            score,
        });
    }

    Ok(placements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use rampage_protocol::{
        ArtifactRefV1, AvailabilityV1, LinkBenchmarkV1, MeshEndpointRecordV1, ResourceClass,
        ResourceQuantityV1, ResourceRequestV1, ShardSetV1, StorageClass, WorkloadTrust,
    };
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn chooses_more_headroom_after_hard_constraints() {
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
            requests: vec![ResourceRequestV1 {
                class: ResourceClass::CpuCompute,
                minimum: 2,
                preferred: 4,
                unit: "logical_core".into(),
                required_labels: BTreeMap::new(),
            }],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline: now + Duration::minutes(5),
            idempotency_key: "placement-test".into(),
        };
        let offer = |available| ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id: Uuid::now_v7(),
            observed_at: now,
            expires_at: now + Duration::minutes(1),
            resources: vec![ResourceQuantityV1 {
                class: ResourceClass::CpuCompute,
                capacity: available,
                available,
                unit: "logical_core".into(),
                labels: BTreeMap::new(),
            }],
            availability: AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 80,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::from(["rampage.echo.v1".into()]),
            workload_capabilities: Vec::new(),
            model_runtimes: Vec::new(),
            link_benchmark: None,
            mesh_endpoint: None,
            signature: "signed".into(),
        };
        let offers = vec![offer(4), offer(12)];
        let (selected, _) = choose_offer(&job, &offers).unwrap();
        assert_eq!(selected.node_id, offers[1].node_id);
    }

    #[test]
    fn pending_reservations_owner_reserve_and_guardband_prevent_overcommit() {
        let now = Utc::now();
        let (job, offer) = job_and_offer(now, 10, 7);
        let policy = AdmissionPolicy {
            protected_owner_reserve: BTreeMap::from([(ResourceClass::CpuCompute, 1)]),
            safety_guardband_percent: 10,
        };
        let reservations = vec![ResourceReservation {
            job_id: Uuid::now_v7(),
            node_id: offer.node_id,
            class: ResourceClass::CpuCompute,
            amount: 2,
            expires_at: now + Duration::minutes(1),
        }];
        let score = score_offer_with_admission(&job, &offer, &reservations, &policy, now);
        assert!(!score.satisfies); // 10 - 2 pending - 1 owner - 1 guardband = 6.
    }

    #[test]
    fn measured_staging_cost_changes_placement_only_when_it_outweighs_capacity() {
        let now = Utc::now();
        let (mut job, mut local) = job_and_offer(now, 4, 1);
        job.requests[0].preferred = 4;
        let digest = format!("sha256:{}", "ab".repeat(32));
        job.inputs = vec![ArtifactRefV1 {
            schema: "rampage.artifact-ref.v1".into(),
            digest: digest.clone(),
            size_bytes: 1024 * 1024,
            media_type: "application/octet-stream".into(),
            storage_class: StorageClass::Cache,
            encrypted: true,
        }];
        local.offer_id = Uuid::now_v7();
        let mut remote = local.clone();
        remote.node_id = Uuid::now_v7();
        remote.resources[0].capacity = 12;
        remote.resources[0].available = 12;
        remote.mesh_endpoint = Some(MeshEndpointRecordV1 {
            schema: MeshEndpointRecordV1::SCHEMA.into(),
            endpoint_id: "remote".into(),
            direct_addresses: vec!["127.0.0.1:1".into()],
            relay_urls: vec![],
            issued_at: now,
            expires_at: now + Duration::minutes(2),
            signature: "signed".into(),
        });
        remote.link_benchmark = Some(LinkBenchmarkV1 {
            schema: LinkBenchmarkV1::SCHEMA.into(),
            controller_endpoint_id: "controller".into(),
            observed_at: now,
            expires_at: now + Duration::minutes(2),
            rtt_micros_p50: 2_000,
            uplink_bps: 1_000_000_000,
            downlink_bps: 1_000_000_000,
            transfer_bytes: rampage_protocol::LINK_BENCHMARK_TRANSFER_BYTES,
            samples: 3,
            transport: "authenticated_quic".into(),
            observed_path: None,
        });
        let offers = vec![local.clone(), remote.clone()];
        let locality = HashMap::from([
            (local.node_id, BTreeSet::from([digest.clone()])),
            (remote.node_id, BTreeSet::new()),
        ]);
        let policy = AdmissionPolicy {
            protected_owner_reserve: BTreeMap::new(),
            safety_guardband_percent: 0,
        };
        let (selected, score) =
            choose_offer_with_topology(&job, &offers, &[], &policy, now, &locality).unwrap();
        assert_eq!(selected.node_id, remote.node_id);
        assert_eq!(score.topology_confidence, "measured");

        job.inputs[0].size_bytes = 64 * 1024 * 1024;
        remote.link_benchmark.as_mut().unwrap().downlink_bps = 20_000_000;
        let offers = vec![local.clone(), remote.clone()];
        let (selected, score) =
            choose_offer_with_topology(&job, &offers, &[], &policy, now, &locality).unwrap();
        assert_eq!(selected.node_id, local.node_id);
        assert_eq!(score.estimated_transfer_millis, 0);
    }

    #[test]
    fn shard_set_planning_uses_one_evolving_reservation_book() {
        let now = Utc::now();
        let project_id = Uuid::now_v7();
        let submitted_by = Uuid::now_v7();
        let make_shard = |index: usize| JobSpecV1 {
            schema: JobSpecV1::SCHEMA.into(),
            job_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            adapter: "rampage.echo.v1".into(),
            operation: "echo".into(),
            arguments: BTreeMap::from([("value".into(), index.to_string())]),
            inputs: vec![],
            requests: vec![ResourceRequestV1 {
                class: ResourceClass::CpuCompute,
                minimum: 1,
                preferred: 1,
                unit: "logical_core".into(),
                required_labels: BTreeMap::new(),
            }],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline: now + Duration::minutes(5),
            idempotency_key: format!("shard-{index}"),
        };
        let set = ShardSetV1 {
            schema: ShardSetV1::SCHEMA.into(),
            set_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            shards: (0..4).map(make_shard).collect(),
            minimum_successes: 4,
            deadline: now + Duration::minutes(5),
            idempotency_key: "set".into(),
        };
        let offers = vec![job_and_offer(now, 2, 1).1, job_and_offer(now, 2, 1).1];
        let placements = plan_shard_set(
            &set,
            &offers,
            &[],
            &AdmissionPolicy {
                protected_owner_reserve: BTreeMap::new(),
                safety_guardband_percent: 0,
            },
            now,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(placements.len(), 4);
        assert_eq!(
            placements
                .iter()
                .map(|placement| placement.node_id)
                .collect::<BTreeSet<_>>()
                .len(),
            2
        );
    }

    #[test]
    fn shard_set_planning_fails_without_partial_authority() {
        let now = Utc::now();
        let project_id = Uuid::now_v7();
        let submitted_by = Uuid::now_v7();
        let shards = (0..2)
            .map(|index| JobSpecV1 {
                schema: JobSpecV1::SCHEMA.into(),
                job_id: Uuid::now_v7(),
                project_id,
                submitted_by,
                adapter: "rampage.echo.v1".into(),
                operation: "echo".into(),
                arguments: BTreeMap::new(),
                inputs: vec![],
                requests: vec![ResourceRequestV1 {
                    class: ResourceClass::CpuCompute,
                    minimum: 1,
                    preferred: 1,
                    unit: "logical_core".into(),
                    required_labels: BTreeMap::new(),
                }],
                trust: WorkloadTrust::NativeTrusted,
                restart_tolerant: true,
                network_allowlist: BTreeSet::new(),
                deadline: now + Duration::minutes(5),
                idempotency_key: format!("shard-{index}"),
            })
            .collect();
        let set = ShardSetV1 {
            schema: ShardSetV1::SCHEMA.into(),
            set_id: Uuid::now_v7(),
            project_id,
            submitted_by,
            shards,
            minimum_successes: 1,
            deadline: now + Duration::minutes(5),
            idempotency_key: "set".into(),
        };
        let failure = plan_shard_set(
            &set,
            &[job_and_offer(now, 1, 1).1],
            &[],
            &AdmissionPolicy {
                protected_owner_reserve: BTreeMap::new(),
                safety_guardband_percent: 0,
            },
            now,
            &HashMap::new(),
        )
        .unwrap_err();
        assert_eq!(failure.planned_shards, 1);
    }

    #[test]
    fn break_even_uses_fabric_only_when_conservative_p90_clears_the_gate() {
        let now = Utc::now();
        let baseline_id = Uuid::now_v7();
        let remote_id = Uuid::now_v7();
        let dividend = benchmark_dividend(baseline_id, remote_id);
        let mut baseline = model_offer(now, 16, false, 0, 0);
        baseline.node_id = baseline_id;
        let mut remote = model_offer(now, 16, true, 2_000, 1_000);
        remote.node_id = remote_id;
        let request = BreakEvenRequestV1 {
            schema: BreakEvenRequestV1::SCHEMA.into(),
            workload_class: WorkloadClassV1::BuildTest,
            fastest_node_compute_ms: 60_000,
            input_bytes: 1_000_000,
            output_bytes: 100_000,
            startup_ms: 200,
            restart_tolerant: true,
            minimum_gain_percent: 10.0,
        };
        let plan = plan_break_even(
            &request,
            Some((&dividend, now - Duration::minutes(5))),
            &[baseline, remote],
            now,
        );
        assert_eq!(plan.decision, BreakEvenDecisionV1::UseFabric);
        assert_eq!(plan.selected_node_ids.len(), 2);
        assert!(plan.estimated_gain_percent.unwrap() > 20.0);
        assert_eq!(
            plan.topology_confidence,
            "fresh_signed_compute_and_link_evidence"
        );
    }

    #[test]
    fn break_even_refuses_slow_or_unmeasured_remote_paths() {
        let now = Utc::now();
        let baseline_id = Uuid::now_v7();
        let remote_id = Uuid::now_v7();
        let dividend = benchmark_dividend(baseline_id, remote_id);
        let mut baseline = model_offer(now, 16, false, 0, 0);
        baseline.node_id = baseline_id;
        let mut remote = model_offer(now, 16, true, 80_000, 10);
        remote.node_id = remote_id;
        let request = BreakEvenRequestV1 {
            schema: BreakEvenRequestV1::SCHEMA.into(),
            workload_class: WorkloadClassV1::InteractiveAi,
            fastest_node_compute_ms: 30_000,
            input_bytes: 512 * 1024 * 1024,
            output_bytes: 8 * 1024 * 1024,
            startup_ms: 1_000,
            restart_tolerant: true,
            minimum_gain_percent: 5.0,
        };
        let plan = plan_break_even(
            &request,
            Some((&dividend, now)),
            &[baseline.clone(), remote.clone()],
            now,
        );
        assert_eq!(plan.decision, BreakEvenDecisionV1::StayOnFastestNode);
        assert_eq!(plan.selected_node_ids, vec![baseline_id]);

        remote.link_benchmark = None;
        let plan = plan_break_even(&request, Some((&dividend, now)), &[baseline, remote], now);
        assert_eq!(plan.decision, BreakEvenDecisionV1::InsufficientEvidence);
        assert!(plan.reason.contains("lacks a fresh signed link benchmark"));
    }

    #[test]
    fn break_even_requires_path_evidence_when_the_fastest_node_is_remote() {
        let now = Utc::now();
        let remote_fastest_id = Uuid::now_v7();
        let local_id = Uuid::now_v7();
        let dividend = benchmark_dividend(remote_fastest_id, local_id);
        let mut remote_fastest = model_offer(now, 16, true, 2_000, 1_000);
        remote_fastest.node_id = remote_fastest_id;
        remote_fastest.link_benchmark = None;
        let mut local = model_offer(now, 16, false, 0, 0);
        local.node_id = local_id;
        let request = BreakEvenRequestV1 {
            schema: BreakEvenRequestV1::SCHEMA.into(),
            workload_class: WorkloadClassV1::BatchAi,
            fastest_node_compute_ms: 60_000,
            input_bytes: 1_000_000,
            output_bytes: 100_000,
            startup_ms: 200,
            restart_tolerant: true,
            minimum_gain_percent: 10.0,
        };

        let plan = plan_break_even(
            &request,
            Some((&dividend, now - Duration::minutes(5))),
            &[remote_fastest, local],
            now,
        );

        assert_eq!(plan.decision, BreakEvenDecisionV1::InsufficientEvidence);
        assert_eq!(plan.baseline_node_id, Some(remote_fastest_id));
        assert!(plan.reason.contains("lacks a fresh signed link benchmark"));
    }

    #[test]
    fn network_autopilot_admits_each_traffic_class_from_fresh_thresholds() {
        let now = Utc::now();
        let local = model_offer(now, 16, false, 0, 0);
        let mut remote = model_offer(now, 16, true, 20_000, 100);
        remote.link_benchmark.as_mut().unwrap().observed_path =
            Some(rampage_protocol::ObservedLinkPathV1::Direct);
        let status = network_autopilot_status(&[local, remote.clone()], now);
        assert_eq!(status.nodes.len(), 2);
        let local = status
            .nodes
            .iter()
            .find(|node| node.preferred_path == NetworkPathKindV1::ControllerLocal)
            .unwrap();
        assert!(local.traffic.iter().all(|traffic| traffic.admitted));
        let measured = status
            .nodes
            .iter()
            .find(|node| node.node_id == remote.node_id)
            .unwrap();
        assert_eq!(measured.preferred_path, NetworkPathKindV1::DirectMeasured);
        assert!(measured.traffic.iter().all(|traffic| traffic.admitted));
    }

    #[test]
    fn network_autopilot_keeps_control_but_fences_unmeasured_performance() {
        let now = Utc::now();
        let mut remote = model_offer(now, 16, true, 20_000, 100);
        remote.link_benchmark = None;
        remote.mesh_endpoint.as_mut().unwrap().relay_urls =
            vec!["https://relay.example.test".into()];
        let status = network_autopilot_status(&[remote], now);
        let node = &status.nodes[0];
        assert_eq!(node.preferred_path, NetworkPathKindV1::OwnerRelayBootstrap);
        assert!(node.traffic[0].admitted);
        assert!(node.traffic[1..].iter().all(|traffic| !traffic.admitted));
    }

    #[test]
    fn maximum_model_plan_combines_only_qualified_compatible_memory() {
        let now = Utc::now();
        let request = model_request(now, ComputeStrategy::MaximumModelSize, 40, 4);
        let offers = vec![
            model_offer(now, 24, false, 500, 10_000),
            model_offer(now, 24, true, 500, 10_000),
        ];
        let plan = plan_model_session(&request, &offers, now);
        assert_eq!(plan.state, ModelPlanState::Ready);
        assert_eq!(plan.backend, Some(ModelBackend::ExoMlx));
        assert_eq!(plan.parallelism, Some(ModelParallelism::Pipeline));
        assert!(plan.distributed);
        assert_eq!(plan.placements.len(), 2);
        assert_eq!(
            plan.placements
                .iter()
                .map(|placement| placement.assigned_bytes)
                .sum::<u64>(),
            request.required_bytes()
        );
        assert_eq!(plan.execution_authority, "none_preview_only");
    }

    #[test]
    fn speed_boost_rejects_slow_distributed_topology() {
        let now = Utc::now();
        let request = model_request(now, ComputeStrategy::SpeedBoost, 20, 2);
        let offers = vec![
            model_offer(now, 32, false, 500, 10_000),
            model_offer(now, 32, true, 12_000, 300),
        ];
        let plan = plan_model_session(&request, &offers, now);
        assert_eq!(plan.state, ModelPlanState::Ready);
        assert_eq!(plan.parallelism, Some(ModelParallelism::WholeModel));
        assert!(!plan.distributed);
        assert_eq!(plan.predicted_speedup_milli, 1_000);
        assert!(
            plan.warnings
                .iter()
                .any(|warning| warning.contains("speedup gate"))
        );
    }

    #[test]
    fn candidate_runtime_reports_theoretical_fit_without_authority() {
        let now = Utc::now();
        let request = model_request(now, ComputeStrategy::MaximumModelSize, 30, 2);
        let mut offers = vec![
            model_offer(now, 20, false, 500, 10_000),
            model_offer(now, 20, true, 500, 10_000),
        ];
        for offer in &mut offers {
            offer.model_runtimes[0].status = ModelRuntimeStatus::Candidate;
            offer.model_runtimes[0].certification_digest = None;
        }
        let plan = plan_model_session(&request, &offers, now);
        assert_eq!(plan.state, ModelPlanState::QualificationRequired);
        assert!(
            plan.blockers
                .iter()
                .any(|blocker| blocker.contains("qualification digest"))
        );
    }

    fn model_request(
        now: chrono::DateTime<Utc>,
        strategy: ComputeStrategy,
        weights_gib: u64,
        kv_gib: u64,
    ) -> ModelSessionRequestV1 {
        ModelSessionRequestV1 {
            schema: ModelSessionRequestV1::SCHEMA.into(),
            session_id: Uuid::now_v7(),
            model_id: "test/large-model".into(),
            estimated_weight_bytes: weights_gib * 1024 * 1024 * 1024,
            kv_cache_bytes: kv_gib * 1024 * 1024 * 1024,
            context_tokens: 32_768,
            strategy,
            max_nodes: 8,
            deadline: now + Duration::minutes(10),
            idempotency_key: Uuid::now_v7().to_string(),
        }
    }

    fn benchmark_dividend(baseline_id: Uuid, remote_id: Uuid) -> FabricBenchmarkResultV1 {
        FabricBenchmarkResultV1 {
            schema: FabricBenchmarkResultV1::SCHEMA.into(),
            set_id: Uuid::now_v7(),
            status: "succeeded".into(),
            nodes: vec![
                rampage_protocol::FabricBenchmarkNodeV1 {
                    job_id: Uuid::now_v7(),
                    node_id: baseline_id,
                    name: "Main PC".into(),
                    receipt_id: Uuid::now_v7(),
                    lanes: 8,
                    total_hashes: 3_000_000,
                    elapsed_ms: 50.0,
                    hashes_per_second: 60_000,
                    result_digest: format!("sha256:{}", "a".repeat(64)),
                },
                rampage_protocol::FabricBenchmarkNodeV1 {
                    job_id: Uuid::now_v7(),
                    node_id: remote_id,
                    name: "Laptop".into(),
                    receipt_id: Uuid::now_v7(),
                    lanes: 4,
                    total_hashes: 2_000_000,
                    elapsed_ms: 50.0,
                    hashes_per_second: 40_000,
                    result_digest: format!("sha256:{}", "b".repeat(64)),
                },
            ],
            fabric_hashes_per_second: 100_000,
            fastest_node_hashes_per_second: 60_000,
            effective_scale_over_fastest_node: 5.0 / 3.0,
            verified_extra_capacity_percent: 200.0 / 3.0,
            estimated_time_saved_percent: 40.0,
            time_returned_hours_per_100: 40.0,
            proof_basis: FabricBenchmarkResultV1::PROOF_BASIS.into(),
            applicability: FabricBenchmarkResultV1::APPLICABILITY.into(),
            all_results_signed: true,
        }
    }

    fn model_offer(
        now: chrono::DateTime<Utc>,
        memory_gib: u64,
        remote: bool,
        rtt_micros: u64,
        bandwidth_mbps: u64,
    ) -> ResourceOfferV1 {
        let node_id = Uuid::now_v7();
        let bytes = memory_gib * 1024 * 1024 * 1024;
        ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id,
            observed_at: now,
            expires_at: now + Duration::minutes(1),
            resources: vec![ResourceQuantityV1 {
                class: ResourceClass::RamWorkingSet,
                capacity: bytes,
                available: bytes,
                unit: "byte".into(),
                labels: BTreeMap::new(),
            }],
            availability: AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 80,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::from(["rampage.exo-mlx.v1".into()]),
            workload_capabilities: Vec::new(),
            model_runtimes: vec![ModelRuntimeOfferV1 {
                schema: ModelRuntimeOfferV1::SCHEMA.into(),
                adapter: "rampage.exo-mlx.v1".into(),
                backend: ModelBackend::ExoMlx,
                runtime_version: "pinned-test".into(),
                runtime_digest: format!("sha256:{}", "a".repeat(64)),
                compatibility_key: "exo-mlx-unified-test".into(),
                memory_kind: ModelMemoryKind::Unified,
                available_model_bytes: bytes,
                supported_parallelism: BTreeSet::from([
                    ModelParallelism::WholeModel,
                    ModelParallelism::Pipeline,
                    ModelParallelism::Tensor,
                    ModelParallelism::Replica,
                ]),
                status: ModelRuntimeStatus::Qualified,
                installed_models: vec![],
                certification_digest: Some(format!("sha256:{}", "b".repeat(64))),
            }],
            link_benchmark: remote.then(|| LinkBenchmarkV1 {
                schema: LinkBenchmarkV1::SCHEMA.into(),
                controller_endpoint_id: "controller".into(),
                observed_at: now,
                expires_at: now + Duration::minutes(2),
                rtt_micros_p50: rtt_micros,
                uplink_bps: bandwidth_mbps * 1_000_000,
                downlink_bps: bandwidth_mbps * 1_000_000,
                transfer_bytes: rampage_protocol::LINK_BENCHMARK_TRANSFER_BYTES,
                samples: 3,
                transport: "authenticated_quic".into(),
                observed_path: None,
            }),
            mesh_endpoint: remote.then(|| MeshEndpointRecordV1 {
                schema: MeshEndpointRecordV1::SCHEMA.into(),
                endpoint_id: node_id.to_string(),
                direct_addresses: vec!["127.0.0.1:1".into()],
                relay_urls: vec![],
                issued_at: now,
                expires_at: now + Duration::minutes(2),
                signature: "signed".into(),
            }),
            signature: "signed".into(),
        }
    }

    fn job_and_offer(
        now: chrono::DateTime<Utc>,
        available: u64,
        minimum: u64,
    ) -> (JobSpecV1, ResourceOfferV1) {
        let job = JobSpecV1 {
            schema: JobSpecV1::SCHEMA.into(),
            job_id: Uuid::now_v7(),
            project_id: Uuid::now_v7(),
            submitted_by: Uuid::now_v7(),
            adapter: "rampage.echo.v1".into(),
            operation: "echo".into(),
            arguments: BTreeMap::new(),
            inputs: vec![],
            requests: vec![ResourceRequestV1 {
                class: ResourceClass::CpuCompute,
                minimum,
                preferred: minimum,
                unit: "logical_core".into(),
                required_labels: BTreeMap::new(),
            }],
            trust: WorkloadTrust::NativeTrusted,
            restart_tolerant: true,
            network_allowlist: BTreeSet::new(),
            deadline: now + Duration::minutes(5),
            idempotency_key: "reservation-test".into(),
        };
        let offer = ResourceOfferV1 {
            schema: "rampage.resource-offer.v1".into(),
            offer_id: Uuid::now_v7(),
            node_id: Uuid::now_v7(),
            observed_at: now,
            expires_at: now + Duration::minutes(1),
            resources: vec![ResourceQuantityV1 {
                class: ResourceClass::CpuCompute,
                capacity: available,
                available,
                unit: "logical_core".into(),
                labels: BTreeMap::new(),
            }],
            availability: AvailabilityV1 {
                on_ac_power: true,
                battery_percent: None,
                thermal_headroom_percent: 80,
                foreground_allowed: true,
                owner_idle: true,
            },
            adapters: BTreeSet::from(["rampage.echo.v1".into()]),
            workload_capabilities: Vec::new(),
            model_runtimes: Vec::new(),
            link_benchmark: None,
            mesh_endpoint: None,
            signature: "signed".into(),
        };
        (job, offer)
    }
}
