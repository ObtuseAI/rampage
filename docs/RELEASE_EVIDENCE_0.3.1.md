# Rampage 0.3.1 release evidence

This is the qualification ledger for the Rampage 0.3.1 recovery release. Source tests, native
packages, public artifacts, and physical two-machine behavior are separate claims.

## Qualification status

| Gate | Command or artifact | Result |
| --- | --- | --- |
| Nine release versions | `scripts/Assert-RampageVersion.ps1 -Tag v0.3.1` | PASS — nine surfaces report 0.3.1 |
| Rust workspace compile | `cargo check --workspace --all-targets` | PASS |
| Controller lifecycle | `cargo test -p rampage-controller --bin rampage-controller` | PASS — 20 tests, including restart-safe node revocation |
| Native desktop recovery and pairing | `cargo test --workspace`; `cargo test -p rampage-desktop` | PASS — 22 desktop tests, including clock-independent loopback enrollment and multi-interface directed-broadcast coverage |
| Desktop UI and recovery | `pnpm --dir apps/desktop test -- --run` | PASS — 18 tests |
| TypeScript SDK | `pnpm --dir packages/sdk-ts test -- --run` | PASS — 12 tests |
| Python SDK | `uv run --project packages/sdk-python --with pytest --with httpx python -m pytest packages/sdk-python/tests -q` | PASS — 11 tests |
| Full workspace tests and policy | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets -- -D warnings`; `scripts/Assert-RustSecBaseline.ps1` | PASS — all tests; no clippy warnings; 0 RustSec vulnerabilities and 18 target-reviewed warnings through 2026-10-31 |
| Desktop, edge, and TypeScript builds | `pnpm check` | PASS — desktop 18, edge 2, SDK 12 tests plus all production builds |
| Proposal-only intelligence | Ruff, mypy, and pytest | PASS — Ruff clean, mypy clean across 9 files, 17 tests |
| NSIS installer and desktop shortcut | `scripts/Smoke-RampageInstaller.ps1` | PASS — install 0, uninstall 0, six payloads, controller/intelligence ready, one signed node and offer, shortcut created then removed, no leaked sidecars |
| Public release assets | [`v0.3.1-recovery.3`](https://github.com/ObtuseAI/rampage/releases/tag/v0.3.1-recovery.3) | PASS — 12 assets, three source-bound manifests, three checksum files, and verified Sigstore/SLSA provenance |
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

The candidate-3 tag-bound GitHub Actions run `30940450467` rebuilt every package from merged commit
`41a091dd02232a92238515996fee9387f40223c8`. Every manifest reports that exact source commit and
version 0.3.1. `gh attestation verify` bound all 12 subjects to the public repository, release tag,
distribution workflow, GitHub-hosted runner, and merged commit.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64-setup.exe` | 69,125,163 | `793cab7cd16e3c40d8ffe7e91e48729548c8f0b40fe124bf73c4ff0591ccf115` |
| `Rampage_0.3.1_x64_en-US.msi` | 79,003,648 | `8c1510ee0b857e7140a2f90e43120d400c4623e262124f4a6cd0ec552c54db4b` |
| `Rampage_0.3.1_amd64.AppImage` | 168,917,496 | `33bafb0c548ff8f3c538844a47b41aae72e311f0bdb274b92fd1fbcdde8b3add` |
| `Rampage_0.3.1_amd64.deb` | 98,061,312 | `b6c3f4521ad9058d092fb4c386226a73cc5b5126f0090b0a71dd92db89ef5786` |
| `Rampage_0.3.1_aarch64.dmg` | 86,264,888 | `48685fe630b23ea451dc035d46ff83c729e01c0a6855eee35c74acb6a7fdccad` |
| `Rampage.app.zip` | 84,303,262 | `8b1f0a5ea7f7c1de2ea6cb3846e3184e0a8274ec7609e8ef08308adf17d7ef77` |

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

## Honest boundary

The Recovery Center screenshot is a browser-rendered view of the real React component using labeled
showcase topology. Rust tests cover local reset target validation and identity cleanup; controller
tests cover revocation replay. The final physical line requires the owner and laptop to install the
same 0.3.1 package, pair over the real network, advertise a fresh signed worker offer, and complete
the fail-closed Remote Assist qualifier.
