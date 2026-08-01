# Rampage platform matrix

This matrix distinguishes shipped, source-portable, and architectural support. “Designed” is not a
binary release claim.

| Platform | Installable 0.1 package | Controller | Worker | Intended donation profile | Evidence |
| --- | --- | --- | --- | --- | --- |
| Windows 11 x64 | MSI and NSIS | Shipped | Shipped | CPU, RAM cache, NVIDIA GPU/VRAM, disk | Full local, direct-QUIC, artifact, Ollama, desktop, and installer gates |
| Windows 10 x64 | Not qualified | Source likely portable | Source likely portable | Same as Windows 11, hardware-dependent | Unexecuted |
| macOS Apple Silicon | Not shipped | Rust source portable | Rust source portable | CPU/GPU whole jobs, RAM cache, disk | Unexecuted; native packaging and discovery adapters required |
| Linux x64 | Not shipped | Rust source portable | Rust source portable | CPU, vendor GPU/VRAM, RAM cache, disk | Unexecuted; GTK dependency warning must be resolved or accepted before release |
| Android | Not shipped | No | Designed edge worker | Charging/idle CPU microtasks and optional encrypted cache | Contract and Governor policy only |
| iPhone/iPad | Not shipped | No | Designed edge worker | Foreground, restart-tolerant microtasks; thermal/battery gated | Contract and Governor policy only |
| Game consoles | Not shipped | No | Designed companion worker where platform policy permits | Foreground restart-tolerant microtasks | Contract and Governor policy only; store/platform approval required |
| Browser/PWA | Not shipped | No | Designed constrained worker | WASM-compatible microtasks, no protected storage | Protocol extension point only |

## Capability boundary

Rampage pools schedulable work and encrypted artifacts. It does not pretend remote RAM or VRAM is
locally addressable hardware. Whole-job placement and independent shards are supported; cross-host
tensor sharding remains disabled until a topology-aware backend passes dedicated correctness,
performance, failure-recovery, and privacy gates.

Phones and tablets are useful when their constraints shape the workload: verification, hashing,
embedding slices, preprocessing, evaluation shards, and cache replication. They are intentionally
excluded from protected replicas, latency-critical service, and thermal-unbounded compute.
