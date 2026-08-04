# Rampage 0.3.1 release evidence

This is the qualification ledger for the Rampage 0.3.1 recovery release. Source tests, native
packages, public artifacts, and physical two-machine behavior are separate claims.

## Qualification status

| Gate | Command or artifact | Result |
| --- | --- | --- |
| Nine release versions | `scripts/Assert-RampageVersion.ps1 -Tag v0.3.1` | PASS — nine surfaces report 0.3.1 |
| Rust workspace compile | `cargo check --workspace --all-targets` | PASS |
| Controller lifecycle | `cargo test -p rampage-controller --bin rampage-controller` | PASS — 20 tests, including restart-safe node revocation |
| Native desktop recovery | `cargo test --workspace --no-fail-fast` | PASS — 19 desktop tests inside the full workspace campaign |
| Desktop UI and recovery | `pnpm --dir apps/desktop test -- --run` | PASS — 18 tests |
| TypeScript SDK | `pnpm --dir packages/sdk-ts test -- --run` | PASS — 12 tests |
| Python SDK | `uv run --project packages/sdk-python --with pytest --with httpx python -m pytest packages/sdk-python/tests -q` | PASS — 11 tests |
| Full workspace tests and policy | `cargo test --workspace --no-fail-fast`; `cargo clippy --workspace --all-targets -- -D warnings`; `scripts/Assert-RustSecBaseline.ps1` | PASS — all tests; no clippy warnings; 0 RustSec vulnerabilities and 18 target-reviewed warnings through 2026-10-31 |
| Desktop, edge, and TypeScript builds | `pnpm check` | PASS — desktop 18, edge 2, SDK 12 tests plus all production builds |
| Proposal-only intelligence | Ruff, mypy, and pytest | PASS — Ruff clean, mypy clean across 9 files, 17 tests |
| NSIS installer and desktop shortcut | `scripts/Smoke-RampageInstaller.ps1` | PASS — install 0, uninstall 0, six payloads, controller/intelligence ready, one signed node and offer, shortcut created then removed, no leaked sidecars |
| Public release assets | [`v0.3.1-recovery.1`](https://github.com/ObtuseAI/rampage/releases/tag/v0.3.1-recovery.1) | PASS — 12 uploaded assets, three source-bound manifests, three checksum files, and GitHub build-provenance attestation |
| Physical owner/laptop re-pair | Fresh 0.3.1 installs | PENDING physical laptop action |
| Physical owner-to-laptop view | `scripts/Qualify-RampageRemoteAssist.ps1 -ExpectedVersion 0.3.1` | PENDING live opted-in worker |

## Locally qualified artifacts

These packages came from the verified local source tree and passed the independent installer smoke
test. GitHub Actions builds the public release artifacts again from the tagged commit, so the public
asset hashes will be recorded separately after publication.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64_en-US.msi` | 79,290,368 | `a02b5995e082eb7be371f675ed80d5b0640a16499eb9de1b8e0f093ac7cd06ee` |
| `Rampage_0.3.1_x64-setup.exe` | 69,374,688 | `e2fffa0326e6a6e3322b294433911d94f47ef9bbd40e0647857ce52bc899a5a5` |

The source-current Recovery Center capture is
`docs/assets/rampage-recovery-center.png`, SHA-256
`f5503739ce0f1a069ea067a9bb807639d541872f7f6487a12efcb46fa414366d`.

## Public candidate artifacts

The tag-bound GitHub Actions run rebuilt every package from merged commit
`51b4c054748200c76bdc45798e705a75da42d98e`. Its Windows manifest reports version 0.3.1,
and the recommended Windows download returned HTTP 200 with the expected byte length after the
release was published.

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| `Rampage_0.3.1_x64-setup.exe` | 69,112,909 | `22e4ea85dfc63bb2667b32523409acab1b377b0eebd1ea25c9655df53fe73c9c` |
| `Rampage_0.3.1_x64_en-US.msi` | 78,987,264 | `c6ad9513c9a5667f2964e05034735a5a52bb83033deab92a9a303f601941bb43` |
| `Rampage_0.3.1_amd64.AppImage` | 168,880,632 | `9d6a05bfed97d0367dc6561a14e70317685d74972953ff86dbed27dd705ee16a` |
| `Rampage_0.3.1_amd64.deb` | 98,031,344 | `4745a39166d008ceb0d13c902c40745eeb50975364b1b0b7a7f6c6049f886a5d` |
| `Rampage_0.3.1_aarch64.dmg` | 86,234,740 | `ba00e8feba971d2de6fe19c35cc9ceb978e04bae127b975227f95dbe35d74d04` |
| `Rampage.app.zip` | 84,272,637 | `644aa2d82823dd1bb226de3bc36bf0dc74699da22f977be339f08f83c953aa01` |

Candidate Windows and macOS packages are intentionally identified as unsigned/not notarized; the
workflow keeps production-signing gates reserved for a future stable channel.

## Honest boundary

The Recovery Center screenshot is a browser-rendered view of the real React component using labeled
showcase topology. Rust tests cover local reset target validation and identity cleanup; controller
tests cover revocation replay. The final physical line requires the owner and laptop to install the
same 0.3.1 package, pair over the real network, advertise a fresh signed worker offer, and complete
the fail-closed Remote Assist qualifier.
