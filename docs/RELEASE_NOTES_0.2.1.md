# Rampage 0.2.1 pairing candidate

Rampage 0.2.1 makes adding a Windows machine feel like pairing a trusted accessory instead of
configuring a distributed system. Install it on the laptop, let the laptop wait, compare four
digits on the main PC, and approve once. The same release also removes the packaged Arena's
external-font dependency that could leave the 3D view indefinitely initializing.

## What changed

- **Zero-copy nearby pairing:** no account, IP address, terminal, or JSON invitation in the normal
  flow. A joining laptop can begin first and wait for the owner PC.
- **Human-verifiable security:** both devices derive the same four-digit short authentication
  string from an ephemeral X25519 transcript. The digits verify the channel; they are not the
  enrollment secret.
- **Encrypted enrollment:** the full Governor-signed invitation travels only as AES-256-GCM
  ciphertext under an HKDF-SHA-256-derived key. The laptop returns an authenticated completion
  receipt before restarting into worker mode.
- **Narrow discovery authority:** the owner listens only during an explicit three-minute window;
  requests expire, datagrams and labels are bounded, unknown fields are rejected, and per-source
  plus global pending limits constrain resource abuse. Controller APIs remain loopback-only.
- **Reliable packaged 3D:** node labels are generated locally as canvas textures, so the Arena no
  longer waits on an external font blocked by desktop CSP. Slow loading, WebGL absence, and render
  failures all provide an accessible Ops Grid escape path.
- **Native lifecycle polish:** the installer still creates the desktop shortcut automatically,
  packages all five sidecars plus the desktop, supports the system tray, and now builds correctly
  from Windows PowerShell 5.1 as well as PowerShell 7.
- **Version consistency:** desktop Rust, Tauri, desktop/edge packages, TypeScript/Python SDKs, and
  intelligence metadata are all bound to 0.2.1 by the release verifier.

## Verification

- Secure pairing unit and loopback-network tests cover matching key/code derivation, authenticated
  invite confidentiality, tamper rejection, datagram bounds, replay-safe retries, rate limiting,
  and the discovery → approval → completion exchange.
- Desktop tests cover zero-copy onboarding, a single explicit owner approval, authenticated
  completion, finite loading, render failure, real WebGL absence, and accessible grid parity.
- The repository gates pass Rust formatting, workspace tests, all-target Clippy with warnings
  denied, desktop/edge/TypeScript SDK tests and builds, Ruff, strict Mypy, intelligence tests, and
  Python SDK tests.
- Separate-process campaigns pass restart/fencing and shard recovery, authenticated direct QUIC,
  encrypted resumable storage and autonomous repair, and OpenAI/Anthropic/OpenRouter-style gateway
  translation with signed receipt evidence.
- The final NSIS candidate installs all six executables, creates the desktop and Rampage Shell
  shortcuts, starts a controller, intelligence service, node, and offer, preserves close-to-tray,
  exits cleanly, uninstalls cleanly, and leaks no sidecars. MSI administrative extraction contains
  the same six executables and generated desktop shortcut.

## Honest boundary

This is an **unsigned Windows x64 prerelease candidate**. Windows may show a reputation warning;
verify the published SHA-256 checksum. Nearby discovery requires both devices to share a private
LAN and can be blocked by guest-network isolation, VPN policy, or a firewall that denies private
LAN traffic. Cross-host tensor/pipeline model execution remains evidence-gated; 0.2.1 does not
pretend arbitrary remote RAM or VRAM becomes a local hardware bus.

See [release evidence](RELEASE_EVIDENCE_0.2.1.md), [pairing](PAIRING.md), and
[checksums](SHA256SUMS-0.2.1.txt).
