from rampage_intelligence.models import CapabilityRequest
from rampage_intelligence.tool_broker import evaluate_tool_request


def test_tool_broker_normalizes_https_origins() -> None:
    decision = evaluate_tool_request(
        CapabilityRequest(
            adapter="rampage.hash.v1",
            operation="hash",
            network_allowlist=("https://example.com/path", "https://example.com/other"),
            reason="test",
        )
    )
    assert decision.allowed
    assert decision.normalized_network_allowlist == ("https://example.com",)


def test_tool_broker_rejects_credentials_and_unknown_tools() -> None:
    credentialed = evaluate_tool_request(
        CapabilityRequest(
            adapter="rampage.hash.v1",
            operation="hash",
            network_allowlist=("https://token@example.com",),
            reason="test",
        )
    )
    assert not credentialed.allowed
    unknown = evaluate_tool_request(
        CapabilityRequest(adapter="shell", operation="run", reason="test")
    )
    assert not unknown.allowed
