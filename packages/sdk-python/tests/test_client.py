import httpx
import pytest
from rampage_sdk import RampageClient


def test_client_only_accepts_loopback_control_plane() -> None:
    with pytest.raises(ValueError, match="loopback"):
        RampageClient("https://control.example.com")
    RampageClient()


def test_artifact_upload_preserves_binary_content_and_token() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["x-rampage-token"] == "local-token"
        assert request.url.path == "/v1/artifacts/put"
        assert b'"data_base64":"AAH/"' in request.content
        return httpx.Response(
            201,
            json={
                "schema": "rampage.artifact-ref.v1",
                "digest": "sha256:test",
                "size_bytes": 3,
                "media_type": "application/octet-stream",
                "storage_class": "cache",
                "encrypted": True,
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    result = client.put_artifact(bytes([0, 1, 255]))
    assert result["digest"] == "sha256:test"


def test_topology_uses_the_token_protected_offer_route() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["x-rampage-token"] == "local-token"
        assert request.url.path == "/v1/offers"
        return httpx.Response(200, json=[])

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    assert client.topology() == []


def test_model_session_planning_is_exposed_as_preview_only() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["x-rampage-token"] == "local-token"
        assert request.url.path == "/v1/model-sessions/plan"
        return httpx.Response(
            200,
            json={
                "schema": "rampage.model-session-plan.v1",
                "execution_authority": "none_preview_only",
                "state": "qualification_required",
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    result = client.plan_model_session(
        {
            "schema": "rampage.model-session-request.v1",
            "strategy": "maximum_model_size",
        }
    )
    assert result["execution_authority"] == "none_preview_only"


def test_shard_set_plan_and_status_use_bounded_routes() -> None:
    requests: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(request.url.path)
        if request.url.path.endswith("/plan"):
            return httpx.Response(
                200,
                json={
                    "schema": "rampage.shard-set-plan.v1",
                    "set_id": "set-1",
                    "admissible": True,
                    "all_or_nothing": True,
                    "placements": [],
                    "mutated": False,
                },
            )
        return httpx.Response(
            200,
            json={
                "schema": "rampage.shard-set-status.v1",
                "set_id": "set-1",
                "status": "running",
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    assert client.plan_shard_set({"schema": "rampage.shard-set.v1"})["admissible"]
    assert client.shard_set_status("set-1")["status"] == "running"
    assert requests == ["/v1/shard-sets/plan", "/v1/shard-sets/set-1"]
