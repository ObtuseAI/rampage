# Rampage 0.2.3 fabric proof

Rampage 0.2.3 turns the first successful two-PC pairing into a measurable compute fabric. The
desktop can now qualify a local AI runtime automatically, publish the exact installed model through
an authenticated worker endpoint, and prove sustained compute with node-signed receipts instead of
inferring performance from hardware names.

## What changed

- **Local AI Autopilot:** on Windows, Rampage detects or installs Ollama 0.32.5, waits for its
  loopback API, pulls `qwen3:4b`, and accepts the model only when the complete official artifact
  digest is exactly
  `sha256:359d7dd4bcdab3d86b87d73ac27966f4dbb9f5efdfcc75d34a8764a09474fae7`.
- **No daemon restart after qualification:** the worker refreshes Ollama availability, installed
  model inventory, and workload capabilities while it is running. A newly qualified model becomes
  eligible without restarting the desktop or agent.
- **Signed sustained benchmark:** `rampage benchmark` admits one CPU-only, node-pinned job per
  capable contributor, runs deterministic SHA-256 chains in bounded lanes, and counts a node only
  after its signed execution receipt exists. The desktop exposes the same proof as **Prove my
  speed**.
- **Owner PC joins its own authenticated fabric:** the local worker now consumes a complete signed
  enrollment invitation on first launch and retains a verified controller pin. Local model traffic
  follows the same authenticated QUIC and receipt path as a remote contributor.
- **Autonomous reconnect:** an enrolled worker keeps its identity and retries with bounded backoff
  when the owner controller or network disappears. The packaged campaign kills and restarts the
  controller and requires the same node to publish a fresh signed endpoint without reopening the
  worker app.
- **Real-machine clock tolerance:** signed job, model, storage, capability, and canary leases accept
  no more than five seconds of positive clock skew. Exact expiration and larger future timestamps
  still fail closed. The regression includes the 16 MiB storage lease that failed during the
  physical laptop campaign.
- **Thinking-field isolation:** reasoning-capable Ollama models stay in structured thinking mode.
  Rampage forwards only `message.content`; the private reasoning field is never relabeled as an
  OpenAI or Anthropic answer.
- **Cold-start tolerance:** the native shell gives a clean Windows controller up to 30 bounded
  seconds to create its local token, covering first-run executable scanning without waiting
  indefinitely.

## Proved in this candidate

- The final installed main-PC worker hash matched the staged release sidecar.
- `qwen3:4b` answered a real OpenAI-compatible request through the controller, authenticated QUIC,
  loopback Ollama, and a transcript-matched signed receipt. The answer contained no reasoning trace.
- The final package completed 20,000,000 deterministic hash iterations at 47,735,112 hashes/second
  on four lanes and returned signed receipt
  `019fc94a-35f4-72b0-974c-42fae0b6e078`.
- The packaged worker passed enrollment, encrypted artifact round trip, signed benchmark,
  controller restart recovery, consumed-secret removal, pinned restart, close-to-tray, explicit
  exit, and no-sidecar-leak checks.
- The exact tagged CI-built NSIS matched its generated checksum manifest, installed all six
  payloads, created the desktop and Rampage Shell shortcuts, started the owner fabric, and removed
  the test installation cleanly with no leaked sidecars.

## Honest boundary

This is an **unsigned Windows 11 x64 prerelease candidate**. Verify its published SHA-256 before
running it. The main PC is on a source-identical local 0.2.3 proof package; the exact public CI NSIS
separately passed its install, packaged-owner, shortcut, shutdown, uninstall, and no-leak campaign.
The already-enrolled physical laptop must be upgraded to the public build before its benchmark
adapter, clock-skew fix, automatic local AI, and controller-restart recovery can be measured
together. Phone qualification remains the next native device gate.

Rampage pools work, services, replicas, and encrypted artifacts. It does not make commodity network
RAM or VRAM locally addressable, and unsafe llama.cpp RPC remains blocked.

See [release evidence](RELEASE_EVIDENCE_0.2.3.md), [model fabric](MODEL_FABRIC.md), and
[checksums](SHA256SUMS-0.2.3.txt).
