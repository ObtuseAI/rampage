# Resumable protected storage

Rampage treats donated disks as encrypted content-addressed artifact capacity. It does not mount a
remote filesystem, expose a block device, or pretend that another machine's RAM is local memory.
The current transport moves artifacts up to 64 MiB over an authenticated QUIC application protocol
in independently bounded 4 MiB frames.

## Durable transfer contract

`rampage.artifact-transfer-request.v2` binds every frame to:

- an exact Governor-signed storage lease;
- the enrolled destination node, digest, direction, size, storage class, expiry, and fencing epoch;
- a deterministic peer/digest/direction session ID;
- a fixed 4 MiB maximum frame size, exact chunk index, byte count, and SHA-256 digest; and
- a fresh controller challenge when the worker must prove possession.

The receiving CAS persists the immutable session contract and accepted chunk metadata in SQLite.
Partial payloads are encrypted with the node-local AES-256-GCM key before they enter the transfer
directory. Retrying the exact chunk is idempotent. A different payload at an accepted index is
rejected. A newer Governor lease can renew the same content session, but a lower fencing epoch,
replayed renewal nonce, changed digest, changed size, changed class, or changed media type is denied.

Commit authenticates and decrypts one chunk at a time, recomputes the full SHA-256 address, checks
the exact total size, writes a bounded manifest, and atomically promotes the encrypted directory
into the CAS namespace. A controller or worker process restart therefore resumes from the durable
missing-chunk set instead of restarting byte zero.

Chunk and manifest files are flushed before directory promotion; supported Unix filesystems also
flush the affected directory entries. If an existing content address is objectively malformed,
missing a declared file, or fails its stored ciphertext/full-content digest, Rampage atomically
moves it under the CAS-local `tmp/corrupt` quarantine and rebuilds it from verified plaintext or a
complete authenticated transfer. Authentication failure with a different encryption key is not
classified as corruption and never triggers quarantine.

Retrieval uses one fresh signed GET lease per chunk. The controller writes each returned chunk into
its own encrypted resumable session and commits only after the complete content address verifies.
If the controller already owns that digest under cache or scratch durability, retrieval preserves
that immutable local metadata instead of silently relabeling it to match a protected remote copy.
The local HTTP response still materializes the final payload because that API returns base64; the
mesh path itself never requires a whole-artifact transfer allocation.

## Independently evidenced durability

A successful PUT commit or challenged HEAD probe returns `rampage.artifact-replica-receipt.v1`.
HEAD decrypts/authenticates every encrypted chunk and recomputes the complete content address before
the worker signs the exact node, artifact, lease, session, challenge, verification time, expiry,
and fencing epoch. A GET chunk alone is never represented as whole-artifact possession. The
controller verifies the signature and every binding before the receipt is eligible.

Protected durability is thresholded automatically:

1. fewer than two fresh receipts from distinct enrolled nodes marks the artifact under-replicated;
2. the reconciler probes known holders with fresh challenges;
3. stale or failed evidence stops counting but does not erase the underlying replica record;
4. repair selects a live protected-storage offer that is not any known holder;
5. a signed storage lease authorizes the encrypted resumable copy; and
6. only the new node-signed receipt closes the gap.

The controller reconciles every 60 seconds and exposes an immediate token-protected
`POST /v1/artifacts/repair` operation. Each cycle uses a rotating worklist capped at four complete
possession probes and 128 MiB of verification, reuses unexpired signed receipts, and defers repair
decisions when a stale holder was outside that cycle's proof budget. Autonomous copies are capped
at four per cycle. Owner STOP prevents probes and repairs. Per-change approval is not required
inside this preconfigured storage envelope; authority expansion remains automatically denied.

Workers advertise protected capacity only when the owner has enabled it with
`RAMPAGE_ALLOW_PROTECTED_STORAGE=true`. That is a donation boundary, not an approval prompt for
each repair. Cache, scratch, and protected limits are also enforced inside the worker CAS: active
resumable sessions reserve their class bytes transactionally, and a later transfer fails closed if
committed plus reserved content would exceed the advertised owner contribution.

## Current boundaries

- The transfer API currently caps one artifact at 64 MiB.
- Protected storage uses whole encrypted replicas, not erasure coding.
- Two receipts mean two enrolled nodes, not two directories on one machine.
- An offline holder is retained as a known replica but its stale receipt does not count toward the
  live durability threshold.
- This layer improves artifact availability and data locality; it does not combine remote drives,
  RAM, or VRAM into a transparent local address space.

See [resumable-storage evidence](RESUMABLE_STORAGE_EVIDENCE.md) for the executable proof and exact
qualification boundary.
