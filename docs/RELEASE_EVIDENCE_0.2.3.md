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
| Public NSIS lifecycle | The exact tagged CI asset produced six payloads; install/uninstall exit 0; desktop and Rampage Shell shortcuts; controller/intelligence/node/offer ready; no sidecar leak | PASS |
| Automatic local AI | Installed Ollama 0.32.5 reported `qwen3:4b`, 2,497,293,931 bytes, and exact artifact digest `sha256:359d7dd4bcdab3d86b87d73ac27966f4dbb9f5efdfcc75d34a8764a09474fae7` | PASS on main PC |
| Live signed AI path | Final packaged sidecar hash matched the installed sidecar; OpenAI-compatible request `chatcmpl-019fc949a1937a1383f8537e4547ff9d` crossed signed owner mesh and returned a complete answer with no `</think>` trace | PASS on main PC |
| AI evidence | Hash-chain events 43507 and 43521 are the matching `model.session.lease.issued` and `model.session.receipted` records for session `019fc949-a193-7a13-83f8-536abd780b5a` | PASS |
| Sustained compute | Four signed lanes completed 20,000,000 SHA-256 iterations in 418.9788 ms: 47,735,112 hashes/second; receipt `019fc94a-35f4-72b0-974c-42fae0b6e078`; result digest `sha256:c7c764e994816693bd7001608fec57b8a47711ecbd07284ac40e6afb1e894efd` | PASS on main PC |
| Lease clock skew | Regression accepts a real 500 ms future storage lease, caps positive skew at five seconds, and still rejects exact expiry and excessive skew | PASS |
| Thinking isolation | Ollama request explicitly retains structured thinking; only ordinary answer content becomes model deltas and the signed transcript | PASS by regression and live final package |

The final packaged worker evidence directory was
`output/worker-desktop-smoke-69d4b82cf2384b00a827f3d26a3f9b63`. The final installer evidence
directory for the source-identical local package was
`output/nsis-install-f5d85a7530f44626a62e9f4fb6e2b44d`. The exact tagged CI-built NSIS was then
downloaded from the draft release, independently matched to its generated checksum manifest, and
passed the same lifecycle campaign in `output/nsis-install-df345c3b840f434d8221250c4563b2aa`.
These are local process evidence paths, not portable release assets.

## Release artifacts

| Artifact | Bytes | SHA-256 | GitHub provenance | Authenticode |
| --- | ---: | --- | --- | --- |
| `Rampage_0.2.3_x64-setup.exe` | 68,932,326 | `5790b322d2ed6b6694af8837bf85233e947bcf049fcde6d6248bfbd130ac21a0` | Attested from tag workflow | Not signed |
| `Rampage_0.2.3_x64_en-US.msi` | 78,749,696 | `f734cbbb893e03f0a590a5ccaf1858123f8f8e12b2c7248d52084e9c5536df04` | Attested from tag workflow | Not signed |

The checksums are duplicated in [`SHA256SUMS-0.2.3.txt`](SHA256SUMS-0.2.3.txt), match the generated
`SHA256SUMS-windows-x64` release asset, and identify the files attached to the GitHub prerelease.
Native package bytes are not claimed to be reproducible across the local and clean CI build
environments; the live Qwen and signed benchmark proofs used the source-identical local candidate,
while the exact public NSIS separately passed the packaged-owner install lifecycle above.

## Physical campaign boundary

- The physical Windows laptop successfully enrolled and published authenticated QUIC offers while
  running 0.2.2. It has not yet installed the exact 0.2.3 artifact above.
- Consequently, the final two-node 0.2.3 sustained benchmark, the repaired 16 MiB storage campaign,
  laptop-local Ollama qualification, and post-controller-restart physical reconnection remain the
  next interactive gate after one laptop installer run.
- The main PC is installed from the source-identical local 0.2.3 candidate, and its signed local
  model and CPU paths have passed. The byte-distinct public CI NSIS separately passed installation,
  packaged-owner startup, shortcut, shutdown, uninstall, and no-leak checks.
- Phone installation and physical lifecycle qualification remain separate from the desktop fabric.
- Windows 10, Authenticode reputation, macOS notarization, Linux stable packaging, and a deployed
  hard-NAT owner relay remain separate qualification gates.
- Cross-host tensor or pipeline inference remains disabled. The release can route whole-model
  inference to one qualified contributor and can pool independent compute, but it does not claim a
  larger multi-host address space.
