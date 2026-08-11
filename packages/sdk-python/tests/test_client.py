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


def test_protected_storage_repair_stays_inside_autonomous_envelope() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/v1/artifacts/repair"
        assert request.method == "POST"
        assert request.headers["x-rampage-token"] == "local-token"
        return httpx.Response(
            200,
            json={
                "schema": "rampage.protected-storage-reconciliation.v1",
                "status": "reconciled",
                "fresh_replica_receipts": 2,
                "per_change_approval_required": False,
                "authority_expansion": "denied",
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    result = client.repair_protected_artifacts()
    assert result["fresh_replica_receipts"] == 2
    assert result["authority_expansion"] == "denied"


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


def test_node_revocation_is_exact_and_token_protected() -> None:
    node_id = "0198f1aa-9f18-7dc3-81a3-d78f22efb662"

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["x-rampage-token"] == "local-token"
        assert request.url.path == f"/v1/nodes/{node_id}/revoke"
        assert request.content == f'{{"confirmation":"FORGET {node_id}"}}'.encode()
        return httpx.Response(
            200,
            json={
                "schema": "rampage.node-revocation-receipt.v1",
                "node_id": node_id,
                "revoked": True,
                "remote_assist_sessions_closed": 0,
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    assert client.revoke_node(node_id)["revoked"] is True


def test_capability_inventory_and_self_scan_are_token_protected() -> None:
    requests: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["x-rampage-token"] == "local-token"
        requests.append(request.url.path)
        if request.url.path.endswith("workload-capabilities"):
            return httpx.Response(
                200,
                json={
                    "schema": "rampage.workload-capability-inventory.v1",
                    "candidate_authority": False,
                    "nodes": [],
                },
            )
        if request.url.path.endswith("relay-access"):
            return httpx.Response(
                200,
                json={
                    "schema": "rampage.relay-access-manifest.v1",
                    "fabric_id": f"sha256:{'a' * 64}",
                    "generation": 2,
                    "allowed_endpoint_ids": ["b" * 64],
                    "signature": "signed",
                },
            )
        return httpx.Response(
            200,
            json={
                "schema": "rampage.fabric-diagnostic-report.v1",
                "autonomy": {"per_change_approval_required": False},
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    assert not client.workload_capabilities()["candidate_authority"]
    assert not client.self_scan()["autonomy"]["per_change_approval_required"]
    assert client.relay_access_manifest()["generation"] == 2
    assert requests == [
        "/v1/workload-capabilities",
        "/v1/diagnostics/self-scan",
        "/v1/mesh/relay-access",
    ]


def test_promotion_canary_forwards_the_complete_candidate_to_the_governor() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["x-rampage-token"] == "local-token"
        assert request.url.path == "/v1/improvements/canary"
        assert b'"candidate_digest":"sha256:' in request.content
        return httpx.Response(
            201,
            json={
                "schema": "rampage.promotion-canary-lease.v1",
                "canary_id": "canary-1",
                "signature": "signed",
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    lease = client.authorize_promotion_canary(
        {
            "schema": "rampage.promotion-candidate.v1",
            "candidate_digest": f"sha256:{'a' * 64}",
        }
    )
    assert lease["signature"] == "signed"


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


def test_dividend_break_even_and_network_autopilot_are_exposed() -> None:
    requests: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(str(request.url))
        if request.url.path == "/v1/dividends":
            assert request.url.params["limit"] == "250"
            return httpx.Response(200, json=[])
        if request.url.path == "/v1/plans/break-even":
            assert b'"restart_tolerant":true' in request.content
            return httpx.Response(
                200,
                json={
                    "schema": "rampage.break-even-plan.v1",
                    "decision": "insufficient_evidence",
                },
            )
        assert request.url.path == "/v1/network/autopilot"
        return httpx.Response(
            200,
            json={
                "schema": "rampage.network-autopilot-status.v1",
                "mode": "automatic_evidence_gated",
                "nodes": [],
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    assert client.dividend_history(999) == []
    assert client.plan_break_even(
        {
            "schema": "rampage.break-even-request.v1",
            "workload_class": "build_test",
            "fastest_node_compute_ms": 60_000,
            "input_bytes": 0,
            "output_bytes": 0,
            "startup_ms": 0,
            "restart_tolerant": True,
            "minimum_gain_percent": 12,
        }
    )["decision"] == "insufficient_evidence"
    assert client.network_autopilot()["mode"] == "automatic_evidence_gated"
    assert len(requests) == 3


def test_openai_gateway_config_and_models_use_bearer_auth() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/v1/models"
        assert request.headers["authorization"] == "Bearer local-token"
        return httpx.Response(
            200,
            json={
                "object": "list",
                "data": [{"id": "gemma3:4b", "object": "model"}],
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    assert client.openai_config() == {
        "base_url": "http://127.0.0.1:47831/v1",
        "api_key": "local-token",
    }
    assert client.models()["data"][0]["id"] == "gemma3:4b"


def test_anthropic_and_openrouter_configs_share_the_bounded_gateway() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/v1/messages"
        assert request.headers["authorization"] == "Bearer local-token"
        return httpx.Response(
            200,
            json={
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "hello"}],
                "model": "gemma3:4b",
                "stop_reason": "end_turn",
                "stop_sequence": None,
                "usage": {"input_tokens": 1, "output_tokens": 1},
            },
        )

    client = RampageClient(token="local-token")
    client._client.close()
    client._client = httpx.Client(
        base_url="http://127.0.0.1:47831",
        headers={"x-rampage-token": "local-token"},
        transport=httpx.MockTransport(handler),
    )
    assert client.anthropic_config()["base_url"] == "http://127.0.0.1:47831"
    assert client.openrouter_config()["base_url"] == "http://127.0.0.1:47831/api/v1"
    response = client.anthropic_message(
        {
            "model": "gemma3:4b",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hello"}],
        }
    )
    assert response["content"][0]["text"] == "hello"


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
