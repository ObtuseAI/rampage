from __future__ import annotations

import hashlib
import json
from datetime import UTC, datetime
from typing import Literal
from uuid import UUID, uuid4

from pydantic import Field

from .models import NonEmpty, StrictModel


class CellTelemetry(StrictModel):
    name: NonEmpty
    state: Literal["ready", "working", "degraded", "blocked", "offline"]
    queued_work: int = Field(ge=0)
    authority: str


class ResourceDemand(StrictModel):
    work_id: NonEmpty
    adapter: NonEmpty
    restart_tolerant: bool
    priority: int = Field(ge=0, le=100)


class TelemetryBundle(StrictModel):
    schema_id: Literal["rampage.dumbmoney.telemetry-bundle.v1"] = Field(
        default="rampage.dumbmoney.telemetry-bundle.v1", alias="schema"
    )
    producer: Literal["dumbmoney"]
    cycle_id: NonEmpty
    observed_at: datetime
    cells: tuple[CellTelemetry, ...]
    resource_demands: tuple[ResourceDemand, ...]
    evidence_refs: tuple[str, ...] = ()
    digest: str

    def verify_digest(self) -> bool:
        payload = self.model_dump(mode="json", exclude={"digest"})
        return self.digest == canonical_digest(payload)


class ProposalArtifact(StrictModel):
    digest: str
    size_bytes: int = Field(ge=0)
    media_type: NonEmpty


class ProposalEnvelope(StrictModel):
    schema_id: Literal["rampage.proposal-envelope.v1"] = Field(
        default="rampage.proposal-envelope.v1", alias="schema"
    )
    proposal_id: UUID = Field(default_factory=uuid4)
    cycle_id: NonEmpty
    producer: Literal["rampage"] = "rampage"
    artifact: ProposalArtifact
    summary: NonEmpty
    execution_authority: Literal[False] = False
    credential_access: Literal[False] = False
    policy_mutation: Literal[False] = False
    capital_authority: Literal[False] = False
    created_at: datetime = Field(default_factory=lambda: datetime.now(UTC))
    signature: NonEmpty


def canonical_digest(payload: object) -> str:
    encoded = json.dumps(
        payload,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode()
    return f"sha256:{hashlib.sha256(encoded).hexdigest()}"


def accept_telemetry(payload: object) -> TelemetryBundle:
    bundle = TelemetryBundle.model_validate(payload)
    if not bundle.verify_digest():
        raise ValueError("DumbMoney telemetry digest mismatch")
    return bundle

