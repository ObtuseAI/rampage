# Rampage 0.3.1 release evidence

This is the qualification ledger for the Rampage 0.3.1 recovery release. Source tests, native
packages, public artifacts, and physical two-machine behavior are separate claims.

## Qualification status

| Gate | Command or artifact | Result |
| --- | --- | --- |
| Nine release versions | `scripts/Assert-RampageVersion.ps1 -Tag v0.3.1` | PASS — nine surfaces report 0.3.1 |
| Rust workspace compile | `cargo check --workspace --all-targets` | PASS |
| Controller lifecycle | `cargo test -p rampage-controller --bin rampage-controller` | PASS — 20 tests, including restart-safe node revocation |
| Native desktop recovery and pairing | `cargo test --workspace`; `cargo test -p rampage-desktop` | PASS — 24 desktop tests, including clock-independent loopback enrollment, multi-interface directed-broadcast coverage, setup-only stale-credential healing, and active-worker credential protection |
| Desktop UI and recovery | `pnpm --dir apps/desktop test -- --run` | PASS — 18 tests |
| TypeScript SDK | `pnpm --dir packages/sdk-ts test -- --run` | PASS — 12 tests |
| Python SDK | `uv run --project packages/sdk-python --with pytest --with httpx python -m pytest packages/sdk-python/tests -q` | PASS — 11 tests |
| Full workspace tests and policy | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets -- -D warnings`; `scripts/Assert-RustSecBaseline.ps1` | PASS — all tests; no clippy warnings; 0 RustSec vulnerabilities and 18 target-reviewed warnings through 2026-10-31 |
| Desktop, edge, and TypeScript builds | `pnpm check` | PASS — desktop 18, edge 2, SDK 12 tests plus all production builds |
| Proposal-only intelligence | Ruff, mypy, and pytest | PASS — Ruff clean, mypy clean across 9 files, 17 tests |
| NSIS installer and desktop shortcut | `scripts/Smoke-RampageInstaller.ps1` | PASS — install 0, uninstall 0, six payloads, controller/intelligence ready, one signed node and offer, shortcut created then removed, no leaked sidecars |
| Public release assets | [`v0.3.1-recovery.4`](https://github.com/ObtuseAI/rampage/releases/tag/v0.3.1-recovery.4) | PASS — 12 assets, three source-bound manifests, three checksum files, and verified Sigstore/SLSA provenance |
| Physical owner upgrade and recovery | Public candidate 2 on Windows | PASS — exact public hash, install exit 0, runtime preserved, desktop shortcut present, lifecycle consistent, non-destructive repair restart, controller ready, one resident agent, and one fresh signed offer |
| Physical owner/laptop re-pair | Fresh 0.3.1 installs | PENDING physical laptop action |
| Physical owner-to-laptop view | `scripts/Qualify-RampageRemoteAssist.ps1 -ExpectedVersion 0.3.1` | PENDING live opted-in worker |

## Locally qualified artifacts

These packages came from the verified local source tree and passed the independent installer smoke
test. GitHub Actions builds the public release artifacts again from the tagged commit, so the public
asset hashes will be recorded separately after publication.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64_en-US.msi` | 79,314,944 | `9b35e4d9348787d504b2fc5eaa1b8e3dce81111b5ebeb2293a88da184b82e789` |
| `Rampage_0.3.1_x64-setup.exe` | 69,396,188 | `0056224623f9cd4bdb6d350d0859a103f0647d2e38d9b22257b1feae4a2aa7d8` |

The source-current Recovery Center capture is
`docs/assets/rampage-recovery-center.png`, SHA-256
`f5503739ce0f1a069ea067a9bb807639d541872f7f6487a12efcb46fa414366d`.

## Public candidate artifacts

The candidate-4 tag-bound GitHub Actions run `30952445731` rebuilt every package from merged commit
`1e230bcd2a2480007f335333fda8c099f202a008`. Every manifest reports that exact source commit and
version 0.3.1. `gh attestation verify` bound all 12 subjects to the public repository, release tag,
distribution workflow, GitHub-hosted runner, and merged commit.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64-setup.exe` | 69,130,089 | `f7863556c21828048f34725800db6cc3b9abac3aea449f35220f38086e6245c8` |
| `Rampage_0.3.1_x64_en-US.msi` | 79,011,840 | `63ad5083ded101ca7a262d0063eda6934ca0bdbfc7a5d89ae6c641a424a96544` |
| `Rampage_0.3.1_amd64.AppImage` | 168,909,304 | `bd55d53e5519a5ff64ed92c7d075b075634ebd918a2b81d18317452b0f41c654` |
| `Rampage_0.3.1_amd64.deb` | 98,052,650 | `c0bde5ecacbe94bb0a6dfa4054e4cf28461db6170f2fdf7e7059fe906bffa2f3` |
| `Rampage_0.3.1_aarch64.dmg` | 86,262,227 | `9122b470a9430a27a83b5228627b479546544439f8b84ef2a8bd87e22b783198` |
| `Rampage.app.zip` | 84,305,433 | `5a1909d0acd22937ecaca3234dcee10aa45efde5912a7496d2337164ae1f25b6` |

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
owner's local three-minute window, laptop's local five-minute window, bounded requests, rate limits,
ephemeral X25519 transcript, matching four-digit code, and encrypted invitation remain intact.

Candidate 4 was required after physical discovery and four-digit approval succeeded but the laptop
reported that it was already enrolled. Pair again previously signalled the worker process to stop and
immediately reset the runtime; the retiring process could recreate its pin after reset. Rampage now
waits for the complete managed process tree to exit before identity rotation. Setup-only invitation
persistence may remove only the fixed stale worker-credential allowlist, while active owner and
worker identities remain protected. The Windows firewall readiness marker also records the current
installation directory so an upgrade cannot silently trust rules for obsolete binaries.

## Honest boundary

The Recovery Center screenshot is a browser-rendered view of the real React component using labeled
showcase topology. Rust tests cover local reset target validation and identity cleanup; controller
tests cover revocation replay. The final physical line requires the owner and laptop to install the
same 0.3.1 package, pair over the real network, advertise a fresh signed worker offer, and complete
the fail-closed Remote Assist qualifier.
