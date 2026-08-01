# Rampage 0.2 release evidence

Validated: **2026-08-01 04:37 America/Chicago**
Status: **PASS as an unsigned Windows x64 release candidate**

## Gates

| Gate | Evidence | Result |
| --- | --- | --- |
| Rust | `cargo fmt --check`; 44 workspace tests; full-workspace/all-target Clippy with warnings denied | PASS |
| Desktop and TypeScript SDK | 4 desktop accessibility/lifecycle tests; production Vite/TypeScript build; 5 SDK tests and build | PASS |
| Python intelligence and SDK | Ruff clean; strict Mypy clean across 9 files; 13 intelligence tests; 5 SDK tests | PASS |
| Local control plane | Separate binaries: recovery, placement, exact-resource signed lease, signed receipt, stop/resume, tokenless request denied | PASS |
| Independent shard pool | Three evaluation shards planned across bounded offers, admitted atomically, returned signed results `2`, `5`, and `8`, met the explicit threshold, and recovered after controller restart | PASS |
| Direct mesh | Separate controller/worker: signed endpoint, QUIC identity enrollment, offer, lease, and receipt | PASS |
| Distributed artifacts | Signed storage leases; binary round trip; encrypted-at-rest worker replica; automatic input staging; retrievable receipt output | PASS |
| Real model work | `llama3.2:latest` through local Ollama returned exactly `RAMPAGE_OK` in a signed receipt | PASS |
| Packaged intelligence | 51,136,730-byte frozen service started with `proposal_only` / `deterministic_only` | PASS |
| Owner desktop | Native release desktop autonomously started controller, intelligence, local node, and offer; close kept the fabric in the system tray | PASS |
| Worker desktop | Native release desktop joined from a signed invite, published a signed artifact endpoint, replicated and returned a byte-exact binary artifact over authenticated QUIC, and stayed active after window close | PASS |
| Lifecycle | Close-to-tray held the owner and worker processes open; explicit exit then left zero Rampage sidecars | PASS |
| NSIS installer | Silent install produced desktop plus four sidecars, `Desktop\\Rampage.lnk`, and the session-local `Rampage Shell.lnk`; native tray lifecycle passed; uninstall removed both shortcuts and exited 0 | PASS |
| MSI package | Administrative extraction exit 0 and contained the five expected executables; generated WiX feature includes `ApplicationShortcutDesktop` with uninstall cleanup | PASS |
| JavaScript dependencies | `pnpm audit --prod --audit-level high` | PASS — no known vulnerabilities |
| Python dependencies | `pip-audit` | PASS — no known vulnerabilities; unpublished local package skipped |
| Rust dependencies | RustSec scan of 714 locked crates | PASS WITH WARNINGS — no vulnerability failure; maintenance warnings below |

Ignored process evidence includes:

- `output/e2e-7cc264c9870d4c7e8ac4e0fbdd627a7a`
- `output/mesh-e2e-287d444011914801be56b14cea626fb9`
- `output/ollama-e2e-76c90ee05ed14c11b94c1be375185f8a`
- `output/desktop-smoke-c5169778433a48349dec6f0a6e6eae76`
- `output/worker-desktop-smoke-05cda97a2be3468d8c31f0ef7079c1f3`

The direct-mesh artifact proof used controller endpoint
`c18135f2589556e8ab2ba358e7627b42e56901e345473323a00802380a3ad0c0`. It moved and
verified `sha256:1e15f9df71575fac97641db86d136fb2c0fa6985cddecd24d28d806dbbbf157a`,
automatically staged `sha256:15f4e2f35460d5e831a54433590bd8eaabafe726e3dcbb1f57ba0275aa2b3140`,
and retrieved worker output
`sha256:e8b2e5ae9ef3cdb3d14d089ca2a562940418979b56242563412908461bb3e050`.

The retained Ollama receipt predates this final packaging run. A fresh rerun was attempted but the
local `llama3.2:latest` warm-up timed out while the machine was under memory pressure; no synthetic
replacement was recorded. The release changes do not broaden the Ollama adapter's authority.

## Release artifacts

| Artifact | Bytes | SHA-256 | Authenticode |
| --- | ---: | --- | --- |
| `target/release/bundle/msi/Rampage_0.2.0_x64_en-US.msi` | 73,719,808 | `8ee25aae6e354bd235e66557acd860b915c00331ea01feef0275342e5029a778` | Not signed |
| `target/release/bundle/nsis/Rampage_0.2.0_x64-setup.exe` | 66,596,324 | `c87a6d69641965d8ce1d3843755772ca3064a767b866d1e46a89ef5454496bf3` | Not signed |
| `target/release/rampage-desktop.exe` | 15,551,488 | `cff610f662466bd671bbf90159de0aa707921d32413e69cb1df541dc37b2f962` | Not signed |
| `apps/desktop/src-tauri/binaries/rampage-intelligence-x86_64-pc-windows-msvc.exe` | 51,136,730 | `9c42859d413330b66c034570a30d588fc2e8fc4720a084f357ca961b7f859818` | Not signed |

The two installer digests are also published in
[`SHA256SUMS-0.2.0.txt`](SHA256SUMS-0.2.0.txt) and attached to the GitHub release.

The missing Authenticode signature is a public-distribution blocker. The binaries are functional
and locally validated, but publication should wait for an ObtuseAI code-signing certificate so
users receive publisher identity and normal Windows reputation behavior. Hashes are evidence, not
a substitute for publisher signing.

## Dependency audit note

RustSec returned no vulnerability failure and 18 allowed maintenance or unsoundness warnings.
GTK3/`glib`, `paste`, and `proc-macro-error` are absent from the Windows target graph. The `unic-*`
warnings reach Tauri's `urlpattern` implementation, including build/code-generation paths, and are
unmaintained-component risk rather than a published vulnerability. The Linux-only `glib`
RUSTSEC-2024-0429 warning remains tracked before any Linux package is released. This is deliberately
not described as a warning-free all-target audit.

## Honest release boundaries

- Packaged and qualified release: Windows 11 x64. Windows 10, macOS, and Linux packages are not yet
  qualified; see the [platform matrix](PLATFORM_MATRIX.md).
- Sharing model: one owner or trusted circle; no public marketplace.
- Network: authenticated direct QUIC; private relay configuration is supported, and public
  dependency relays are never silently selected.
- Distributed execution: whole-job placement and independent shards. Cross-host tensor sharding is
  disabled until a topology/engine adapter passes dedicated evidence gates.
- Edge: contract and Governor policy exist; native phone/tablet/console binaries are not in 0.2.
- Storage: signed, bounded direct-QUIC artifact transfer is shipped for cache/scratch replicas,
  automatic job-input staging, and receipt outputs. V1 transfers are capped at 64 MiB. Protected
  storage still requires an explicit remote-replica durability workflow.
- Publication: no installers or executables currently carry an Authenticode signature.
