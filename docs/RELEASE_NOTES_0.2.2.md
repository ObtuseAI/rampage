# Rampage 0.2.2 durable pairing candidate

Rampage 0.2.2 fixes the first real two-PC enrollment campaign. A joined Windows worker no longer
falls back to **Create my fabric / Join my fabric** after its one-time invitation expires. The
worker converts that consumed secret into a Governor-verified controller pin, deletes the secret,
and reconnects through authenticated direct QUIC on later launches.

## What changed

- **Durable worker identity:** the desktop recognizes either an unconsumed invitation or a durable
  controller pin as worker state. Reopening the app cannot silently turn a joined worker into an
  owner onboarding flow.
- **No retained enrollment secret:** after the controller accepts enrollment, the worker writes a
  verified pin containing the signed route and controller key, then removes `remote-invite.json`.
- **Truthful worker status:** **Worker active** appears only after the controller acknowledges the
  worker's first signed resource offer. Startup, transport failure, and sidecar exit remain visible
  as bounded connecting or attention states.
- **Restart-safe controller route:** owner controllers reuse a durable UDP mesh port. Existing 0.2.1
  ledgers migrate from the latest signed `mesh.started` event; new data directories use the stable
  default. Workers retain ephemeral local ports.
- **Private-network setup:** the first pairing action can request one Windows UAC elevation and add
  executable-scoped, inbound UDP rules limited to the Private profile for the desktop, controller,
  and worker. No public-profile or arbitrary-program exception is added.
- **Honest sidecar lifecycle:** the native shell observes worker stdout, stderr, and termination and
  does not report a dead worker as active.
- **Installer isolation:** packaged diagnostics receive isolated controller, intelligence, and mesh
  ports, allowing a clean candidate installation to be validated without stopping a live fabric.

## Verification

- The Windows NSIS candidate installed all six executables, created the desktop and Rampage Shell
  shortcuts, started controller/intelligence/node/offer, preserved close-to-tray behavior,
  uninstalled cleanly, and leaked no sidecars.
- The packaged worker consumed an invitation, published a signed artifact endpoint, completed a
  byte-exact encrypted artifact round trip, removed the invitation, restarted from its durable pin,
  published a fresh signed offer, and exited without sidecar leaks.
- Focused Rust suites passed for agent, controller, desktop, ledger, and mesh, including expired
  invitation migration, durable legacy-port reuse, exact newest-event lookup, and stable mesh-port
  binding. Desktop component tests and the production web build also passed.

## Honest boundary

This is an **unsigned Windows 11 x64 prerelease candidate**. Windows may display an unknown-publisher
warning; verify the published SHA-256 checksum. The 0.2.2 physical laptop reconnection is the next
gate after installation. Nearby discovery still requires both devices on a LAN that permits local
peer traffic. Cross-host tensor/pipeline execution remains evidence-gated; Rampage pools schedulable
work and artifacts rather than pretending remote RAM or VRAM is a local hardware bus.

See [release evidence](RELEASE_EVIDENCE_0.2.2.md), [pairing](PAIRING.md), and
[checksums](SHA256SUMS-0.2.2.txt).
