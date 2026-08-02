# Rampage mesh

Rampage does not depend on Tailscale, a Tailscale account, or a third-party coordination plane.
Its local API is loopback-only and protected by a random per-install token. Remote workers use the
implemented `rampage-mesh` transport: authenticated direct QUIC, signed controller and worker
endpoints, and separate versioned control and artifact ALPNs. Artifact protocol v2 caps one object
at 64 MiB and one request or response payload at 4 MiB. Every frame is content-address verified and
authorized by a Governor-signed storage lease scoped to node, digest, direction, size, class,
expiry, and fencing epoch.

Rampage intentionally uses Iroh as a low-level, open-source networking building block.
Rampage owns device enrollment, peer allowlists, signed endpoint records, private relay selection,
capability leases, and payload protocols. This is analogous to using rustls instead of inventing
new cryptography: it avoids a product dependency without creating an unsafe home-grown protocol.

The transport supports two policy modes:

- `local_only`: direct UDP/QUIC with no relay and no external discovery service;
- `private_relay`: one or more explicit HTTPS relay URLs operated by Rampage or the owner.

There is deliberately no mode that silently selects the dependency's public relays. A transport
identity proves who is on the other end; it does not authorize it. Before enrollment, the QUIC key
must equal the invite's signed identity key. After enrollment, every accepted offer, claim poll, and
receipt must come from the public key recorded for that node. The gateway exposes no remote route
for minting invitations, submitting jobs, changing policy, or stopping/resuming the fabric.

Workers advertise their artifact endpoint inside their signed, short-lived resource offer. The
controller verifies that its endpoint identity equals the enrolled identity before using it. A
worker artifact server accepts only the exact controller endpoint pinned by its enrollment invite.
Inputs are staged before an execution lease becomes claimable; worker outputs remain encrypted in
the donated CAS and appear as retrievable artifact references in the signed execution receipt.

PUT sessions have deterministic peer/digest/direction IDs and persist their immutable contract,
accepted chunk digests, and consumed renewal nonces beside the encrypted CAS. After either process
restarts, the sender asks for the durable missing-chunk set and resumes without retransmitting
accepted frames. Commit authenticates chunks incrementally, verifies the whole address and size,
then atomically promotes the encrypted object. Retrieval uses a fresh GET lease for each bounded
chunk, so a replay cannot read the next frame.

A committed replica and each challenged HEAD probe return a possession receipt signed by the
enrolled worker identity. For protected artifacts the controller counts only fresh receipts from
distinct node IDs. A challenged HEAD authenticates all chunks and recomputes the full address; a
single GET chunk produces no possession receipt. The rotating 60-second worklist is capped at four
proofs/128 MiB and four repairs, and it cannot decide to repair from evidence deferred by that
budget. Owner STOP suppresses both probing and repair. See
[Resumable protected storage](RESUMABLE_STORAGE.md).

## Owner relay service

The workspace now ships `rampage-relay`, not just a relay URL setting. It exposes Iroh's bounded
relay wire primitive behind Rampage policy:

- the public URL must be credential-free HTTPS;
- the plaintext listener always binds only to loopback, while built-in public mode loads bounded
  explicit certificate and private-key files;
- the controller exports a ten-minute `rampage.relay-access-manifest.v1` containing only the
  controller and enrolled endpoint identities;
- the Governor signs the manifest and the relay re-verifies schema, fabric binding, generation,
  expiry, endpoint syntax, and signature before every cached admission window;
- missing, stale, unreadable, tampered, or over-large authorization denies every new connection;
- per-endpoint, total-connection, receive-rate, burst, key-cache, and loopback-metrics thresholds
  bound resource exhaustion;
- the joining worker enables only the relay URLs carried by the verified signed controller record.

The integration proof disables all IP transports on two endpoints, leaving the owner relay as the
only possible path, then exchanges an authenticated QUIC payload. This demonstrates a real fallback
path rather than a configuration object. See [Owner relay](OWNER_RELAY.md) for deployment.
