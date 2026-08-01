from __future__ import annotations

import base64
import time
from typing import Any
from urllib.parse import urlparse

import httpx


class RampageClient:
    def __init__(
        self, base_url: str = "http://127.0.0.1:47831", token: str | None = None
    ) -> None:
        parsed = urlparse(base_url)
        if parsed.hostname not in {"127.0.0.1", "localhost"}:
            raise ValueError("Rampage SDK only connects to the loopback controller API")
        headers = {"x-rampage-token": token} if token else None
        self._client = httpx.Client(
            base_url=base_url.rstrip("/"), timeout=30.0, headers=headers
        )

    def health(self) -> dict[str, Any]:
        return self._get("/health")

    def invite(self) -> dict[str, Any]:
        return self._post("/v1/enrollment/invites", {})

    def discover(self, path: str) -> dict[str, Any]:
        return self._post("/v1/projects/discover", {"path": path})

    def plan(self, job: dict[str, Any]) -> dict[str, Any]:
        return self._post("/v1/jobs/plan", job)

    def topology(self) -> list[dict[str, Any]]:
        response = self._client.get("/v1/offers")
        response.raise_for_status()
        payload: list[dict[str, Any]] = response.json()
        return payload

    def workload_capabilities(self) -> dict[str, Any]:
        """Return operation-exact capabilities from verified live signed offers."""
        return self._get("/v1/workload-capabilities")

    def self_scan(self) -> dict[str, Any]:
        """Run and return the deterministic fabric bottleneck and failure scan."""
        return self._get("/v1/diagnostics/self-scan")

    def relay_access_manifest(self) -> dict[str, Any]:
        """Return the fresh Governor-signed endpoint allowlist for an owner relay."""
        return self._get("/v1/mesh/relay-access")

    def authorize_promotion_canary(
        self, candidate: dict[str, Any]
    ) -> dict[str, Any]:
        """Request a signed, evidence-gated canary lease from the Rust Governor."""
        return self._post("/v1/improvements/canary", candidate)

    def plan_model_session(self, request: dict[str, Any]) -> dict[str, Any]:
        """Preview a model placement; this endpoint never issues execution authority."""
        return self._post("/v1/model-sessions/plan", request)

    def openai_config(self) -> dict[str, str]:
        """Return values accepted by the official OpenAI client for the local Rampage gateway."""
        token = self._client.headers.get("x-rampage-token")
        if not token:
            raise ValueError("Rampage gateway requires the local controller token")
        return {
            "base_url": f"{str(self._client.base_url).rstrip('/')}/v1",
            "api_key": token,
        }

    def openrouter_config(self) -> dict[str, str]:
        """Return values accepted by OpenAI-compatible clients using OpenRouter paths."""
        token = self._client.headers.get("x-rampage-token")
        if not token:
            raise ValueError("Rampage gateway requires the local controller token")
        return {
            "base_url": f"{str(self._client.base_url).rstrip('/')}/api/v1",
            "api_key": token,
        }

    def anthropic_config(self) -> dict[str, str]:
        """Return values accepted by the official Anthropic client."""
        token = self._client.headers.get("x-rampage-token")
        if not token:
            raise ValueError("Rampage gateway requires the local controller token")
        return {"base_url": str(self._client.base_url).rstrip("/"), "api_key": token}

    def gateway_capabilities(self) -> dict[str, Any]:
        return self._gateway_get("/v1/capabilities")

    def models(self) -> dict[str, Any]:
        return self._gateway_get("/v1/models")

    def chat_completion(self, request: dict[str, Any]) -> dict[str, Any]:
        if request.get("stream") is True:
            raise ValueError("use an OpenAI client with openai_config() for SSE streaming")
        return self._gateway_post("/v1/chat/completions", request)

    def anthropic_message(self, request: dict[str, Any]) -> dict[str, Any]:
        if request.get("stream") is True:
            raise ValueError("use an Anthropic client with anthropic_config() for SSE streaming")
        return self._gateway_post("/v1/messages", request)

    def cancel_model_session(self, session_id: str) -> dict[str, Any]:
        return self._gateway_post(f"/v1/model-sessions/{session_id}/cancel", {})

    def plan_shard_set(self, shard_set: dict[str, Any]) -> dict[str, Any]:
        return self._post("/v1/shard-sets/plan", shard_set)

    def run_shard_set(self, shard_set: dict[str, Any]) -> dict[str, Any]:
        return self._post("/v1/shard-sets", shard_set)

    def shard_set_status(self, set_id: str) -> dict[str, Any]:
        return self._get(f"/v1/shard-sets/{set_id}")

    def run(self, job: dict[str, Any]) -> dict[str, Any]:
        return self._post("/v1/jobs", job)

    def receipts(self, job_id: str) -> list[dict[str, Any]]:
        response = self._client.get("/v1/receipts", params={"job_id": job_id})
        response.raise_for_status()
        payload: list[dict[str, Any]] = response.json()
        return payload

    def put_artifact(
        self,
        payload: bytes,
        media_type: str = "application/octet-stream",
        storage_class: str = "cache",
    ) -> dict[str, Any]:
        return self._post(
            "/v1/artifacts/put",
            {
                "data_base64": base64.b64encode(payload).decode("ascii"),
                "media_type": media_type,
                "storage_class": storage_class,
            },
        )

    def get_artifact(self, digest: str) -> bytes:
        response = self._client.get("/v1/artifacts/get", params={"digest": digest})
        response.raise_for_status()
        return base64.b64decode(response.json()["data_base64"], validate=True)

    def replicate_artifact(
        self,
        digest: str,
        node_id: str,
        media_type: str = "application/octet-stream",
        storage_class: str = "cache",
    ) -> dict[str, Any]:
        return self._post(
            "/v1/artifacts/replicate",
            {
                "digest": digest,
                "node_id": node_id,
                "media_type": media_type,
                "storage_class": storage_class,
            },
        )

    def retrieve_artifact(self, digest: str, node_id: str) -> bytes:
        response = self._post(
            "/v1/artifacts/retrieve", {"digest": digest, "node_id": node_id}
        )
        return base64.b64decode(response["data_base64"], validate=True)

    def wait_for_receipt(self, job_id: str, timeout_seconds: float = 120.0) -> dict[str, Any]:
        deadline = time.monotonic() + timeout_seconds
        while time.monotonic() < deadline:
            receipts = self.receipts(job_id)
            if receipts:
                return receipts[-1]
            time.sleep(0.25)
        raise TimeoutError(f"Rampage job {job_id} did not finish before the deadline")

    def events(self, after: int = 0) -> list[dict[str, Any]]:
        response = self._client.get("/v1/events", params={"after": after})
        response.raise_for_status()
        payload: list[dict[str, Any]] = response.json()
        return payload

    def stop(self) -> dict[str, Any]:
        return self._post("/v1/stop", {})

    def resume(self) -> dict[str, Any]:
        return self._post("/v1/resume", {"confirmation": "OWNER_RESUME"})

    def _get(self, path: str) -> dict[str, Any]:
        response = self._client.get(path)
        response.raise_for_status()
        payload: dict[str, Any] = response.json()
        return payload

    def _post(self, path: str, body: dict[str, Any]) -> dict[str, Any]:
        response = self._client.post(path, json=body)
        response.raise_for_status()
        payload: dict[str, Any] = response.json()
        return payload

    def _gateway_headers(self) -> dict[str, str]:
        token = self._client.headers.get("x-rampage-token")
        if not token:
            raise ValueError("Rampage gateway requires the local controller token")
        return {"authorization": f"Bearer {token}"}

    def _gateway_get(self, path: str) -> dict[str, Any]:
        response = self._client.get(path, headers=self._gateway_headers())
        response.raise_for_status()
        payload: dict[str, Any] = response.json()
        return payload

    def _gateway_post(self, path: str, body: dict[str, Any]) -> dict[str, Any]:
        response = self._client.post(path, json=body, headers=self._gateway_headers())
        response.raise_for_status()
        payload: dict[str, Any] = response.json()
        return payload
