from datetime import UTC, datetime

import pytest

from rampage_intelligence.dumbmoney import (
    ProposalArtifact,
    ProposalEnvelope,
    accept_telemetry,
    canonical_digest,
)


def telemetry() -> dict[str, object]:
    payload: dict[str, object] = {
        "schema": "rampage.dumbmoney.telemetry-bundle.v1",
        "producer": "dumbmoney",
        "cycle_id": "cycle-1",
        "observed_at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        "cells": [
            {"name": "waterboy", "state": "ready", "queued_work": 3, "authority": "sports"}
        ],
        "resource_demands": [
            {
                "work_id": "eval-1",
                "adapter": "rampage.eval-shard.v1",
                "restart_tolerant": True,
                "priority": 50,
            }
        ],
        "evidence_refs": [],
    }
    payload["digest"] = canonical_digest(payload)
    return payload


def test_telemetry_is_fail_closed_on_tamper() -> None:
    payload = telemetry()
    assert accept_telemetry(payload).cycle_id == "cycle-1"
    payload["cycle_id"] = "changed"
    with pytest.raises(ValueError, match="digest mismatch"):
        accept_telemetry(payload)


def test_proposal_cannot_be_constructed_with_authority() -> None:
    with pytest.raises(ValueError):
        ProposalEnvelope(
            cycle_id="cycle-1",
            artifact=ProposalArtifact(
                digest="sha256:" + "1" * 64,
                size_bytes=12,
                media_type="application/json",
            ),
            summary="bounded compute proposal",
            execution_authority=True,
            signature="signed",
        )
