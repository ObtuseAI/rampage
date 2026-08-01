from __future__ import annotations

from datetime import UTC, datetime, timedelta
from pathlib import PurePosixPath
from typing import Literal

from .models import (
    AutonomyEnvelope,
    GovernorPromotionCandidate,
    ImprovementProposal,
    PromotionDecision,
    RiskClass,
)

REQUIRED_GATES = (
    "g0_schema_policy_static",
    "g1_deterministic_replay",
    "g2_quality_reliability_cost",
    "g3_sealed_holdout",
    "g4_adversarial_security",
    "g5_independent_replication",
    "g6_shadow",
    "g7_canary_rollback",
)

PROTECTED_PREFIXES = (
    "crates/rampage-policy/",
    "crates/rampage-protocol/",
    "crates/rampage-controller/",
    "crates/rampage-agent/",
    "crates/rampage-mesh/",
    "crates/rampage-storage/",
    "policies/",
    "evals/holdouts/",
    "signing/",
    "updater/",
    "services/intelligence/src/rampage_intelligence/promotion.py",
    ".github/workflows/",
)


def _canonical_repo_path(path: str) -> str | None:
    normalized = path.replace("\\", "/")
    segments = normalized.split("/")
    if (
        not normalized
        or normalized.startswith("/")
        or ":" in normalized
        or any(segment in {"", ".", ".."} for segment in segments)
    ):
        return None
    return PurePosixPath(normalized).as_posix().casefold()


def classify_paths(paths: tuple[str, ...]) -> RiskClass:
    normalized = tuple(_canonical_repo_path(path) or "" for path in paths)
    if any(path.startswith(PROTECTED_PREFIXES) for path in normalized):
        return RiskClass.R3_AUTHORITY_CRITICAL
    if any(
        path.endswith(("pyproject.toml", "Cargo.toml", "package.json"))
        or "/migrations/" in f"/{path}/"
        or path.startswith("contracts/")
        for path in normalized
    ):
        return RiskClass.R2_PROTECTED_CHANGE
    if all(path.startswith(("prompts/", "routing/", "cache/")) for path in normalized):
        return RiskClass.R0_CONFIGURATION
    return RiskClass.R1_ALLOWLISTED_SOURCE


def evaluate_promotion(
    proposal: ImprovementProposal,
    *,
    envelope: AutonomyEnvelope,
) -> PromotionDecision:
    def decision(
        outcome: Literal["eligible", "denied"],
        reason: str,
        *,
        missing_gates: tuple[str, ...] = (),
    ) -> PromotionDecision:
        return PromotionDecision(
            proposal_id=proposal.proposal_id,
            envelope_id=envelope.envelope_id,
            decision=outcome,
            reason=reason,
            missing_gates=missing_gates,
        )

    if proposal.project_id != envelope.project_id:
        return decision("denied", "Proposal and autonomy envelope project identities differ")
    if not envelope.enabled:
        return decision("denied", "Autonomous promotion is disabled for this project envelope")
    if len(proposal.changed_paths) > envelope.max_changed_paths:
        return decision("denied", "Candidate exceeds the envelope's changed-path threshold")
    if any(_canonical_repo_path(path) is None for path in proposal.changed_paths):
        return decision("denied", "Candidate contains a non-canonical repository path")
    classified = classify_paths(proposal.changed_paths)
    if classified != proposal.risk:
        return decision(
            "denied",
            f"Declared risk {proposal.risk} does not match classified risk {classified}",
        )
    if proposal.risk is RiskClass.R3_AUTHORITY_CRITICAL:
        return decision(
            "denied",
            "Authority-critical changes are outside every autonomous envelope",
        )
    risk_rank = {
        RiskClass.R0_CONFIGURATION: 0,
        RiskClass.R1_ALLOWLISTED_SOURCE: 1,
        RiskClass.R2_PROTECTED_CHANGE: 2,
        RiskClass.R3_AUTHORITY_CRITICAL: 3,
    }
    if risk_rank[proposal.risk] > risk_rank[envelope.max_risk]:
        return decision("denied", "Candidate risk exceeds the autonomous envelope ceiling")
    if proposal.risk is RiskClass.R2_PROTECTED_CHANGE and not envelope.allow_protected_changes:
        return decision("denied", "Protected changes are disabled in this autonomy envelope")
    if envelope.allowed_path_prefixes:
        canonical_prefixes = tuple(
            _canonical_repo_path(prefix.rstrip("/\\"))
            for prefix in envelope.allowed_path_prefixes
        )
        if any(prefix is None for prefix in canonical_prefixes):
            return decision("denied", "Autonomy envelope contains a non-canonical path prefix")
        normalized_paths = tuple(
            _canonical_repo_path(path) or ""
            for path in proposal.changed_paths
        )
        allowed_prefixes = tuple(
            f"{prefix}/" for prefix in canonical_prefixes if prefix is not None
        )
        if any(
            not any(path == prefix[:-1] or path.startswith(prefix) for prefix in allowed_prefixes)
            for path in normalized_paths
        ):
            return decision("denied", "Candidate changes a path outside the autonomy envelope")
    gates = {gate.name: gate for gate in proposal.gates}
    missing = tuple(
        name
        for name in REQUIRED_GATES
        if name not in gates or not gates[name].passed or not gates[name].evidence_digest
    )
    if missing:
        return decision(
            "denied",
            "Required evidence gates are missing or failed",
            missing_gates=missing,
        )
    if (
        envelope.require_independent_replication
        and not gates["g5_independent_replication"].independent
    ):
        return decision(
            "denied",
            "Independent replication is required by the autonomy envelope",
            missing_gates=("g5_independent_replication.independent",),
        )
    requested_at = datetime.now(UTC)
    return PromotionDecision(
        proposal_id=proposal.proposal_id,
        envelope_id=envelope.envelope_id,
        decision="eligible",
        reason=(
            "All deterministic thresholds passed inside the owner-defined envelope; "
            "the Rust Governor may independently authorize a canary"
        ),
        signed_by_governor=False,
        governor_candidate=GovernorPromotionCandidate(
            proposal_id=proposal.proposal_id,
            project_id=proposal.project_id,
            base_revision=proposal.base_revision,
            candidate_digest=proposal.candidate_digest,
            changed_paths=proposal.changed_paths,
            risk=proposal.risk,
            gates=proposal.gates,
            requested_at=requested_at,
            expires_at=requested_at + timedelta(minutes=10),
        ),
    )

