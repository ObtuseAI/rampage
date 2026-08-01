# Universal compute milestone evidence

Validated: **2026-08-01 America/Chicago**

Status: **PASS for source and deterministic process qualification; not a new signed binary release**

| Gate | Evidence | Result |
| --- | --- | --- |
| Rust workspace | `cargo test --workspace --all-features` | PASS — 76 tests |
| Rust quality | `cargo fmt --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| Desktop | Vitest accessibility/UI suite and production TypeScript/Vite build | PASS — 5 tests |
| TypeScript SDK | Vitest and TypeScript build | PASS — 9 tests |
| Python SDK | Pytest with local source import | PASS — 9 tests |
| Intelligence plane | Ruff, strict Mypy across all 15 source and test files, and Pytest | PASS — 16 tests |
| Universal model gateway | Deterministic fake-Ollama campaign plus a separate controller/worker campaign against local Ollama `llama3.2:latest` | PASS |
| OpenAI surface | Models, non-streaming chat, SSE chat, bearer denial | PASS |
| Anthropic surface | Messages, system/text blocks, non-streaming, Anthropic SSE events, `x-api-key` | PASS |
| OpenRouter-style surface | `/api/v1/models` and `/api/v1/chat/completions` | PASS |
| Workload capability contract | Exact signed Ollama `chat` capability discovered; candidate authority false | PASS |
| Self-scan | Stable SHA-256 evidence, no per-change prompt, hash-chained transition | PASS |
| Autonomous canary | Eight evidence gates rechecked by Rust, bounded signed lease, repeat request idempotent | PASS |
| JavaScript dependencies | `pnpm audit --audit-level high` | PASS — no known vulnerabilities |
| Rust dependencies | `cargo audit` over 714 locked crates | PASS WITH WARNINGS — no published vulnerability; 18 allowed maintenance/unsoundness warnings |

The retained ignored process campaigns are:

- `output/ollama-e2e-24d71b1303e04fe29fa9c6e30add47cd` — deterministic fake Ollama
- `output/ollama-e2e-9d2e73b2bc464bfca5ed0b998cb5d9d7` — local Ollama `llama3.2:latest`

It proves request translation, authentication, authenticated QUIC transport, exact installed-model
selection, streaming, terminal transcript receipts, capability discovery, self-diagnostics, and
signed canary authority. The deterministic fixture isolates protocol behavior; the local Ollama
campaign confirms the same path with a real installed model. Neither is evidence of model quality,
measured token throughput, multi-host VRAM pooling, or cross-host tensor/pipeline execution.

## Dependency warning boundary

RustSec returned no vulnerability failure. The 18 warnings are the already tracked GTK3/`glib`,
`paste`, `proc-macro-error`, and `unic-*` maintenance set. GTK3 and `glib` are outside the qualified
Windows target graph. The `unic-*` path remains transitive through Tauri tooling. Linux packaging
remains blocked on isolating or replacing the GTK3/`glib` path; these warnings are not described as
fixed or warning-free.

## Honest execution boundary

- Whole-model text inference is operational when the exact model fits one contributor.
- The universal capability schema does not make every domain executable. Only a live signed
  `shipped` or evidence-backed `qualified` adapter/operation grants authority.
- Cross-host model-memory aggregation remains a planner-only candidate until a backend and topology
  campaign qualifies tensor or pipeline launch.
- The operational autonomous lane can only reduce placement authority by excluding unroutable,
  overheating, or low-battery nodes. It cannot enroll peers or grant new resources.
- Recursive candidates receive only a short-lived, traffic- and regression-capped canary lease.
  The AI cannot alter the Governor, signing key, STOP latch, or owner envelope.
- The existing 0.2 Windows installers remain unsigned. No new installer artifact is claimed here.
