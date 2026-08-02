# RustSec dependency boundary

Rampage treats the complete lockfile and the code reachable on each release target as separate,
testable facts. `scripts/Assert-RustSecBaseline.ps1` fails on any published vulnerability, any new
or removed informational advisory, any package/advisory mismatch, an expired review date, or a
change in target reachability.

The current 751-package lockfile has **zero published vulnerabilities** and 18 explicitly reviewed
informational advisories:

| Reachable release graph | Reviewed advisories | Enforcement |
|---|---:|---|
| Windows x64 | 5 unmaintained `unic-*` crates through Tauri's URLPattern parser | Candidate and stable builds may proceed; baseline drift fails CI. |
| macOS Apple Silicon | The same 5 `unic-*` crates | Candidate and signed stable builds may proceed; baseline drift fails CI. |
| Linux x64 | 18 advisories: the 5 `unic-*` crates, GTK3 maintenance set, `paste`, `proc-macro-error`, and the `glib` unsoundness advisory | Candidate packages are evidence artifacts only. Stable Linux publication is blocked. |

The five cross-platform `unic-*` entries are unmaintained-component notices, not published
vulnerabilities. They remain transitive through Tauri's URLPattern parser and cannot be replaced
inside Rampage without forking a security-sensitive framework component. The Linux-only GTK3
runtime includes `RUSTSEC-2024-0429`; Rampage therefore refuses to call a Linux package stable until
that runtime path is removed upstream or replaced and the baseline becomes clean.

The machine-readable source of truth is `security/rustsec-baseline.json`. Its review deadline is
intentional: carrying a reviewed exception forever is not permitted.
