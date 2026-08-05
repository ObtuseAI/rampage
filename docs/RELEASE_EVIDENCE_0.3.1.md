# Rampage 0.3.1 release evidence

This is the qualification ledger for the Rampage 0.3.1 recovery release. Source tests, native
packages, public artifacts, and physical two-machine behavior are separate claims.

## Qualification status

| Gate | Command or artifact | Result |
| --- | --- | --- |
| Nine release versions | `scripts/Assert-RampageVersion.ps1 -Tag v0.3.1-recovery.7` | PASS — nine surfaces report 0.3.1 |
| Rust workspace compile | `cargo check --workspace --all-targets` | PASS |
| Controller lifecycle | `cargo test -p rampage-controller --bin rampage-controller` | PASS — 20 tests, including restart-safe node revocation |
| Native desktop recovery and pairing | `cargo test --workspace`; `cargo test -p rampage-desktop` | PASS — 27 desktop tests, including neutral first run, protected legacy-owner conversion, confirmed-owner preservation, loopback enrollment, directed-broadcast coverage, raise-once/clear-once owner attention, and active-worker credential protection |
| Desktop UI and recovery | `pnpm --dir apps/desktop test -- --run` | PASS — 18 tests |
| TypeScript SDK | `pnpm --dir packages/sdk-ts test -- --run` | PASS — 12 tests |
| Python SDK | `uv run --project packages/sdk-python --with pytest --with httpx python -m pytest packages/sdk-python/tests -q` | PASS — 11 tests |
| Full workspace tests and policy | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets -- -D warnings`; `scripts/Assert-RustSecBaseline.ps1` | PASS — all tests; no clippy warnings; 0 RustSec vulnerabilities and 18 target-reviewed warnings through 2026-10-31 |
| Desktop, edge, and TypeScript builds | `pnpm check` | PASS — desktop 18, edge 2, SDK 12 tests plus all production builds |
| Proposal-only intelligence | Ruff, mypy, and pytest | PASS — Ruff clean, mypy clean across 9 files, 17 tests |
| NSIS installer and desktop shortcut | `scripts/Smoke-RampageInstaller.ps1 -Installer output/public-recovery7/Rampage_0.3.1_x64-setup.exe` | PASS — empty runtime stayed neutral; explicit owner install 0/uninstall 0; six payloads; controller/intelligence ready; one signed node and offer; shortcuts created then removed; no leaked sidecars |
| Public release assets | [`v0.3.1-recovery.7`](https://github.com/ObtuseAI/rampage/releases/tag/v0.3.1-recovery.7) | PASS — 12 assets, three source-bound manifests, three checksum files, and verified Sigstore/SLSA provenance |
| Physical owner upgrade and recovery | Public candidate 2 on Windows | PASS — exact public hash, install exit 0, runtime preserved, desktop shortcut present, lifecycle consistent, non-destructive repair restart, controller ready, one resident agent, and one fresh signed offer |
| Physical owner/laptop re-pair | Fresh 0.3.1 installs | PENDING physical laptop action |
| Physical owner-to-laptop view | `scripts/Qualify-RampageRemoteAssist.ps1 -ExpectedVersion 0.3.1` | PENDING live opted-in worker |

## Locally qualified artifacts

These Windows packages were downloaded from the tag-bound GitHub draft. The NSIS package passed the
independent neutral-first-run plus explicit-owner installer smoke before publication.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64_en-US.msi` | 79,007,744 | `a59ec2a5651fc6a8d7da5729080da573a08ad9785d4221496b1406a990c6754e` |
| `Rampage_0.3.1_x64-setup.exe` | 69,126,892 | `af35f9861a8b24e4d2db8f070faba755b88f22b0f4bea16baa966d59ffdd9499` |

The source-current Recovery Center capture is
`docs/assets/rampage-recovery-center.png`, SHA-256
`f5503739ce0f1a069ea067a9bb807639d541872f7f6487a12efcb46fa414366d`.

## Public candidate artifacts

The candidate-7 tag-bound GitHub Actions run `30989045661` rebuilt every package from merged commit
`6090cb30a9e60f5634db44dc022237636239892b`. Every manifest reports that exact source commit and
version 0.3.1. `gh attestation verify` bound all 12 subjects to the public repository, release tag,
distribution workflow, GitHub-hosted runner, and merged commit.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64-setup.exe` | 69,126,892 | `af35f9861a8b24e4d2db8f070faba755b88f22b0f4bea16baa966d59ffdd9499` |
| `Rampage_0.3.1_x64_en-US.msi` | 79,007,744 | `a59ec2a5651fc6a8d7da5729080da573a08ad9785d4221496b1406a990c6754e` |
| `Rampage_0.3.1_amd64.AppImage` | 168,921,592 | `036e8dae661c9f7ac12a039c89398fb4e33d0a4ab242c1ebdf3fa4f01e6862ec` |
| `Rampage_0.3.1_amd64.deb` | 98,071,962 | `7ded61fce917462753f6d99c17bfd3dd842867fc91db8af630244a26f0e21c5a` |
| `Rampage_0.3.1_aarch64.dmg` | 86,259,635 | `2fa12927f77f6202fada397582ccacc1458a64b6fb6ed3efb4c5a0dd0b1db39d` |
| `Rampage.app.zip` | 84,307,233 | `4b0aec1556866691e8118c500c1ecdf12e55b0e641975e497655c9a46966572a` |

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

## Honest boundary

The Recovery Center screenshot is a browser-rendered view of the real React component using labeled
showcase topology. Rust tests cover local reset target validation and identity cleanup; controller
tests cover revocation replay. The final physical line requires the owner and laptop to install the
same 0.3.1 package, pair over the real network, advertise a fresh signed worker offer, and complete
the fail-closed Remote Assist qualifier.
