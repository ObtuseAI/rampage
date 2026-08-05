# Rampage 0.3.1 release evidence

This is the qualification ledger for the Rampage 0.3.1 recovery release. Source tests, native
packages, public artifacts, and physical two-machine behavior are separate claims.

## Qualification status

| Gate | Command or artifact | Result |
| --- | --- | --- |
| Nine release versions | `scripts/Assert-RampageVersion.ps1 -Tag v0.3.1-recovery.9` | PASS — nine surfaces report 0.3.1 |
| Rust workspace compile | `cargo check --workspace --all-targets` | PASS |
| Controller lifecycle | `cargo test -p rampage-controller --bin rampage-controller` | PASS — 20 tests, including restart-safe node revocation |
| Native desktop recovery and pairing | `cargo test --workspace`; `cargo test -p rampage-desktop` | PASS — 31 desktop tests, including neutral first run, protected legacy-owner conversion, confirmed-owner preservation, retry-safe pairing intent, bounded fragment reassembly under loss/duplicates/reordering, loopback enrollment, directed-broadcast coverage, direct native approval delivery, stale-owner deactivation, self-source rejection, raise-once/clear-once owner attention, and active-worker credential protection |
| Desktop UI and recovery | `pnpm --dir apps/desktop test -- --run` | PASS — 19 tests |
| TypeScript SDK | `pnpm --dir packages/sdk-ts test -- --run` | PASS — 12 tests |
| Python SDK | `uv run --project packages/sdk-python --with pytest --with httpx python -m pytest packages/sdk-python/tests -q` | PASS — 11 tests |
| Full workspace tests and policy | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets -- -D warnings`; `scripts/Assert-RustSecBaseline.ps1` | PASS — all tests; no clippy warnings; 0 RustSec vulnerabilities and 18 target-reviewed warnings through 2026-10-31 |
| Desktop, edge, and TypeScript builds | `pnpm check` | PASS — desktop 19, edge 2, SDK 12 tests plus all production builds |
| Proposal-only intelligence | Ruff, mypy, and pytest | PASS — Ruff clean, mypy clean across 9 files, 17 tests |
| NSIS installer and desktop shortcut | `scripts/Smoke-RampageInstaller.ps1 -Installer output/public-recovery9/Rampage_0.3.1_x64-setup.exe` | PASS — empty runtime stayed neutral; explicit owner install 0/uninstall 0; six payloads; controller/intelligence ready; one signed node and offer; shortcuts created then removed; no leaked sidecars; real installed firewall rules preserved |
| Public release assets | [`v0.3.1-recovery.9`](https://github.com/ObtuseAI/rampage/releases/tag/v0.3.1-recovery.9) | PASS — 12 assets, three source-bound manifests, three checksum files, and verified Sigstore/SLSA provenance |
| Physical owner upgrade and recovery | Public candidate 2 on Windows | PASS — exact public hash, install exit 0, runtime preserved, desktop shortcut present, lifecycle consistent, non-destructive repair restart, controller ready, one resident agent, and one fresh signed offer |
| Physical owner/laptop re-pair | Fresh 0.3.1 installs | PENDING physical laptop action |
| Physical owner-to-laptop view | `scripts/Qualify-RampageRemoteAssist.ps1 -ExpectedVersion 0.3.1` | PENDING live opted-in worker |

## Locally qualified artifacts

These Windows packages were downloaded from the tag-bound GitHub draft. The NSIS package passed the
independent neutral-first-run plus explicit-owner installer smoke before publication.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64_en-US.msi` | 79,036,416 | `0be20761780920c3d543e12ee688c477841bf04b9439d8259f83a738050941f3` |
| `Rampage_0.3.1_x64-setup.exe` | 69,149,351 | `d337b7d77b1bbb6ac47a981b434098402f2c1c58e88327bb26c336017dcee66c` |

The source-current Recovery Center capture is
`docs/assets/rampage-recovery-center.png`, SHA-256
`f5503739ce0f1a069ea067a9bb807639d541872f7f6487a12efcb46fa414366d`.

## Public candidate artifacts

The candidate-9 tag-bound GitHub Actions run `31045549648` rebuilt every package from merged commit
`a726e0055f67af8474f11c4ac64e1b8a9a93c66a`. Every manifest reports that exact source commit and
version 0.3.1. `gh attestation verify` bound all 12 subjects to the public repository, release tag,
distribution workflow, GitHub-hosted runner, and merged commit.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64-setup.exe` | 69,149,351 | `d337b7d77b1bbb6ac47a981b434098402f2c1c58e88327bb26c336017dcee66c` |
| `Rampage_0.3.1_x64_en-US.msi` | 79,036,416 | `0be20761780920c3d543e12ee688c477841bf04b9439d8259f83a738050941f3` |
| `Rampage_0.3.1_amd64.AppImage` | 168,925,688 | `75b32d1e19d5646db25b7df4bc6f33559eeea4374402d5204d54ad2cadab8ecc` |
| `Rampage_0.3.1_amd64.deb` | 98,092,292 | `c7ec067b8e81d3ffd25daad5baf08c8b3d78acb142a1cc451523e475c016cae6` |
| `Rampage_0.3.1_aarch64.dmg` | 86,286,887 | `667c71558aeaf400d6ac73c62c77905c65632937810ff836b0204e22c93a3d78` |
| `Rampage.app.zip` | 84,328,548 | `3945b766192368413c4dc51ae062c2f54f4ad4d9867f4b0c340bd89671aa8827` |

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
The owner's local request lifetime, laptop's local fifteen-minute window, bounded requests, rate limits,
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

Candidate 9 was required after the first genuine owner approval stayed in **Approval sent** until the
laptop's five-minute transaction expired. The owner PC's machine-wide firewall rule also still
targeted a temporary candidate-8 smoke installation, which initially blocked discovery; it was
repaired to the installed executable. The measured 790-byte invitation expands beyond a conservative
1,200-byte cross-network UDP budget after authenticated encryption and JSON/base64 framing, making
path-MTU loss the best-supported diagnosis for the post-approval timeout. Approval is now one
authenticated AES-GCM ciphertext transported as bounded sub-1 KiB fragments; the worker retains
strict reassembly limits and tolerates fragment loss, duplicates, and reordering. The request window
is fifteen minutes, failed attempts can retry the existing protected transaction, and desktop smoke
pre-seeds only its isolated firewall readiness marker instead of mutating real Rampage rules.

## Honest boundary

The Recovery Center screenshot is a browser-rendered view of the real React component using labeled
showcase topology. Rust tests cover local reset target validation and identity cleanup; controller
tests cover revocation replay. The final physical line requires the owner and laptop to install the
same 0.3.1 package, pair over the real network, advertise a fresh signed worker offer, and complete
the fail-closed Remote Assist qualifier.
