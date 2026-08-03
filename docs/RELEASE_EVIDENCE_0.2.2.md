# Rampage 0.2.2 durable-pairing evidence

Validated: **2026-08-02 America/Chicago**

Status: **PASS as an unsigned Windows 11 x64 prerelease candidate**

Release channel: **`v0.2.2-pairing.2`**

## Gates

| Gate | Evidence | Result |
| --- | --- | --- |
| Consumed-invite migration | An enrolled worker accepts an expired one-time bundle only to verify and persist its signed controller route, then removes the invitation | PASS |
| Pinned restart | The same packaged desktop restarts without an invitation, reconnects to the controller, and publishes a fresh signed offer | PASS |
| Worker truthfulness | Runtime becomes active only after signed-offer acknowledgement; stderr, premature exit, and connection failure produce bounded non-active states | PASS |
| Stable owner route | Explicit and persisted ports bind exactly; a legacy ledger migrates its newest IPv4 `mesh.started` endpoint | PASS |
| Private Windows firewall | Pairing installs executable-scoped inbound UDP rules for the Private profile only, behind the native UAC boundary | PASS by code/test inspection; physical UAC confirmation pending |
| Encrypted artifact path | Packaged worker accepts and returns a seven-byte binary artifact over authenticated direct QUIC with an unchanged SHA-256 digest | PASS |
| NSIS lifecycle | Six payloads; install/uninstall exit 0; desktop and shell shortcuts; controller/intelligence/node/offer ready; no sidecar leak | PASS |
| Diagnostic coexistence | Packaged owner smoke uses isolated loopback API and mesh ports while the installed main fabric remains live | PASS |
| Rust | Formatting clean; focused agent/controller/desktop/ledger/mesh suites pass, including restart and migration regressions | PASS |
| Desktop | 12 component tests, 13 native desktop tests, and the production build pass | PASS |

The final worker evidence directory was
`output/worker-desktop-smoke-0d84cae89cca4fcca5d14267a2fc0057`. It records
`consumed_invite_removed=true`, `pinned_restart=true`, `artifact_round_trip=true`,
`clean_explicit_exit=true`, and `sidecar_leak=false`. These paths are local process evidence, not
portable release assets.

## Release artifacts

| Artifact | Bytes | SHA-256 | Authenticode |
| --- | ---: | --- | --- |
| `Rampage_0.2.2_x64-setup.exe` | 68,824,182 | `94731f9e55f56e48d6d5a91a618bbcc7315917f1937a61d81a1a701c4e8f808d` | Not signed |
| `Rampage_0.2.2_x64_en-US.msi` | 78,606,336 | `7b2a81140c836830da0cb6284ff53319c20f4cb14f322b5092dc9fd28c4c7f60` | Not signed |

The checksums are duplicated in [`SHA256SUMS-0.2.2.txt`](SHA256SUMS-0.2.2.txt) and must match the
assets attached to the GitHub prerelease.

## Remaining qualification boundaries

- Install this exact 0.2.2 candidate on the real laptop, approve the four-digit comparison, verify
  a signed offer on the main PC, then restart the laptop and verify automatic reconnection.
- The phone remains a separate native Rampage Edge gate after the laptop passes.
- Windows 10, Authenticode reputation, macOS notarization, Linux stable packaging, and hard-NAT
  owner-relay deployment remain separate qualification gates.
- Arbitrary applications cannot borrow another machine's address space. Useful pooling remains
  workload-aware: whole jobs, independent shards, supported engine-native distribution, caches,
  and encrypted artifacts.
