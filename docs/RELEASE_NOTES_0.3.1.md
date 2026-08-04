# Rampage 0.3.1 — Recovery without rituals

Rampage 0.3.1 closes the device lifecycle that a real multi-machine desktop product needs. A broken,
stale, or interrupted pairing no longer requires uninstalling the app or finding hidden runtime
files. Recovery is available from both first-run setup and the main header.

## What changed

- **Fix Rampage** safely restarts a consistent native installation without erasing identity or work.
- **Pair again** stops the worker sidecars, removes its old fabric runtime, and returns the device to
  the two-button nearby-pairing screen.
- Owner **Forget** revokes one exact enrolled identity and removes its offer, outstanding assignments,
  reservations, shard sets, Remote Assist sessions, artifact locations, and possession evidence.
- Revocation is appended to the hash-chained ledger and replayed after controller restart, so a stale
  node and offer do not reappear.
- **Factory reset** disables Rampage auto-start and clears only the bounded Rampage runtime after the
  user types `RESET RAMPAGE`. External Ollama model storage is not deleted.
- A redacted recovery receipt makes support diagnosis copyable without exposing controller secrets or
  private keys.
- SDK lifecycle methods are available in Rust, TypeScript, and Python for loopback owner automation.

## Simpler compute outcomes

Rampage now defaults to **Automatic** and auto-assigns technical roles from signed hardware, runtime,
link, power, thermal, and workload facts. Users can override the outcome with four plain choices:
**Biggest AI**, **Fastest AI**, **More Work**, or **Protect This PC**. Detailed model sizing remains in
the advanced disclosure instead of occupying the default surface.

The strategies do not claim that commodity networks create physically shared RAM or VRAM. Whole
models, replicas, independent shards, caches, relays, and engine-native distributed layouts remain
separate lanes with separate qualification gates. The research and delivery sequence is in the
[universal fabric blueprint](UNIVERSAL_FABRIC_BLUEPRINT.md).

## Security and lifecycle boundaries

- Exact destructive confirmations are checked again by native Rust/controller code; the React dialog
  is not the security boundary.
- Reset refuses paths outside the exact Rampage app-runtime shape and refuses a redirected runtime
  root.
- Setup mode launches no fabric sidecars until the device creates or joins a fabric.
- Worker Remote Assist is still off by default, visible when active, limited to a paired owner, and
  bounded by signed 30-second leases plus STOP/revoke.
- Intelligence remains proposal-only. Automatic improvements can act inside the owner's standing
  envelope; authority-expanding proposals are automatically denied.

## Install and recover

1. Install Rampage on the owner PC and laptop with the Windows installer. The installer creates the
   desktop shortcut.
2. Create the fabric on the owner, then choose **Join my fabric** and **Find my fabric** on the laptop.
3. Approve the matching four digits on the owner.
4. If the laptop ever retains a stale identity, choose **Fix Rampage → Pair again**, then approve it
   again. On the owner, use **Advanced recovery → Forget** for the stale enrolled entry.

Package hashes, verification commands, and the remaining physical two-machine gate are recorded in
[the 0.3.1 release evidence](RELEASE_EVIDENCE_0.3.1.md).
