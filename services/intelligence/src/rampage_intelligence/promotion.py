from __future__ import annotations

from pathlib import PurePosixPath

from .models import ImprovementProposal, PromotionDecision, RiskClass

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
    "policies/",
    "evals/holdouts/",
    "signing/",
    "updater/",
    ".github/workflows/release",
)


def classify_paths(paths: tuple[str, ...]) -> RiskClass:
    normalized = tuple(PurePosixPath(path.replace("\\", "/")).as_posix() for path in paths)
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
    trusted_autopilot: bool,
) -> PromotionDecision:
    classified = classify_paths(proposal.changed_paths)
    if classified != proposal.risk:
        return PromotionDecision(
            proposal_id=proposal.proposal_id,
            decision="denied",
            reason=f"Declared risk {proposal.risk} does not match classified risk {classified}",
        )
    if proposal.risk in (RiskClass.R2_PROTECTED_CHANGE, RiskClass.R3_AUTHORITY_CRITICAL):
        return PromotionDecision(
            proposal_id=proposal.proposal_id,
            decision="human_review",
            reason="Protected and authority-critical changes are never autonomously promoted",
        )
    gates = {gate.name: gate for gate in proposal.gates}
    missing = tuple(
        name
        for name in REQUIRED_GATES
        if name not in gates or not gates[name].passed or not gates[name].evidence_digest
    )
    if missing:
        return PromotionDecision(
            proposal_id=proposal.proposal_id,
            decision="denied",
            reason="Required evidence gates are missing or failed",
            missing_gates=missing,
        )
    if proposal.risk is RiskClass.R1_ALLOWLISTED_SOURCE and not trusted_autopilot:
        return PromotionDecision(
            proposal_id=proposal.proposal_id,
            decision="human_review",
            reason="R1 promotion requires explicit per-project Trusted Autopilot opt-in",
        )
    return PromotionDecision(
        proposal_id=proposal.proposal_id,
        decision="eligible",
        reason="Evidence is eligible for the Rust Governor's signed promotion decision",
        signed_by_governor=False,
    )

