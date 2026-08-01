from __future__ import annotations

import hashlib
import json
import sqlite3
from pathlib import Path

from .models import ExperimentRecord, StoredExperiment


def experiment_digest(record: ExperimentRecord) -> str:
    canonical = json.dumps(
        record.model_dump(mode="json", by_alias=True),
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return f"sha256:{hashlib.sha256(canonical).hexdigest()}"


class ScientificMemory:
    """Content-addressed, immutable experimental memory; never a promotion authority."""

    def __init__(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        self._connection = sqlite3.connect(path, check_same_thread=False)
        self._connection.execute("PRAGMA journal_mode=WAL")
        self._connection.execute(
            """
            CREATE TABLE IF NOT EXISTS experiments (
                digest TEXT PRIMARY KEY,
                experiment_id TEXT NOT NULL UNIQUE,
                project_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                record_json TEXT NOT NULL
            )
            """
        )
        self._connection.commit()

    def record(self, experiment: ExperimentRecord) -> StoredExperiment:
        digest = experiment_digest(experiment)
        payload = json.dumps(
            experiment.model_dump(mode="json", by_alias=True),
            sort_keys=True,
            separators=(",", ":"),
        )
        try:
            self._connection.execute(
                "INSERT INTO experiments VALUES (?, ?, ?, ?, ?)",
                (
                    digest,
                    str(experiment.experiment_id),
                    str(experiment.project_id),
                    experiment.created_at.isoformat(),
                    payload,
                ),
            )
            self._connection.commit()
        except sqlite3.IntegrityError:
            existing = self.get(digest)
            if existing is None or existing.record != experiment:
                raise ValueError(
                    "experiment identity or digest is already bound to other data"
                ) from None
        return StoredExperiment(digest=digest, record=experiment)

    def get(self, digest: str) -> StoredExperiment | None:
        row = self._connection.execute(
            "SELECT record_json FROM experiments WHERE digest = ?", (digest,)
        ).fetchone()
        if row is None:
            return None
        record = ExperimentRecord.model_validate_json(row[0])
        if experiment_digest(record) != digest:
            raise ValueError("scientific memory failed content-address verification")
        return StoredExperiment(digest=digest, record=record)

    def close(self) -> None:
        self._connection.close()
