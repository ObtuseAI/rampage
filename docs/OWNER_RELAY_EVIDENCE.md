# Owner-relay milestone evidence

Validated: **2026-08-01 America/Chicago**

Status: **PASS for source, forced-relay transport, and local process qualification; not a public-WAN or newly signed binary qualification**

| Gate | Evidence | Result |
| --- | --- | --- |
| Rust workspace | `cargo test --workspace --all-features` | PASS — 85 tests |
| Rust quality | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| Forced relay | `owner_relay_carries_quic_when_ip_transports_are_disabled` | PASS — authenticated QUIC payload crossed the owner relay with direct IP and address lookup disabled |
| Relay admission | Protocol, Governor, relay library, and CLI tests | PASS — short-lived fabric-bound signature, endpoint-exact allowlist, tamper/expiry denial, pre-token loopback validation |
| Resource exhaustion | Relay and controller bounds | PASS — bounded regular files, streaming one-MiB HTTP limits, per-client rate/burst, per-endpoint and total connections, bounded key cache, loopback metrics |
| Worker relay import | `signed_controller_relays_are_the_only_worker_relay_candidates` | PASS — only URLs in the verified controller endpoint record enter worker mesh configuration |
| Desktop lifecycle | Rust desktop tests plus production desktop build | PASS — bundled relay sidecar, configuration screening, desktop-owned launch and shutdown |
| Consolidated product campaign | `scripts/Test-Rampage.ps1 -SkipOllama` | PASS |
| Desktop UI | Vitest and production Vite build | PASS — 5 tests |
| TypeScript SDK | Vitest and TypeScript build | PASS — 10 tests |
| Python SDK | Pytest with local source import | PASS — 9 tests |
| Intelligence plane | Ruff, strict Mypy, and Pytest | PASS — 16 tests |
| Local execution/restart | `scripts/e2e.ps1` | PASS — receipt, pooled shards, restart recovery, STOP fencing, tokenless denial |
| Authenticated mesh/storage | `scripts/mesh-e2e.ps1` | PASS — measured direct QUIC, encrypted replica, staging, receipt, retrievable output |
| Universal gateway | `scripts/model-gateway-e2e.ps1` | PASS — signed owner-relay access included |
| Installed local model | `scripts/ollama-e2e.ps1` with `llama3.2:latest` | PASS |
| JavaScript dependencies | `pnpm audit --audit-level high` | PASS — no known vulnerabilities |
| Rust dependencies | `cargo audit` over 739 locked crates | PASS WITH WARNINGS — no published vulnerability; 18 tracked maintenance/unsoundness warnings |

The retained ignored process campaigns for this source state are:

- `output/e2e-8dc7d6a8b77b4ee6bd6ff1c847b06e67`
- `output/mesh-e2e-d6c8d7c6255b4b5480f8b77653801cf7`
- `output/ollama-e2e-c4781c6e086449cd9c6f73c778b28a2e` — deterministic fake Ollama
- `output/ollama-e2e-de0563f5779d46769824863a337a7b00` — installed Ollama `llama3.2:latest`

## What the forced-relay proof establishes

Two independently keyed Iroh endpoints are created with all direct IP transports and address
lookup disabled. Their only configured transport is an in-process `rampage-relay` protected by a
fresh Governor-signed endpoint allowlist. The peers exchange `RAMPAGE_RELAY_OK` over authenticated
QUIC. If either endpoint is omitted from the signed manifest, or if that manifest is stale or
tampered, the relay denies the connection.

The controller exposes relay admission only through its token-protected loopback API. The manifest
contains the exact controller and enrolled endpoint identities, is bound to the Governor-derived
fabric digest and durable fencing generation, expires within ten minutes, and is never treated as
execution authority. Worker execution still requires its separate operation-exact signed lease.

## Integration defects found and closed

- Autonomous diagnostics had globally suppressed a valid local polling worker because it had no
  mesh endpoint. Local polling is now an explicit restricted lane; mesh-only operations still need
  a signed endpoint, and remote mesh offers without one are denied.
- The mesh campaign reused the receipt event snapshot before the immediately following output event
  was committed. It now polls for the complete durable transfer evidence set.
- Chunked controller and relay responses are bounded while streaming, before aggregation; oversized
  token, manifest, configuration, and response inputs fail closed.

## Evidence boundary

This milestone proves a real owner-relay data path and its local authority/resource boundaries. It
does not prove reachability through every NAT, a public DNS/TLS deployment, sustained multi-user
throughput, relay high availability, Linux/macOS packaging, or an Authenticode-signed installer.
Direct authenticated QUIC remains preferred. A hard-NAT deployment still needs an owner-controlled
public HTTPS route; no application can manufacture inbound Internet reachability where none exists.

The RustSec result is unchanged from the prior milestone: no vulnerability failure and 18 tracked
warnings in GTK3/`glib`, `paste`, `proc-macro-error`, and `unic-*` transitive paths. The relay did
not add a new advisory. The existing Windows installers remain unsigned and do not contain this
milestone until a later distribution release is built and qualified.
