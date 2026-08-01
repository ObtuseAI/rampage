//! OnePool admission and deterministic scheduling.

use chrono::{DateTime, Utc};
use rampage_protocol::{
    ComputeStrategy, JobSpecV1, ModelBackend, ModelMemoryKind, ModelParallelism,
    ModelRuntimeOfferV1, ModelRuntimeStatus, ModelSessionRequestV1, ResourceClass, ResourceOfferV1,
    ShardSetV1,
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
            model_runtimes: Vec::new(),
            link_benchmark: None,
            mesh_endpoint: None,
            signature: "signed".into(),
        };
        (job, offer)
    }
}
