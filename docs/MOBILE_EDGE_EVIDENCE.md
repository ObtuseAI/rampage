# Rampage Edge mobile candidate evidence

Validated: **2026-08-02 00:53 America/Chicago**  
Status: **PASS as unsigned Android ARM64 and Apple-silicon iOS simulator source candidates**

This record qualifies the native mobile source at commit
[`e68d22fcceb7212108e90287e01121b090e99914`](https://github.com/ObtuseAI/rampage/commit/e68d22fcceb7212108e90287e01121b090e99914).
GitHub Actions tested pull-request merge commit
[`c4adafb504f63fe55c82f52ee95cb918dd2d82cb`](https://github.com/ObtuseAI/rampage/commit/c4adafb504f63fe55c82f52ee95cb918dd2d82cb),
whose parents are main commit `bfa95bb40b7541583d9c81b61eb796e83997b091` and that source head.
The synthetic merge digest appears in the artifact names because pull-request workflows use
`github.sha`; the run API separately binds the campaign to the source head.

## Qualification campaign

| Gate | Evidence | Result |
| --- | --- | --- |
| Android ARM64 | [Job 91461385629](https://github.com/ObtuseAI/rampage/actions/runs/30734733419/job/91461385629) on Ubuntu 24.04; Rust `aarch64-linux-android`; Java 21; Android API/target 36; NDK `27.1.12297006`; unsigned APK build, archive test, checksum, and upload | PASS |
| iOS simulator ARM64 | [Job 91461385675](https://github.com/ObtuseAI/rampage/actions/runs/30734733419/job/91461385675) on macOS 15 ARM64; Rust `aarch64-apple-ios-sim`; generated Xcode project; Tauri `--ci --debug --target aarch64-sim --no-sign`; archive test, checksum, and upload | PASS |
| Mobile workflow | [Run 30734733419](https://github.com/ObtuseAI/rampage/actions/runs/30734733419), source head `e68d22f`, both jobs completed successfully | PASS |
| Rust trust plane | [Run 30734733415](https://github.com/ObtuseAI/rampage/actions/runs/30734733415): full workspace tests, full-workspace/all-target Clippy with warnings denied, native sidecars, and model-gateway campaign | PASS |
| Desktop, TypeScript, Python, dependency review | The same CI run passed the desktop/TypeScript build and tests, strict Python checks and tests, and GitHub dependency review | PASS |
| Code scanning | [CodeQL run 30734733412](https://github.com/ObtuseAI/rampage/actions/runs/30734733412) analyzed Actions, JavaScript/TypeScript, and Python at the qualified source head | PASS |
| Local source hygiene | `cargo fmt --all -- --check`; `pnpm --dir apps/edge build`; branch-wide `git diff main --check`; credential-pattern scan; landing-page local-reference and CSS-structure checks | PASS |

## Downloaded artifact verification

The two workflow artifacts were downloaded after the successful run. Their embedded
`SHA256SUMS.txt` values matched independent SHA-256 calculations over the extracted candidates.

| Candidate | Bytes | SHA-256 | Artifact |
| --- | ---: | --- | --- |
| `rampage-edge-android-arm64-unsigned.apk` | 35,247,174 | `5f027964f32ef6a5379c658c2579ae30f92b1a9958762cb882af07a213efae98` | `rampage-edge-android-arm64-c4adafb504f63fe55c82f52ee95cb918dd2d82cb` ([artifact 8829208673](https://github.com/ObtuseAI/rampage/actions/runs/30734733419/artifacts/8829208673)) |
| `rampage-edge-ios-simulator-arm64-unsigned.zip` | 36,595,273 | `683040528e4ceacc4086b1323a5dafc94c02285e761cdea721e262de35877bb9` | `rampage-edge-ios-simulator-arm64-c4adafb504f63fe55c82f52ee95cb918dd2d82cb` ([artifact 8829201084](https://github.com/ObtuseAI/rampage/actions/runs/30734733419/artifacts/8829201084)) |

The Android artifact was independently inspected with Android build-tools 36 `aapt2`. It reports:

- package `ai.obtuse.rampage.edge`, version `0.2.0` / code `2000`;
- minimum SDK 26, target and compile SDK 36;
- launchable activity `ai.obtuse.rampage.edge.MainActivity`;
- native code only for `arm64-v8a`, including `librampage_edge_app_lib.so`;
- `android.permission.INTERNET` and no TV/Leanback feature declaration.

The iOS candidate archive contains `Rampage Edge.app`, its executable, `Info.plist`, asset catalog,
launch storyboard, and packaged web assets. `SystemConfiguration` is declared in the generated
project contract because the Rust mesh resolver uses Apple's network-configuration APIs.

GitHub Actions artifacts are retention-bound build evidence, not a stable store-distribution
channel. The checksums above preserve candidate identity after artifact expiry.

## Runtime and authority contract

The successful native builds contain the same bounded edge runtime:

1. Kotlin or Swift reads native device class, foreground state, battery, external power,
   low-power mode, thermal state, and screen-awake state.
2. Donation is eligible only in the foreground, when requested by the owner, outside low-power
   mode, with at least 40% battery unless externally powered, and with at least 35% thermal
   headroom. Missing or unsupported telemetry fails closed.
3. A persistent Ed25519 identity enrolls once through a fresh signed invitation. The stored
   controller route remains pinned and its Governor signature is rechecked after every restart.
4. Each eligible pulse posts a signed 20-second CPU offer, polls at most one claim, and accepts only
   `rampage.hash.v1` or `rampage.eval-shard.v1` under a fresh one-shot, epoch-fenced lease.
5. Results enter a durable signed-receipt outbox before submission. Consumed nonce and fencing state
   survive restart even though the mobile worker advertises zero storage capacity.
6. Native lifecycle loss, pressure, owner STOP, or a failed UI pulse clears donation and the
   screen-awake state locally; no controller response is required to stop offering new work.

The controller rejects a resource offer whose `device_kind` labels do not match the enrolled native
identity and rejects mobile offers without native battery telemetry. Device labels never imply an
operation; the exact adapter allowlist does.

## Honest release boundaries

- These are unsigned source candidates, not Google Play or App Store releases.
- The iOS output is an Apple-silicon **simulator** application. It is not a signed physical-device
  IPA, and it does not prove real iPhone/iPad thermal or lifecycle behavior.
- The Android output is an unsigned ARM64 APK. It has not completed a physical-device matrix,
  Play signing, store review, or long-duration thermal campaign.
- Android 8 and 9 meet the installation floor but do not expose the required process thermal signal,
  so donation remains ineligible there by design.
- Contribution is foreground-only. There is no Android foreground service, iOS background mode,
  always-on daemon, shell, public marketplace, model server, protected storage role, or console package.
- Phones and tablets do not become transparent RAM or VRAM. This milestone offloads small hashing and
  independent evaluation shards; every additional operation needs a separately shipped and qualified adapter.
- Store signing, physical-device installation, interruption/relaunch campaigns, and sustained thermal,
  battery, and network qualification remain separate release gates.

See [Rampage Edge](EDGE_DEVICES.md) for the design and platform-policy rationale.
