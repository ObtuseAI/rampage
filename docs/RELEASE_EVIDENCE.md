# Rampage 0.1 release evidence

Validated: **2026-07-31 22:41 America/Chicago**  
Status: **PASS as an unsigned Windows x64 release candidate**

## Gates

| Gate | Evidence | Result |
| --- | --- | --- |
| Rust | `cargo fmt --check`; 35 workspace tests; full-workspace/all-target Clippy with warnings denied | PASS |
| Desktop and TypeScript SDK | 2 desktop accessibility tests; production Vite/TypeScript build; 4 SDK tests and build | PASS |
| Python intelligence and SDK | Ruff clean; strict Mypy clean across 9 files; 13 intelligence tests; 4 SDK tests | PASS |
| Local control plane | Separate binaries: recovery, placement, exact-resource signed lease, signed receipt, stop/resume, tokenless request denied | PASS |
| Independent shard pool | Three evaluation shards planned across bounded offers, admitted atomically, returned signed results `2`, `5`, and `8`, met the explicit threshold, and recovered after controller restart | PASS |
| Direct mesh | Separate controller/worker: signed endpoint, QUIC identity enrollment, offer, lease, and receipt | PASS |
| Distributed artifacts | Signed storage leases; binary round trip; encrypted-at-rest worker replica; automatic input staging; retrievable receipt output | PASS |
| Real model work | `llama3.2:latest` through local Ollama returned exactly `RAMPAGE_OK` in a signed receipt | PASS |
| Packaged intelligence | 51,136,881-byte frozen service started with `proposal_only` / `deterministic_only` | PASS |
| Owner desktop | Release desktop autonomously started controller, intelligence, local node, and offer | PASS |
| Worker desktop | Release desktop joined from a signed invite, published a signed artifact endpoint, replicated and returned a byte-exact binary artifact over authenticated QUIC | PASS |
| Lifecycle | Owner and worker desktop tests found zero Rampage sidecars after window close | PASS |
| NSIS installer | Silent install produced desktop plus four sidecars and `Desktop\\Rampage.lnk`; shortcut target matched the installed desktop executable; installed app passed smoke; uninstall removed the shortcut and exited 0 | PASS |
| MSI package | Administrative extraction exit 0 and contained the five expected executables; generated WiX feature includes `ApplicationShortcutDesktop` with uninstall cleanup | PASS |
| JavaScript dependencies | `pnpm audit --prod --audit-level high` | PASS — no known vulnerabilities |
| Python dependencies | `pip-audit` | PASS — no known vulnerabilities; unpublished local package skipped |
| Rust dependencies | RustSec scan of 680 locked crates | PASS WITH WARNINGS — no vulnerability failure; maintenance warnings below |

Ignored process evidence includes:

- `output/e2e-202ecc07756f4de3970d073539324707`
- `output/mesh-e2e-c0d8f9390dfc4656a3f2c52ddab84386`
- `output/ollama-e2e-b414cbc6958e4791b7e56938b38b70ae`
- `output/intelligence-smoke-3c4bdc2576fa428a9f4b2eab03cb78f4`
- `output/desktop-smoke-48de1eb292e84385bebf28fc30a4c91a`
- `output/worker-desktop-smoke-36ea6dc5372a4327a1e86e5a6c886866`
- `output/msi-extract-c22d7a05d0e047afba6e7f7db3d12bbc`

The direct-mesh artifact proof used controller endpoint
`d32eaf346f6e1053e79427895e47dccc085c808ff96911dce3e9486cc4957cd0`. It moved and
verified `sha256:1f8feb07aa7f4f82375f37b75ca909edde3f0219dabf9472043321a1a87d3047`,
automatically staged `sha256:aed25e9f0302a09337a09abb3f83c604525c0ada7f834546b9f102d4887876e7`,
and retrieved worker output
`sha256:525ccb3f9d33151a22815878756a11f89958511b5dfec0ecf690c3145a6e05a0`.

## Release artifacts

| Artifact | Bytes | SHA-256 | Authenticode |
| --- | ---: | --- | --- |
| `target/release/bundle/msi/Rampage_0.1.0_x64_en-US.msi` | 73,461,760 | `e207ab863e8f6187905f074561167e9071cca690d406ce1910d0b125aab5ae76` | Not signed |
| `target/release/bundle/nsis/Rampage_0.1.0_x64-setup.exe` | 66,425,137 | `221076d7470fe1cc43887023e893c5264b0f760cb997d542922e9b700bbb45e5` | Not signed |
| `target/release/rampage-desktop.exe` | 15,017,984 | `7bfac4e417aea0f688b019236d3e37d47093096b178acc97a463894c19906094` | Not signed |
| `dist/rampage-intelligence.exe` | 51,137,326 | `1d5425c8448ab0b1124686e799da44faccb25549c57e49b767a70d02ce837640` | Not signed |

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
- Edge: contract and Governor policy exist; native phone/tablet/console binaries are not in 0.1.
- Storage: signed, bounded direct-QUIC artifact transfer is shipped for cache/scratch replicas,
  automatic job-input staging, and receipt outputs. V1 transfers are capped at 64 MiB. Protected
  storage still requires an explicit remote-replica durability workflow.
- Publication: no installers or executables currently carry an Authenticode signature.
