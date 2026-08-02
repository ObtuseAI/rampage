from uuid import uuid4

import pytest
from fastapi.testclient import TestClient

from rampage_intelligence.api import app


def test_api_advertises_proposal_only_deterministic_mode(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("RAMPAGE_ENABLE_MODELS", "false")
    monkeypatch.setenv("RAMPAGE_TOKEN", "intelligence-only")
    with TestClient(app) as client:
        health = client.get("/health")
        assert health.status_code == 200
        assert health.json()["auth_configured"] is True
        assert health.json()["authority"] == "proposal_only"
        response = client.post(
            "/v1/goals",
            headers={"x-rampage-token": "intelligence-only"},
            json={
                "project_id": str(uuid4()),
                "principal_id": str(uuid4()),
                "objective": "Explain available local capacity",
            },
        )
        assert response.status_code == 200
        assert response.json()["capability_state"] == "deterministic_only"


def test_missing_token_fails_closed(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("RAMPAGE_TOKEN", raising=False)
    with TestClient(app) as client:
        health = client.get("/health")
        assert health.status_code == 200
        assert health.json()["status"] == "blocked"
        assert health.json()["auth_configured"] is False
        denied = client.post(
            "/v1/goals",
            json={
                "project_id": str(uuid4()),
                "principal_id": str(uuid4()),
                "objective": "This must not run without authentication",
            },
        )
        assert denied.status_code == 503


def test_packaged_token_protects_intelligence_work(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("RAMPAGE_TOKEN", "local-secret")
    with TestClient(app) as client:
        assert client.get("/health").status_code == 200
        denied = client.post(
            "/v1/goals",
            json={
                "project_id": str(uuid4()),
                "principal_id": str(uuid4()),
                "objective": "Explain available local capacity",
            },
        )
        assert denied.status_code == 401
