from __future__ import annotations

from collections.abc import Iterable
from typing import Literal

from .models import (
    CapabilityState,
    CompiledIntent,
    GoalIntent,
    StageProposal,
    VerificationFinding,
    VerificationReport,
    WorkflowResult,
)

FORBIDDEN_AUTHORITY_PHRASES = (
    "disable governor",
    "bypass policy",
    "expose credential",
    "live trade",
    "transfer funds",
    "delete evidence",
)


def compile_intent(intent: GoalIntent, models_available: bool) -> CompiledIntent:
    objective_lower = intent.objective.casefold()
    blocked = tuple(
        phrase for phrase in FORBIDDEN_AUTHORITY_PHRASES if phrase in objective_lower
    )
    criteria = intent.success_criteria or (
        "Requested output exists",
        "Deterministic verification reports no failed check",
        "Every external effect has a receipt",
    )
    if blocked:
        state = CapabilityState.BLOCKED
    elif models_available and intent.allow_models:
        state = CapabilityState.FULL
    else:
        state = CapabilityState.DETERMINISTIC_ONLY
    return CompiledIntent(
        goal_id=intent.goal_id,
        objective=intent.objective,
        acceptance_checks=criteria,
        constraints=intent.constraints,
        blocked_authority_requests=blocked,
        capability_state=state,
    )


def deterministic_plan(compiled: CompiledIntent) -> tuple[StageProposal, ...]:
    if compiled.capability_state is CapabilityState.BLOCKED:
        return (
            StageProposal(
                stage="intent_compiler",
                summary="The request crosses a protected authority boundary.",
                claims=("No execution capabilities were requested",),
                uncertainties=compiled.blocked_authority_requests,
                requires_human_review=True,
            ),
        )
    return (
        StageProposal(
            stage="intent_compiler",
            summary=f"Compiled objective: {compiled.objective}",
            claims=(f"{len(compiled.acceptance_checks)} acceptance checks are explicit",),
        ),
        StageProposal(
            stage="planner",
            summary="Decompose into bounded discovery, implementation, and verification slices.",
            claims=("Each effect must be requested through a typed capability",),
            uncertainties=("Repository-specific constraints require inspection",),
        ),
        StageProposal(
            stage="researchers",
            summary="Gather independent architecture and evidence perspectives in parallel.",
            claims=("Research output is untrusted input to later verification",),
        ),
        StageProposal(
            stage="builder",
            summary="Prepare isolated changes and content-addressed artifacts.",
            claims=("No protected path may be modified by model authority",),
            artifacts_expected=("patch", "test_receipts", "provenance"),
        ),
        StageProposal(
            stage="critic_adversary",
            summary="Independently search for correctness, security, and evidence failures.",
            claims=("Criticism cannot self-certify a result",),
        ),
        StageProposal(
            stage="evolver",
            summary="Propose a smaller or stronger candidate only after baseline measurement.",
            claims=("A mutation without sealed evidence is not promotable",),
        ),
        StageProposal(
            stage="auditor_synthesizer",
            summary="Assemble evidence and request a Governor decision.",
            claims=("The synthesizer has proposal authority only",),
            requires_human_review=True,
        ),
    )


def verify_plan(compiled: CompiledIntent, stages: Iterable[StageProposal]) -> VerificationReport:
    stages = tuple(stages)
    findings = (
        VerificationFinding(
            check="intent_not_blocked",
            passed=not compiled.blocked_authority_requests,
            evidence=(
                "No protected-authority phrase detected"
                if not compiled.blocked_authority_requests
                else ", ".join(compiled.blocked_authority_requests)
            ),
        ),
        VerificationFinding(
            check="proposal_has_stages",
            passed=bool(stages),
            evidence=f"{len(stages)} typed stage proposals",
        ),
        VerificationFinding(
            check="ai_has_no_execution_authority",
            passed=all(not stage.capability_requests for stage in stages),
            evidence="No capability lease is embedded in an AI proposal",
        ),
    )
    missing = tuple(finding.check for finding in findings if not finding.passed)
    return VerificationReport(
        passed=not missing,
        findings=findings,
        missing_evidence=missing,
    )


def run_deterministic(intent: GoalIntent) -> WorkflowResult:
    compiled = compile_intent(intent, models_available=False)
    stages = deterministic_plan(compiled)
    verification = verify_plan(compiled, stages)
    if compiled.capability_state is CapabilityState.BLOCKED:
        action: Literal["request_leases", "human_review", "blocked"] = "blocked"
        explanation = "Protected authority request denied before planning."
    elif verification.passed:
        action = "human_review"
        explanation = "Deterministic plan is ready for review; no model or external tool was used."
    else:
        action = "blocked"
        explanation = "Deterministic verification failed closed."
    return WorkflowResult(
        goal_id=intent.goal_id,
        capability_state=compiled.capability_state,
        stages=stages,
        verification=verification,
        governor_action=action,
        explanation=explanation,
    )
