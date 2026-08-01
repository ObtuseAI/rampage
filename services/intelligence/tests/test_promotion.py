from uuid import uuid4

from rampage_intelligence.models import (
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
        changed_paths=(path,),
        change_summary="candidate change",
        risk=risk,
        gates=evidence(),
    )


def test_r1_requires_explicit_trusted_autopilot() -> None:
    candidate = proposal("src/cache_tuning.py", RiskClass.R1_ALLOWLISTED_SOURCE)
    assert evaluate_promotion(candidate, trusted_autopilot=False).decision == "human_review"
    eligible = evaluate_promotion(candidate, trusted_autopilot=True)
    assert eligible.decision == "eligible"
    assert not eligible.signed_by_governor


def test_governor_and_policy_changes_never_auto_promote() -> None:
    candidate = proposal(
        "crates/rampage-policy/src/lib.rs",
        RiskClass.R3_AUTHORITY_CRITICAL,
    )
    decision = evaluate_promotion(candidate, trusted_autopilot=True)
    assert decision.decision == "human_review"


def test_risk_understatement_is_denied() -> None:
    candidate = proposal("contracts/job.json", RiskClass.R1_ALLOWLISTED_SOURCE)
    assert evaluate_promotion(candidate, trusted_autopilot=True).decision == "denied"

