from uuid import uuid4

from fastapi.testclient import TestClient

from rampage_intelligence.api import app


def test_api_advertises_proposal_only_deterministic_mode(monkeypatch) -> None:
    monkeypatch.setenv("RAMPAGE_ENABLE_MODELS", "false")
    with TestClient(app) as client:
        health = client.get("/health")
        assert health.status_code == 200
        assert health.json()["authority"] == "proposal_only"
        response = client.post(
            "/v1/goals",
            json={
                "project_id": str(uuid4()),
                "principal_id": str(uuid4()),
                "objective": "Explain available local capacity",
            },
        )
        assert response.status_code == 200
        assert response.json()["capability_state"] == "deterministic_only"


def test_packaged_token_protects_intelligence_work(monkeypatch) -> None:
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
