from __future__ import annotations

from datetime import UTC, datetime
from enum import StrEnum
from typing import Annotated, Literal
from uuid import UUID, uuid4

from pydantic import BaseModel, ConfigDict, Field, StringConstraints

NonEmpty = Annotated[str, StringConstraints(strip_whitespace=True, min_length=1)]
Sha256Digest = Annotated[str, StringConstraints(pattern=r"^sha256:[0-9a-f]{64}$")]


class StrictModel(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
        frozen=True,
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
    )


class CapabilityState(StrEnum):
    FULL = "full"
    LOCAL_REDUCED = "local_reduced"
    DETERMINISTIC_ONLY = "deterministic_only"
    READ_ONLY = "read_only"
    BLOCKED = "blocked"


class RiskClass(StrEnum):
    R0_CONFIGURATION = "r0_configuration"
    R1_ALLOWLISTED_SOURCE = "r1_allowlisted_source"
    R2_PROTECTED_CHANGE = "r2_protected_change"
    R3_AUTHORITY_CRITICAL = "r3_authority_critical"


class GoalIntent(StrictModel):
    schema_id: Literal["rampage.goal-intent.v1"] = Field(
        default="rampage.goal-intent.v1", alias="schema"
    )
    goal_id: UUID = Field(default_factory=uuid4)
    project_id: UUID
    principal_id: UUID
    objective: NonEmpty
    success_criteria: tuple[NonEmpty, ...] = ()
    constraints: tuple[NonEmpty, ...] = ()
    repository: str | None = None
    allow_network: bool = False
    allow_models: bool = True
    max_wall_seconds: int = Field(default=1800, ge=1, le=86_400)
    submitted_at: datetime = Field(default_factory=lambda: datetime.now(UTC))


class CompiledIntent(StrictModel):
    goal_id: UUID
    objective: NonEmpty
    acceptance_checks: tuple[NonEmpty, ...]
    constraints: tuple[NonEmpty, ...]
    blocked_authority_requests: tuple[NonEmpty, ...] = ()
    capability_state: CapabilityState


class CapabilityRequest(StrictModel):
    adapter: NonEmpty
    operation: NonEmpty
    arguments: dict[str, str] = Field(default_factory=dict)
    restart_tolerant: bool = True
    network_allowlist: tuple[str, ...] = ()
    reason: NonEmpty


class StageProposal(StrictModel):
    stage: NonEmpty
    summary: NonEmpty
    claims: tuple[NonEmpty, ...] = ()
    uncertainties: tuple[NonEmpty, ...] = ()
    capability_requests: tuple[CapabilityRequest, ...] = ()
    artifacts_expected: tuple[NonEmpty, ...] = ()
    requires_human_review: bool = False
    requires_governor_authorization: bool = True


class VerificationFinding(StrictModel):
    check: NonEmpty
    passed: bool
    evidence: NonEmpty


class VerificationReport(StrictModel):
    deterministic: Literal[True] = True
    passed: bool
    findings: tuple[VerificationFinding, ...]
    missing_evidence: tuple[NonEmpty, ...] = ()


class WorkflowResult(StrictModel):
    schema_id: Literal["rampage.goal-workflow-result.v1"] = Field(
        default="rampage.goal-workflow-result.v1", alias="schema"
    )
    goal_id: UUID
    capability_state: CapabilityState
    stages: tuple[StageProposal, ...]
    verification: VerificationReport
    governor_action: Literal["request_leases", "blocked"]
    explanation: NonEmpty
    completed_at: datetime = Field(default_factory=lambda: datetime.now(UTC))


class EvidenceGate(StrictModel):
    name: NonEmpty
    passed: bool
    evidence_digest: str | None = None
    independent: bool = False


class ImprovementProposal(StrictModel):
    schema_id: Literal["rampage.improvement-proposal.v1"] = Field(
        default="rampage.improvement-proposal.v1", alias="schema"
    )
    proposal_id: UUID = Field(default_factory=uuid4)
    project_id: UUID
    base_revision: NonEmpty
    candidate_digest: Sha256Digest
    changed_paths: tuple[NonEmpty, ...]
    change_summary: NonEmpty
    risk: RiskClass
    gates: tuple[EvidenceGate, ...]


class AutonomyEnvelope(StrictModel):
    schema_id: Literal["rampage.autonomy-envelope.v1"] = Field(
        default="rampage.autonomy-envelope.v1", alias="schema"
    )
    envelope_id: UUID = Field(default_factory=uuid4)
    project_id: UUID
    enabled: bool = True
    max_risk: RiskClass = RiskClass.R1_ALLOWLISTED_SOURCE
    allowed_path_prefixes: tuple[NonEmpty, ...] = ()
    allow_protected_changes: bool = False
    max_changed_paths: int = Field(default=32, ge=1, le=256)
    require_independent_replication: bool = True
    per_change_approval_required: Literal[False] = False
    authority_expansion: Literal["deny"] = "deny"


class PromotionEvaluationRequest(StrictModel):
    proposal: ImprovementProposal
    envelope: AutonomyEnvelope


class GovernorPromotionCandidate(StrictModel):
    schema_id: Literal["rampage.promotion-candidate.v1"] = Field(
        default="rampage.promotion-candidate.v1", alias="schema"
    )
    proposal_id: UUID
    project_id: UUID
    base_revision: NonEmpty
    candidate_digest: Sha256Digest
    changed_paths: tuple[NonEmpty, ...]
    risk: RiskClass
    gates: tuple[EvidenceGate, ...]
    requested_at: datetime
    expires_at: datetime


class PromotionDecision(StrictModel):
    schema_id: Literal["rampage.promotion-decision.v1"] = Field(
        default="rampage.promotion-decision.v1", alias="schema"
    )
    proposal_id: UUID
    envelope_id: UUID
    decision: Literal["eligible", "denied"]
    reason: NonEmpty
    missing_gates: tuple[NonEmpty, ...] = ()
    per_change_approval_required: Literal[False] = False
    signed_by_governor: bool = False
    governor_candidate: GovernorPromotionCandidate | None = None


class ExperimentRecord(StrictModel):
    schema_id: Literal["rampage.experiment-record.v1"] = Field(
        default="rampage.experiment-record.v1", alias="schema"
    )
    experiment_id: UUID = Field(default_factory=uuid4)
    project_id: UUID
    base_revision: NonEmpty
    candidate_digest: NonEmpty
    preregistration_digest: NonEmpty
    evaluation_digest: NonEmpty
    baseline_metrics: dict[str, float]
    candidate_metrics: dict[str, float]
    evaluator_version: NonEmpty
    holdout_id: str | None = None
    created_at: datetime = Field(default_factory=lambda: datetime.now(UTC))


class StoredExperiment(StrictModel):
    digest: NonEmpty
    record: ExperimentRecord


class ToolDecision(StrictModel):
    allowed: bool
    reason: NonEmpty
    normalized_network_allowlist: tuple[str, ...] = ()
