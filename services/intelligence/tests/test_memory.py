from pathlib import Path
from uuid import uuid4

import pytest

from rampage_intelligence.memory import ScientificMemory
from rampage_intelligence.models import ExperimentRecord


def record() -> ExperimentRecord:
    return ExperimentRecord(
        project_id=uuid4(),
        base_revision="abc123",
        candidate_digest="sha256:candidate",
        preregistration_digest="sha256:preregistered",
        evaluation_digest="sha256:evaluation",
        baseline_metrics={"quality": 0.7, "cost": 4.0},
        candidate_metrics={"quality": 0.8, "cost": 3.0},
        evaluator_version="eval-v1",
    )


def test_scientific_memory_is_content_addressed_and_reopenable(tmp_path: Path) -> None:
    path = tmp_path / "memory.sqlite"
    first = ScientificMemory(path)
    stored = first.record(record())
    first.close()
    second = ScientificMemory(path)
    assert second.get(stored.digest) == stored
    second.close()


def test_experiment_identity_cannot_be_rebound(tmp_path: Path) -> None:
    memory = ScientificMemory(tmp_path / "memory.sqlite")
    initial = record()
    memory.record(initial)
    changed = initial.model_copy(update={"candidate_digest": "sha256:other"})
    with pytest.raises(ValueError, match="already bound"):
        memory.record(changed)
    memory.close()
