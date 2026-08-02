# Rampage 0.2.1 pairing-candidate evidence

Validated: **2026-08-02 15:04 America/Chicago**
Status: **PASS as an unsigned Windows 11 x64 prerelease candidate**
Release channel: **`v0.2.1-pairing.1`**

## Gates

| Gate | Evidence | Result |
| --- | --- | --- |
| Nearby-pairing protocol | X25519/HKDF/AES-GCM; four-digit transcript code; explicit expiry; source/payload/pending bounds; authenticated completion | PASS |
| Network-level pairing | Real loopback UDP discovery → challenge → matching code → encrypted approval → authenticated completion | PASS |
| Pairing UX | Zero-copy laptop flow, waiting state, readable code, single owner approval, completion state, advanced fallback hidden by default | PASS |
| Packaged Arena | Production bundle contains local canvas labels and no external font reference; native WebView visually renders node/grid/label | PASS |
| Arena recovery/accessibility | Four-second escape, render error boundary, WebGL capability fallback, no false hidden-fallback announcement | PASS |
| Rust | Formatting clean; workspace tests green; full-workspace/all-target Clippy with warnings denied | PASS |
| Desktop/edge/SDK | 12 desktop tests; 2 edge tests; 11 TypeScript SDK tests; all production builds | PASS |
| Python intelligence/SDK | Ruff clean; strict Mypy clean; 17 intelligence tests; 10 Python SDK tests | PASS |
| Supply chain | RustSec baseline: 0 vulnerabilities and 18 reviewed warnings; `pnpm audit --prod --audit-level high`: 0 known vulnerabilities; `pip-audit`: 0 known vulnerabilities | PASS |
| Controller campaign | Signed leases/receipts, three-shard threshold and restart recovery, local STOP/resume, durable fencing, stale authority and tokenless request denied | PASS |
| Mesh/storage campaign | Authenticated direct QUIC, measured topology, encrypted-at-rest round trip, resumable chunks, independent replicas, signed possession, autonomous repair | PASS |
| Universal model gateway | OpenAI non-streaming/SSE, Anthropic Messages, OpenRouter paths, exact capability discovery, signed receipt, tokenless denial | PASS |
| NSIS lifecycle | Six payloads; install/uninstall exit 0; desktop + shell shortcuts; controller/intelligence/node/offer ready; no sidecar leak | PASS |
| MSI structure | Administrative extraction exit 0; six payloads; generated `ApplicationShortcutDesktop` | PASS |
| Installed main PC | Product version 0.2.1; controller/intelligence ready; one node and one signed offer; desktop shortcut present | PASS |

Process evidence produced during the final source campaign includes:

- `output/e2e-4011331c1a644f748090c4f3cf831674`
- `output/mesh-e2e-7c8560f77a8040989436fd63390432df`
- `output/ollama-e2e-1094797b01f34cdeb90c3fc16fde13e5`

The model-gateway campaign uses a deterministic loopback Ollama fixture to prove translation,
authorization, streaming, authenticated worker transport, exact model/runtime binding, and signed
receipt behavior. A physical local Ollama generation was not rerun for this candidate because the
installed Ollama service was not listening during the campaign; no physical-inference performance
claim is made.

## Release artifacts

| Artifact | Bytes | SHA-256 | Authenticode |
| --- | ---: | --- | --- |
| `Rampage_0.2.1_x64-setup.exe` | 69,066,549 | `d81ae34fecd7c2b7faeeb3542cc0e3d5092852fa5f3765c127f160ee2a3c7a2b` | Not signed |
| `Rampage_0.2.1_x64_en-US.msi` | 78,864,384 | `af2ae59db323dec6c8ae9dfb3aec1b8f8eeb873e2e42da62db3311822732df95` | Not signed |

The checksums are duplicated in [`SHA256SUMS-0.2.1.txt`](SHA256SUMS-0.2.1.txt) and must match the
assets attached to the GitHub prerelease.

## Remaining qualification boundaries

- The real laptop is the next physical-device gate. Automated proof cannot substitute for the
  actual two-screen code comparison, Windows private-network behavior, restart, and signed offer.
- The phone is a separate native Rampage Edge gate after laptop pairing; this candidate does not
  claim a physically paired phone.
- Windows 10, signed Authenticode reputation, macOS notarization, and Linux stable packaging remain
  separate platform gates.
- Arbitrary applications cannot borrow another machine's address space. Useful pooling remains
  workload-aware: whole jobs, independent shards, supported engine-native distribution, caches,
  and encrypted artifacts.
