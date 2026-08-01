# Operations

## Data and ports

The desktop stores runtime state under the platform application-data directory in `runtime`; tests
and operators can override it with `RAMPAGE_DATA_DIR`. The controller binds only to
`127.0.0.1:47831`, intelligence to `127.0.0.1:47832`, and the mesh to an ephemeral UDP endpoint.
The controller token and Governor, mesh, and node keys live inside the runtime directory and must
not be copied between unrelated fabrics.

## Owner flow

The owner installation launches controller, intelligence, and one local worker. The desktop waits
for the controller token before launching dependent sidecars, creates a one-time invite for its own
worker, and then discovers and advertises available resources. Additional invitations are complete
signed JSON documents; transfer them directly to the intended device and never post them publicly.

## Worker flow

A joining desktop verifies the invite's Governor signature and signed endpoint expiry before it
contacts the owner. Enrollment consumes the invitation. The worker persists its identity, discovers
resources, advertises short-lived signed offers, claims only leases addressed to its node, validates
the Governor signature and fencing epoch, executes an allowlisted adapter, and returns a signed
receipt. After enrollment it reconnects using the recorded identity without reusing the invite.

## Stop and recovery

**STOP** writes a local `KILL` latch. The Governor refuses new leases and workers cease claiming
work. Resume requires the exact owner confirmation through the local authenticated API. On restart,
the controller verifies its ledger before serving, reconstructs nodes/offers/idempotency/assignments,
and refuses corrupted history. Desktop exit terminates the complete sidecar process trees.

## Contribution controls

Default storage contribution is 10 GiB, divided into cache and scratch. Set
`RAMPAGE_STORAGE_CONTRIBUTION_GB` to a value up to 1024 to change it. Protected storage is disabled
unless `RAMPAGE_ALLOW_PROTECTED_STORAGE=true`, and the CAS refuses a protected object without at
least two declared replicas. `RAMPAGE_EDGE_FOREGROUND=true` is required for edge eligibility.

Rampage detects local Ollama at `127.0.0.1:11434`; `RAMPAGE_OLLAMA_URL` may select a different plain
HTTP loopback IP origin for testing or a non-default local service port. Hostnames, credentials,
paths, queries, fragments, HTTPS, and non-loopback destinations are rejected. The Ollama adapter is
not advertised when the endpoint is unavailable, and models are never downloaded silently.

## OpenAI-compatible model gateway

The owner controller exposes `http://127.0.0.1:47831/v1`. Use the controller token as the bearer API
key. The desktop **Copy API setup** action provides `OPENAI_BASE_URL` and `OPENAI_API_KEY`; protect
the copied token like any local credential. `GET /v1/models` returns eligible consistent installed
models. Chat Completions accepts a bounded text-only subset and supports SSE. The response header
`x-rampage-session-id` identifies the explicit cancel route.

Unknown request fields are rejected. Prompts are capped at one MiB, outputs at 32,768 requested
tokens and 16 MiB of transcript, model aliases with conflicting digests are hidden, and execution
cannot download a missing model. Owner STOP cancels active controller sessions and fences later
authority. A client disconnect drops the QUIC/loopback request path rather than leaving the
controller waiting for an unobserved result.

## Artifact flow

The desktop node inspector accepts files up to 64 MiB. Selecting an ordinary owner node encrypts the
file in the controller CAS; selecting a remote node with a signed artifact endpoint also replicates
it to that worker. Equivalent CLI operations are `artifact-put`, `artifact-get`,
`artifact-replicate`, `artifact-retrieve`, and `artifact-hash`.

Jobs reference immutable artifact digests. Admission verifies controller metadata and stages missing
inputs to the selected worker before the execution lease is published. Worker output references are
accepted only inside a valid signed execution receipt and become recorded remote replicas. Retrieval
uses a new signed storage lease and revalidates content addressing before materializing bytes.

## Pooled shard flow

`rampage shard-plan PARTITION...` is a read-only preview. It plans every independent job against
the same reservation snapshot and reports whether complete placement is possible. `rampage
shard-run PARTITION...` repeats validation, stages every declared input, records one durable
all-or-nothing admission, and waits for signed receipts. The desktop Portal exposes the same safe
proof as **Pool a proof across devices**. A bounded success threshold is part of the contract;
`shard-status` reports admitted, running, succeeded, failed, or ambiguous members without filling
missing telemetry with synthetic success.

## Failure states

- Missing or invalid evidence: deny promotion or execution.
- Expired, mismatched, unsigned, or fenced lease: do not execute.
- Backend error: return a signed failed receipt so reservations can be reconciled.
- Lost edge device: retry only restart-tolerant work elsewhere.
- No compatible offer: leave the job unassigned and explain the resource/policy mismatch.
- Ledger verification failure: refuse controller startup; preserve files for investigation.
