from __future__ import annotations

from urllib.parse import urlparse

from .models import CapabilityRequest, ToolDecision

ALLOWLISTED_OPERATIONS = {
    "rampage.echo.v1": frozenset({"echo"}),
    "rampage.hash.v1": frozenset({"hash"}),
    "rampage.eval-shard.v1": frozenset({"score"}),
}


def evaluate_tool_request(request: CapabilityRequest) -> ToolDecision:
    operations = ALLOWLISTED_OPERATIONS.get(request.adapter)
    if operations is None or request.operation not in operations:
        return ToolDecision(allowed=False, reason="adapter and operation are not allowlisted")
    if sum(len(key) + len(value) for key, value in request.arguments.items()) > 64 * 1024:
        return ToolDecision(allowed=False, reason="arguments exceed the broker size limit")
    normalized: list[str] = []
    for entry in request.network_allowlist:
        parsed = urlparse(entry)
        if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
            return ToolDecision(
                allowed=False,
                reason="network entries must be credential-free HTTPS origins",
            )
        origin = f"https://{parsed.hostname}"
        if parsed.port not in (None, 443):
            origin += f":{parsed.port}"
        normalized.append(origin)
    return ToolDecision(
        allowed=True,
        reason="request shape is eligible for a Rust Governor lease",
        normalized_network_allowlist=tuple(sorted(set(normalized))),
    )
