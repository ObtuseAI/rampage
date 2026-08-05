# Rampage 0.3.1 — Recovery without rituals

Rampage 0.3.1 closes the device lifecycle that a real multi-machine desktop product needs. A broken,
stale, or interrupted pairing no longer requires uninstalling the app or finding hidden runtime
files. Recovery is available from both first-run setup and the main header.

Candidate 2 also recognizes an owner's cryptographically matching self-enrolled local agent without
confusing it for a worker-role conflict. The fix was added after a physical in-place owner upgrade,
and foreign-controller pins remain fail-closed.

Candidate 3 fixes the next physical-laptop discovery failure. Rampage now advertises on every active
LAN interface instead of trusting Windows to choose one path for global broadcast, and pairing
lifetimes are bounded independently on each device instead of requiring synchronized wall clocks.
The normal flow is one **Find my fabric** action on the laptop; the owner detects that request
automatically and asks for one device approval.

Candidate 4 closes a worker-lifecycle race found during the physical re-pair. Pair again now waits
for every managed sidecar process to exit before rotating the runtime. While the authoritative setup
marker is present, accepting a new invitation can remove only a fixed allowlist of stale worker
credentials left by an older retiring process; active owner and worker identities are never
self-deleted. The Windows firewall marker is also bound to the current installation directory, so
an upgrade cannot silently retain private-network rules for obsolete binaries.

Candidate 5 fixes the first-run role transition that caused the repeated “already enrolled” loop.
An empty runtime previously started a complete owner fabric before the Create/Join choice, so a
laptop could hold an owner marker and legitimate local controller pin while its UI still offered
**Join my fabric**. First run is now neutral and authority-free. Choosing Join creates a protected
transaction that may retire only an unconfirmed legacy bootstrap; confirmed owners and active
workers remain protected. The owner listens for nearby requests automatically, and pairing now asks
for one device approval on the main PC with no verification code to type or compare.

Candidate 6 closes the last owner-notification gap. A newly discovered laptop now restores a
tray-hidden owner window and names the waiting machine in the tray tooltip. The alert fires once per
pending request, so the laptop's normal discovery retransmissions cannot repeatedly interrupt the
owner's foreground work.

Candidate 7 closes the alert lifecycle before publication. Windows attention and the temporary tray
label now return to the normal owner state after approval, rejection, or request expiry, with an
exact raise-once/clear-once regression test.

## What changed

- **Fix Rampage** safely restarts a consistent native installation without erasing identity or work.
- **Pair again** stops the worker sidecars, removes its old fabric runtime, and returns the device to
  the two-button nearby-pairing screen.
- Sidecar shutdown has a bounded exit barrier, and setup-only invitation persistence clears a fixed
  stale-worker allowlist so an older process cannot trap the next attempt in “already enrolled.”
- Owner **Forget** revokes one exact enrolled identity and removes its offer, outstanding assignments,
  reservations, shard sets, Remote Assist sessions, artifact locations, and possession evidence.
- Revocation is appended to the hash-chained ledger and replayed after controller restart, so a stale
  node and offer do not reappear.
- **Factory reset** disables Rampage auto-start and clears only the bounded Rampage runtime after the
  user types `RESET RAMPAGE`. External Ollama model storage is not deleted.
- A redacted recovery receipt makes support diagnosis copyable without exposing controller secrets or
  private keys.
- SDK lifecycle methods are available in Rust, TypeScript, and Python for loopback owner automation.
- Nearby discovery sends to multicast, global broadcast, and each active directed LAN broadcast,
  while excluding loopback, link-local, and point-to-point tunnel interfaces.
- The owner derives request expiry from its own open window and the laptop displays its own bounded
  countdown. Remote clocks cannot extend the window or silently prevent discovery.
- Private-network firewall readiness records the exact installation directory and regenerates the
  three scoped Windows rules when that directory changes.
- Empty runtimes launch no controller, agent, or fabric authority before the person chooses a role.
- **Join my fabric** is transaction-bound: invitation persistence is rejected unless the protected
  join intent is active, and a confirmed owner fabric cannot be erased by the join path.
- The owner app keeps its bounded private-LAN listener ready automatically and surfaces only active
  device requests. The laptop needs no copied value, address, or verification code.
- A new nearby request restores a tray-hidden owner window without taking keyboard focus, while
  duplicate discovery packets reuse the existing request without repeating the alert.

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
3. Rampage automatically shows the laptop on the owner. Choose **Approve this machine**.
4. If the laptop ever retains a stale identity, choose **Fix Rampage → Pair again**, then approve it
   again. On the owner, use **Advanced recovery → Forget** for the stale enrolled entry.

Package hashes, verification commands, and the remaining physical two-machine gate are recorded in
[the 0.3.1 release evidence](RELEASE_EVIDENCE_0.3.1.md).
