# Rampage 0.2.0 — Model Fabric

Rampage 0.2 turns a group of trusted machines into an explicit strategy surface for local AI. The
desktop now separates the two goals people usually blur together: fitting the largest possible
model and producing the fastest possible response.

## What changed

- **Maximum Model** plans the largest qualified, compatibility-matched aggregate memory placement.
- **Speed Boost** accepts distributed tensor peers only when signed link measurements predict a
  real gain; slow links fall back to the best whole-model host.
- **Throughput, Efficiency, and Autonomous Balanced** expose replica, smallest-fit, and
  proposal-only adaptive planning lanes.
- Signed agent offers can advertise bounded Ollama, Exo/MLX, or vLLM/Ray runtime profiles through a
  strict manifest and digest contract.
- The controller, CLI, Rust SDK, TypeScript SDK, Python SDK, and native desktop all share the same
  versioned model-session planning contract.
- The native Tauri shell now has a role-aware Windows tray, close-to-tray behavior, Start with
  Windows, quiet background launch, emergency stop, explicit Quit, and deterministic sidecar
  cleanup.
- MSI and NSIS packages create an automatic desktop launcher plus a Start-menu Rampage Shell whose
  session-local PATH makes the authenticated CLI immediately usable without mutating system PATH.

## The boundary that matters

The 0.2 planner is real and read-only. It never turns a capacity estimate into execution authority.
Cross-host model launch and the proposed loopback inference gateway remain disabled until a backend
passes correctness, isolation, topology, streaming-failure, cleanup, and measured-benefit gates.
That distinction is the foundation for making Rampage ambitious without making it reckless.

## Verification

The release candidate passes 44 Rust tests, strict all-target Clippy, 4 desktop tests and production
build, 5 TypeScript SDK tests, 13 intelligence tests, 5 Python SDK tests, authenticated direct-QUIC
mesh and artifact proofs, a real Ollama generation receipt, native owner/worker lifecycle probes,
and a silent NSIS install/shortcut/uninstall smoke.

The Windows artifacts are still unsigned. See [release evidence](RELEASE_EVIDENCE.md) before
distribution.
