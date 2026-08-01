//! OnePool admission and deterministic scheduling.

use chrono::{DateTime, Utc};
use rampage_protocol::{JobSpecV1, ResourceClass, ResourceOfferV1, ShardSetV1};
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
            link_benchmark: None,
            mesh_endpoint: None,
            signature: "signed".into(),
        };
        (job, offer)
    }
}
