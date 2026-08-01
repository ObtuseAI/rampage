from uuid import uuid4

from rampage_intelligence.models import (
    AutonomyEnvelope,
    EvidenceGate,
    ImprovementProposal,
    RiskClass,
)
from rampage_intelligence.promotion import REQUIRED_GATES, evaluate_promotion


def evidence() -> tuple[EvidenceGate, ...]:
    return tuple(
        EvidenceGate(
            name=name,
            passed=True,
            evidence_digest=f"sha256:{index:064x}",
            independent=name == "g5_independent_replication",
        )
        for index, name in enumerate(REQUIRED_GATES, start=1)
    )


def proposal(path: str, risk: RiskClass) -> ImprovementProposal:
    return ImprovementProposal(
        project_id=uuid4(),
        base_revision="abc123",
        candidate_digest=f"sha256:{'f' * 64}",
        changed_paths=(path,),
        change_summary="candidate change",
        risk=risk,
        gates=evidence(),
    )


def envelope(
    candidate: ImprovementProposal,
    *,
    max_risk: RiskClass = RiskClass.R1_ALLOWLISTED_SOURCE,
    allow_protected_changes: bool = False,
    allowed_path_prefixes: tuple[str, ...] = (),
) -> AutonomyEnvelope:
    return AutonomyEnvelope(
        project_id=candidate.project_id,
        max_risk=max_risk,
        allow_protected_changes=allow_protected_changes,
        allowed_path_prefixes=allowed_path_prefixes,
    )


def test_r1_is_autonomously_thresholded_without_per_change_approval() -> None:
    candidate = proposal("src/cache_tuning.py", RiskClass.R1_ALLOWLISTED_SOURCE)
    eligible = evaluate_promotion(
        candidate,
        envelope=envelope(candidate, allowed_path_prefixes=("src/",)),
    )
    assert eligible.decision == "eligible"
    assert not eligible.per_change_approval_required
    assert not eligible.signed_by_governor
    assert eligible.governor_candidate is not None
    assert eligible.governor_candidate.candidate_digest == candidate.candidate_digest


def test_authority_critical_changes_are_automatically_denied() -> None:
    candidate = proposal(
        "crates/rampage-policy/src/lib.rs",
        RiskClass.R3_AUTHORITY_CRITICAL,
    )
    decision = evaluate_promotion(
        candidate,
        envelope=envelope(
            candidate,
            max_risk=RiskClass.R3_AUTHORITY_CRITICAL,
            allow_protected_changes=True,
        ),
    )
    assert decision.decision == "denied"
    assert "outside every autonomous envelope" in decision.reason


def test_risk_understatement_is_denied() -> None:
    candidate = proposal("contracts/job.json", RiskClass.R1_ALLOWLISTED_SOURCE)
    assert evaluate_promotion(candidate, envelope=envelope(candidate)).decision == "denied"


def test_protected_change_canary_can_run_only_inside_explicit_envelope() -> None:
    candidate = proposal("contracts/job.json", RiskClass.R2_PROTECTED_CHANGE)
    denied = evaluate_promotion(
        candidate,
        envelope=envelope(candidate, max_risk=RiskClass.R2_PROTECTED_CHANGE),
    )
    assert denied.decision == "denied"
    eligible = evaluate_promotion(
        candidate,
        envelope=envelope(
            candidate,
            max_risk=RiskClass.R2_PROTECTED_CHANGE,
            allow_protected_changes=True,
            allowed_path_prefixes=("contracts/",),
        ),
    )
    assert eligible.decision == "eligible"


def test_path_escape_is_denied_without_review_queue() -> None:
    candidate = proposal("src/cache_tuning.py", RiskClass.R1_ALLOWLISTED_SOURCE)
    decision = evaluate_promotion(
        candidate,
        envelope=envelope(candidate, allowed_path_prefixes=("routing/",)),
    )
    assert decision.decision == "denied"


def test_noncanonical_and_case_varied_authority_paths_cannot_bypass_classification() -> None:
    escaped = proposal("src/../routing/cache.py", RiskClass.R1_ALLOWLISTED_SOURCE)
    assert evaluate_promotion(escaped, envelope=envelope(escaped)).decision == "denied"
    critical = proposal(
        "CRATES/RAMPAGE-POLICY/src/lib.rs",
        RiskClass.R1_ALLOWLISTED_SOURCE,
    )
    decision = evaluate_promotion(critical, envelope=envelope(critical))
    assert decision.decision == "denied"
    assert "does not match classified risk" in decision.reason

