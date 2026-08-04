# Rampage platform matrix

This matrix distinguishes shipped, source-portable, and architectural support. “Designed” is not a
binary release claim.

| Platform | Installable package | Controller | Worker | Intended donation profile | Evidence |
| --- | --- | --- | --- | --- | --- |
| Windows 11 x64 | MSI and NSIS 0.3.0 Remote Assist candidate | Shipped | Shipped | CPU, RAM cache, NVIDIA GPU/VRAM, disk, exact local Ollama models; optional paired-owner desktop view/control | 0.2.3 packaged owner/worker lifecycle proof plus source-qualified 0.3.0 Remote Assist contracts, authenticated transport, worker opt-in, visible active state, short signed leases, replay fencing, and STOP/revoke; exact 0.3.0 package hashes and physical laptop control remain gated by the release evidence |
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

The installable column describes the current 0.3.0 candidate lane. Later source-qualified capabilities
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

Phones and tablets are useful when their constraints shape the workload. The current native source
ships only foreground-safe hashing and independent evaluation shards; embedding, preprocessing, and
cache roles remain future adapter work. Edge devices are intentionally excluded from protected
replicas, latency-critical service, and thermal-unbounded compute.

## Remote Assist boundary

Remote Assist is a Windows-worker capability, not general administrator access. A paired worker must
enable it locally before advertising the exact capability. The owner then receives at most one visible
session per worker through Rampage's authenticated mesh, governed by renewable signed leases of no
more than 30 seconds. Turning the worker toggle off, closing the session, or using STOP revokes access.

The backend captures and injects input only into the current unlocked interactive desktop. Windows
integrity isolation remains authoritative: Rampage cannot cross UAC, the lock screen, the secure
desktop, or a higher-integrity application, and it does not expose a shell or elevation primitive.
