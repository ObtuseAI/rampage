# Resumable protected-storage evidence

Status: **source-qualified on Windows 11 x64; not yet represented as a newly signed installer
release**.

## Executable proof

The milestone is covered at three levels:

1. `rampage-storage` tests interrupt an encrypted multi-chunk PUT, close the store, reopen the same
   SQLite/CAS directory, renew authority at a higher epoch, reject the stale epoch, finish missing
   chunks, and commit the exact content address. Duplicate chunks are idempotent; conflicting
   chunks, incomplete commit, oversized manifests, and oversized ciphertext files fail closed.
   Additional recovery tests corrupt a committed ciphertext, prove it is quarantined and rebuilt,
   and prove that opening the same object with the wrong key is denied without quarantine.
2. `rampage-agent` terminates a real worker gateway after the first 4 MiB QUIC frame, closes its
   endpoint and CAS, reopens both with the same durable identity/storage keys, receives a missing
   set containing only chunk two, commits it, and returns a verifiable node-signed receipt.
3. `scripts/mesh-e2e.ps1` enrolls two independent workers, transfers a 4 MiB-plus protected
   artifact in two frames to worker one, invokes autonomous reconciliation, proves a second
   encrypted copy on worker two, verifies two fresh signed receipts, retrieves the bytes exactly,
   stages a job input, and retrieves the signed job output.

Latest local two-node evidence directory:

`output/mesh-e2e-14e6bceb4779405c972b3df70dd3b53d`

Observed result:

```text
result: PASS
transport: authenticated_direct_quic
transfer_chunks: 2
signed_replica_receipt: true
independent_replicas: 2
autonomous_repair: true
artifact_round_trip: true
encrypted_at_rest: true
```

## Qualification gates

Validated on **2026-08-01 America/Chicago**:

| Gate | Result |
| --- | --- |
| Rust format | `cargo fmt --check` — PASS |
| Rust workspace | `cargo test --workspace --all-features` — 106 tests PASS |
| Rust lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` — PASS |
| TypeScript SDK | 11 tests plus declaration/build check — PASS |
| Python SDK | 10 tests — PASS |
| Complete repository harness | Desktop, intelligence, SDK, control-plane, mesh, and deterministic model-gateway campaigns — PASS |
| Supply chain | JavaScript and Python: no known vulnerabilities; RustSec: no vulnerability failure and 18 tracked maintenance/unsoundness warnings across 739 locked crates |

Companion fresh process evidence:

- control plane: `output/e2e-afefdbdf16124560bf9debe6c02b940b`;
- resumable two-worker mesh: `output/mesh-e2e-14e6bceb4779405c972b3df70dd3b53d`; and
- universal model gateway: `output/ollama-e2e-c22357fedcd14fe5a21b7ba13d6652fb`.

The host was simultaneously used for gaming during the final qualification run. Functional,
cryptographic, recovery, and deterministic contract assertions remain valid; observed compile
times, RTT, link throughput, and staging estimates from this run are not performance qualification.

## Security properties exercised

- exact enrolled controller and worker QUIC identities;
- signed storage leases and durable nonce/epoch fencing;
- deterministic restart sessions scoped by peer, digest, and direction;
- 4 MiB request/response payload limits and 64 MiB artifact limit;
- encrypted partial files and encrypted committed CAS chunks;
- exact per-chunk and full-artifact SHA-256 validation;
- node-signed fresh whole-artifact challenge receipts, never chunk-only possession claims;
- two distinct node IDs for protected durability;
- rotating verification capped at four proofs/128 MiB per cycle;
- autonomous repair capped inside the owner storage envelope; and
- owner STOP preserved as the final operational authority.

## Read-only audit follow-through

An interrupted, read-only Kimi CLI pass did not produce a final report, so it is not treated as an
independent audit attestation. Its saved reasoning did identify candidates that were then verified
against source and remediated locally: intelligence now fails closed with a token isolated from
controller authority; native tray/UI STOP reconciles to a restart-safe fencing marker; protected
retrieval preserves an existing local CAS metadata contract; key files reject symlinks and use
owner-only Unix permissions; canonical digests reject uppercase; event API pages are capped; CAS
promotion flushes data and quarantines only objective corruption; and replica verification has a
rotating byte/count budget. Active sessions also reserve their cache/scratch/protected class quota
inside the CAS transaction. Suspicions contradicted by source—such as an unrestricted remote mesh
control surface—were not reported as findings. The remaining ledger lifecycle limitation is stated
explicitly: reads and replay memory are paged and bounded, while startup time is still linear until
a separately qualified signed-checkpoint/archive design exists.

## Honest boundary

This proves restart resumption on the local qualification host and independent two-process worker
replication over authenticated direct QUIC. It does not yet prove public-WAN interruption across
every NAT, erasure coding, artifacts larger than 64 MiB, disk-loss recovery on physical machines,
or a newly signed multi-platform installer. Those remain separate qualification work.
