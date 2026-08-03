# Rampage 0.2.3 fabric-proof evidence

Validated: **2026-08-03 America/Chicago**

Status: **PASS as an unsigned Windows 11 x64 prerelease candidate**

Release channel: **`v0.2.3-fabric-proof.1`**

## Gates

| Gate | Evidence | Result |
| --- | --- | --- |
| Version coherence | `Assert-RampageVersion.ps1 -Tag v0.2.3` inspected nine version surfaces | PASS |
| Full repository | `Test-Rampage.ps1 -SkipOllama` completed the Rust workspace tests, clippy with warnings denied, desktop/edge/SDK tests and builds, Python lint/type/tests, two-worker mesh/storage/repair, and deterministic OpenAI/Anthropic/OpenRouter model gateway | PASS |
| Packaged owner | Isolated native owner produced controller, proposal-only intelligence, one enrolled node, one signed offer, and an authenticated owner mesh endpoint; close-to-tray and explicit cleanup passed | PASS |
| Packaged worker | Signed enrollment, authenticated direct QUIC, encrypted artifact round trip, signed sustained benchmark, consumed-invite removal, durable pin restart, tray lifecycle, and no leaked sidecars | PASS |
| Controller restart | The packaged campaign terminated and restarted the owner controller on its durable mesh port; the running worker retained enrollment and published a fresh signed endpoint automatically | PASS |
| NSIS lifecycle | Six payloads; install/uninstall exit 0; desktop and Rampage Shell shortcuts; controller/intelligence/node/offer ready; no sidecar leak | PASS |
| Automatic local AI | Installed Ollama 0.32.5 reported `qwen3:4b`, 2,497,293,931 bytes, and exact artifact digest `sha256:359d7dd4bcdab3d86b87d73ac27966f4dbb9f5efdfcc75d34a8764a09474fae7` | PASS on main PC |
| Live signed AI path | Final packaged sidecar hash matched the installed sidecar; OpenAI-compatible request `chatcmpl-019fc949a1937a1383f8537e4547ff9d` crossed signed owner mesh and returned a complete answer with no `</think>` trace | PASS on main PC |
| AI evidence | Hash-chain events 43507 and 43521 are the matching `model.session.lease.issued` and `model.session.receipted` records for session `019fc949-a193-7a13-83f8-536abd780b5a` | PASS |
| Sustained compute | Four signed lanes completed 20,000,000 SHA-256 iterations in 418.9788 ms: 47,735,112 hashes/second; receipt `019fc94a-35f4-72b0-974c-42fae0b6e078`; result digest `sha256:c7c764e994816693bd7001608fec57b8a47711ecbd07284ac40e6afb1e894efd` | PASS on main PC |
| Lease clock skew | Regression accepts a real 500 ms future storage lease, caps positive skew at five seconds, and still rejects exact expiry and excessive skew | PASS |
| Thinking isolation | Ollama request explicitly retains structured thinking; only ordinary answer content becomes model deltas and the signed transcript | PASS by regression and live final package |

The final packaged worker evidence directory was
`output/worker-desktop-smoke-69d4b82cf2384b00a827f3d26a3f9b63`. The final installer evidence
directory was `output/nsis-install-f5d85a7530f44626a62e9f4fb6e2b44d`. These are local process
evidence paths, not portable release assets.

## Release artifacts

| Artifact | Bytes | SHA-256 | Authenticode |
| --- | ---: | --- | --- |
| `Rampage_0.2.3_x64-setup.exe` | 69,200,646 | `015cb0172ac655a6e1ab7a278481c67092a9bd591577bc32a2c7930536b41f34` | Not signed |
| `Rampage_0.2.3_x64_en-US.msi` | 79,060,992 | `0a5a6ecba49d3c5b3e5123d7b6c2266c5e44e9f8a7eec4483b502180edf30ed1` | Not signed |

The checksums are duplicated in [`SHA256SUMS-0.2.3.txt`](SHA256SUMS-0.2.3.txt) and must match the
assets attached to the GitHub prerelease.

## Physical campaign boundary

- The physical Windows laptop successfully enrolled and published authenticated QUIC offers while
  running 0.2.2. It has not yet installed the exact 0.2.3 artifact above.
- Consequently, the final two-node 0.2.3 sustained benchmark, the repaired 16 MiB storage campaign,
  laptop-local Ollama qualification, and post-controller-restart physical reconnection remain the
  next interactive gate after one laptop installer run.
- The main PC is installed from the exact final NSIS artifact and its signed local model and CPU
  paths have passed.
- Phone installation and physical lifecycle qualification remain separate from the desktop fabric.
- Windows 10, Authenticode reputation, macOS notarization, Linux stable packaging, and a deployed
  hard-NAT owner relay remain separate qualification gates.
- Cross-host tensor or pipeline inference remains disabled. The release can route whole-model
  inference to one qualified contributor and can pool independent compute, but it does not claim a
  larger multi-host address space.
