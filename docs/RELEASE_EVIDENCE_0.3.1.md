# Rampage 0.3.1 release evidence

This is the qualification ledger for the Rampage 0.3.1 recovery release. Source tests, native
packages, public artifacts, and physical two-machine behavior are separate claims.

## Qualification status

| Gate | Command or artifact | Result |
| --- | --- | --- |
| Nine release versions | `scripts/Assert-RampageVersion.ps1 -Tag v0.3.1-recovery.8` | PASS — nine surfaces report 0.3.1 |
| Rust workspace compile | `cargo check --workspace --all-targets` | PASS |
| Controller lifecycle | `cargo test -p rampage-controller --bin rampage-controller` | PASS — 20 tests, including restart-safe node revocation |
| Native desktop recovery and pairing | `cargo test --workspace`; `cargo test -p rampage-desktop` | PASS — 29 desktop tests, including neutral first run, protected legacy-owner conversion, confirmed-owner preservation, loopback enrollment, directed-broadcast coverage, direct native approval delivery, stale-owner deactivation, self-source rejection, raise-once/clear-once owner attention, and active-worker credential protection |
| Desktop UI and recovery | `pnpm --dir apps/desktop test -- --run` | PASS — 19 tests |
| TypeScript SDK | `pnpm --dir packages/sdk-ts test -- --run` | PASS — 12 tests |
| Python SDK | `uv run --project packages/sdk-python --with pytest --with httpx python -m pytest packages/sdk-python/tests -q` | PASS — 11 tests |
| Full workspace tests and policy | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets -- -D warnings`; `scripts/Assert-RustSecBaseline.ps1` | PASS — all tests; no clippy warnings; 0 RustSec vulnerabilities and 18 target-reviewed warnings through 2026-10-31 |
| Desktop, edge, and TypeScript builds | `pnpm check` | PASS — desktop 19, edge 2, SDK 12 tests plus all production builds |
| Proposal-only intelligence | Ruff, mypy, and pytest | PASS — Ruff clean, mypy clean across 9 files, 17 tests |
| NSIS installer and desktop shortcut | `scripts/Smoke-RampageInstaller.ps1 -Installer output/public-recovery8/Rampage_0.3.1_x64-setup.exe` | PASS — empty runtime stayed neutral; explicit owner install 0/uninstall 0; six payloads; controller/intelligence ready; one signed node and offer; shortcuts created then removed; no leaked sidecars |
| Public release assets | [`v0.3.1-recovery.8`](https://github.com/ObtuseAI/rampage/releases/tag/v0.3.1-recovery.8) | PASS — 12 assets, three source-bound manifests, three checksum files, and verified Sigstore/SLSA provenance |
| Physical owner upgrade and recovery | Public candidate 2 on Windows | PASS — exact public hash, install exit 0, runtime preserved, desktop shortcut present, lifecycle consistent, non-destructive repair restart, controller ready, one resident agent, and one fresh signed offer |
| Physical owner/laptop re-pair | Fresh 0.3.1 installs | PENDING physical laptop action |
| Physical owner-to-laptop view | `scripts/Qualify-RampageRemoteAssist.ps1 -ExpectedVersion 0.3.1` | PENDING live opted-in worker |

## Locally qualified artifacts

These Windows packages were downloaded from the tag-bound GitHub draft. The NSIS package passed the
independent neutral-first-run plus explicit-owner installer smoke before publication.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64_en-US.msi` | 79,015,936 | `708fd0ef4cab4c20879b10e5c089d9beaf8abd5e11b5da362fa4268c20fb0747` |
| `Rampage_0.3.1_x64-setup.exe` | 69,139,844 | `314e0b464ad8a137719025b36c1146c0e82fb04b44ad966db0c4d2db7d43213d` |

The source-current Recovery Center capture is
`docs/assets/rampage-recovery-center.png`, SHA-256
`f5503739ce0f1a069ea067a9bb807639d541872f7f6487a12efcb46fa414366d`.

## Public candidate artifacts

The candidate-8 tag-bound GitHub Actions run `31037500549` rebuilt every package from merged commit
`bcb304e5860d5c8804cbde2b333a7437afc2a759`. Every manifest reports that exact source commit and
version 0.3.1. `gh attestation verify` bound all 12 subjects to the public repository, release tag,
distribution workflow, GitHub-hosted runner, and merged commit.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64-setup.exe` | 69,139,844 | `314e0b464ad8a137719025b36c1146c0e82fb04b44ad966db0c4d2db7d43213d` |
| `Rampage_0.3.1_x64_en-US.msi` | 79,015,936 | `708fd0ef4cab4c20879b10e5c089d9beaf8abd5e11b5da362fa4268c20fb0747` |
| `Rampage_0.3.1_amd64.AppImage` | 168,913,400 | `2bd4a2466dfb013516fedf7a9fcce345527850271891024ed0f8975fde2c7fdd` |
| `Rampage_0.3.1_amd64.deb` | 98,066,682 | `da0ab63ebcc394fa9b32f79d5f5dec616a7d5689c901ee85057d3e027ab8f021` |
| `Rampage_0.3.1_aarch64.dmg` | 86,269,832 | `9fcb20d9131e88eb7fca0e070ef426c0d1de7b8e9c9ffa9e4555d2b0e21c6dbc` |
| `Rampage.app.zip` | 84,307,351 | `56a385dac45fc7c3387b246deeef8f073ff6c250e2d9f0706ad325f4b9bcce62` |

Candidate Windows and macOS packages are intentionally identified as unsigned/not notarized; the
workflow keeps production-signing gates reserved for a future stable channel.

Candidate 1 was superseded after the first physical owner upgrade exposed a false recovery warning:
an owner legitimately self-enrolls its own local agent. Candidate 2 accepts that state only when the
enrollment marker matches the pinned endpoint and the pinned Ed25519 governor key matches this
owner's local controller. Foreign, incomplete, and mismatched identities remain fail-closed.

Candidate 3 was required after the physical laptop remained on “Looking for your main PC” while the
owner listener and private firewall rule were healthy. The old worker sent only global broadcast and
one default-interface multicast packet, so Windows interface selection could hide the actual LAN.
The corrected worker also sends every active directed LAN broadcast, the owner joins multicast on
every active LAN address, and neither side trusts the other device's wall clock for expiry. The
owner's local request lifetime, laptop's local five-minute window, bounded requests, rate limits,
ephemeral X25519 channel, and encrypted invitation remain intact.

Candidate 4 was required after physical discovery and approval succeeded but the laptop
reported that it was already enrolled. Pair again previously signalled the worker process to stop and
immediately reset the runtime; the retiring process could recreate its pin after reset. Rampage now
waits for the complete managed process tree to exit before identity rotation. Setup-only invitation
persistence may remove only the fixed stale worker-credential allowlist, while active owner and
worker identities remain protected. The Windows firewall readiness marker also records the current
installation directory so an upgrade cannot silently trust rules for obsolete binaries.

Candidate 5 was required after a reboot disproved the remaining-process diagnosis. A clean runtime
could silently start an owner controller before onboarding, leaving an owner marker and valid local
pin while the screen still offered **Join my fabric**. Empty runtimes now remain neutral and launch
no fabric sidecars. Creating a fabric writes a durable owner confirmation. Joining creates a
protected transaction that can retire only an unconfirmed legacy bootstrap after its sidecars exit;
confirmed owners and active workers remain non-destructible. The main PC now listens for nearby
requests automatically and displays one device-approval card, with no value to type or compare.

Candidate 6 makes that automatic discovery visible even when the owner minimized Rampage to the
system tray. A newly admitted request restores the window, requests Windows attention without
taking keyboard focus, and names the waiting laptop in the tray. Discovery retransmissions reuse
the existing request and do not repeat the alert.

Candidate 7 completes the lifecycle: approval, rejection, and expiry clear Windows attention and
restore the normal owner tray label exactly once. The final loopback regression proves encrypted
discovery, approval, completion, one alert, duplicate suppression, and one attention clear together.

Candidate 8 was required after live two-PC debugging proved the laptop could retain an in-memory
owner inbox while entering join mode. Its worker broadcast could then receive its own challenge and
falsely display **Main PC found** while the real owner held zero pending requests. Joining now
deactivates any stale owner inbox before broadcasting, and owner discovery rejects requests sourced
from the same machine. The native listener also emits the actual pending request directly to the
owner WebView, while periodic IPC remains reconciliation rather than the only presentation path.

## Honest boundary

The Recovery Center screenshot is a browser-rendered view of the real React component using labeled
showcase topology. Rust tests cover local reset target validation and identity cleanup; controller
tests cover revocation replay. The final physical line requires the owner and laptop to install the
same 0.3.1 package, pair over the real network, advertise a fresh signed worker offer, and complete
the fail-closed Remote Assist qualifier.
