# Rampage platform matrix

This matrix distinguishes shipped, source-portable, and architectural support. “Designed” is not a
binary release claim.

| Platform | Installable package | Controller | Worker | Intended donation profile | Evidence |
| --- | --- | --- | --- | --- | --- |
| Windows 11 x64 | MSI and NSIS 0.2.1 pairing candidate | Shipped | Shipped | CPU, RAM cache, NVIDIA GPU/VRAM, disk | Packaged 0.2.1 lifecycle, zero-copy pairing protocol, 3D recovery, direct QUIC, owner relay, restart-resumable storage, signed possession receipts, and two-node autonomous repair; physical laptop pairing pending |
| Windows 10 x64 | Not qualified | Source likely portable | Source likely portable | Same as Windows 11, hardware-dependent | Unexecuted |
| macOS Apple Silicon | Not shipped | Rust source portable | Rust source portable | CPU/GPU whole jobs, RAM cache, disk | Unexecuted; native packaging and discovery adapters required |
| Linux x64 | Not shipped | Rust source portable | Rust source portable | CPU, vendor GPU/VRAM, RAM cache, disk | Unexecuted; GTK dependency warning must be resolved or accepted before release |
| Android | Not shipped | No | Designed edge worker | Charging/idle CPU microtasks and optional encrypted cache | Contract and Governor policy only |
| iPhone/iPad | Not shipped | No | Designed edge worker | Foreground, restart-tolerant microtasks; thermal/battery gated | Contract and Governor policy only |
| Game consoles | Not shipped | No | Designed companion worker where platform policy permits | Foreground restart-tolerant microtasks | Contract and Governor policy only; store/platform approval required |
| Browser/PWA | Not shipped | No | Designed constrained worker | WASM-compatible microtasks, no protected storage | Protocol extension point only |

Fresh Windows x64, Linux x64, and macOS Apple Silicon packages are now defined by the fail-closed
[native distribution workflow](DISTRIBUTION.md). A successful pull-request run qualifies candidate
packaging on its named GitHub-hosted runner; it does not convert an unsigned candidate into a stable
release. Stable Windows and macOS publication additionally requires real platform credentials and
independent signature/notarization verification.

The installable column describes the current 0.2.1 candidate artifacts. Later source-qualified capabilities
do not become packaged or signed merely because they pass a source campaign; each evidence page
states that boundary explicitly.

## Capability boundary

Rampage pools schedulable work and encrypted artifacts. It does not pretend remote RAM or VRAM is
locally addressable hardware. Whole-job placement and independent shards are supported; cross-host
tensor sharding remains disabled until a topology-aware backend passes dedicated correctness,
performance, failure-recovery, and privacy gates.

The five-way Model Fabric planner and desktop toggle are shipped on Windows x64. They may report a
theoretical compatible placement but cannot launch it. Exo/MLX distributed execution is currently a
macOS-focused candidate (with Linux CPU support upstream); vLLM/Ray remains a Linux/WSL homogeneous
CUDA candidate. Neither is bundled or qualified by this release. See [MODEL_FABRIC.md](MODEL_FABRIC.md).

Phones and tablets are useful when their constraints shape the workload: verification, hashing,
embedding slices, preprocessing, evaluation shards, and cache replication. They are intentionally
excluded from protected replicas, latency-critical service, and thermal-unbounded compute.
