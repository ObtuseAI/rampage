# Rampage mesh

Rampage does not depend on Tailscale, a Tailscale account, or a third-party coordination plane.
Its local API is loopback-only and protected by a random per-install token. Remote workers use the
implemented `rampage-mesh` transport: authenticated direct QUIC, signed controller and worker
endpoints, and separate versioned control and artifact ALPNs. Artifact transfers are capped at 64
MiB, framed independently from JSON control traffic, content-address verified, and authorized by a
Governor-signed storage lease scoped to node, digest, direction, size, class, expiry, and fencing
epoch.

The first release intentionally uses Iroh as a low-level, open-source networking building block.
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
