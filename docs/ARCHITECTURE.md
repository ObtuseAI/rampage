# Architecture

Rampage uses a narrow-waist design. Every interface above and below the waist speaks versioned job,
resource, lease, artifact, receipt, and evidence contracts. This allows desktop UX, agents, model
runtimes, transports, and storage engines to evolve independently without moving authority into an
AI framework.

```text
Arena / Ops Grid / CLI / SDK / DumbMoney cell
                  |
        signed user and project intents
                  v
 Controller -> Governor -> Capability Lease -> Agent
      |             ^                         /  |  \
      |             |                    CPU GPU encrypted CAS
      v             |
 Intelligence sidecar (proposal-only)
      |
 DBOS workflows + Pydantic AI model/tool adapters

All state transitions -> hash-chained evidence ledger
```

## Planes

- **Trust plane (Rust):** identity, policy, admission, leases, fencing, signatures, kill latch,
  promotion, and ledger verification. It remains usable with the intelligence sidecar removed.
- **Control plane (Rust):** discovery, project twins, queues, scheduling, reconciliation, transport
  abstraction, and public APIs.
- **Intelligence plane (Python):** intent compilation, planning, research, building, criticism,
  adversarial review, evolution, and synthesis. It can only request typed capabilities.
- **Experience plane (native Tauri/React):** one-click enrollment, spatial Arena, accessible 2D
  parity, role-aware system tray, close-to-tray, start-at-login, explanations, replay, and owner
  controls. Explicit Quit owns deterministic sidecar cleanup.

## Durable authority

Signing a lease is necessary but not sufficient. The controller keeps a monotonic authority epoch
in the hash-chained SQLite ledger. A normal crash or process restart reuses that durable epoch so
recoverable work remains valid; an explicit owner STOP advances it before any later admission.
Claims and receipts must match the controller's current epoch.

Workers and artifact gateways verify the Governor signature first, then atomically consume the
lease nonce and update their locally durable highest-seen epoch in the local CAS index beside the
encrypted payloads. The same signed lease cannot execute or transfer twice, and a lower epoch
remains rejected after the worker restarts. Lease expiry, one-shot nonce consumption, monotonic
fencing, the local kill latch,
and the evidence event are separate checks so no one volatile flag carries the whole safety claim.

## OnePool is a market of leases, not pooled RAM

Remote RAM cannot safely or efficiently become transparent local RAM across commodity networks.
Rampage pools *work and explicit resources*: it schedules a workload to a suitable machine, grants a
bounded resource lease, moves content-addressed inputs, and retrieves verified outputs. GPU memory is
normally used by moving a whole model/job to the GPU host. Cross-host tensor/model sharding is only
enabled for an adapter that proves topology and engine compatibility.

The Model Fabric makes that distinction visible through five compute strategies. Maximum Model
optimizes compatible aggregate model memory; Speed Boost uses distributed tensor placement only
when conservative link evidence predicts a real improvement; Throughput uses replicas; Efficiency
uses the smallest whole-model fit; Autonomous Balanced may accept proposal-only recommendations.
`ModelSessionPlan` is read-only and always declares `none_preview_only` authority. See
[MODEL_FABRIC.md](MODEL_FABRIC.md).

Resource classes include CPU, GPU, NPU, GPU memory, working-set RAM, cache RAM, cache storage,
scratch storage, protected storage, fetch/relay network, toolchains, runtimes, codecs, licensed
services, and availability/thermal/power constraints.

## Storage fabric

Donated drive space is a distributed content-addressed artifact layer, not a mounted remote drive.
The controller ingests an artifact into its encrypted CAS, then obtains a short-lived signed storage
lease before opening the worker's artifact QUIC protocol. Cache and scratch replicas may be
recomputed; protected data requires at least two declared replicas. Every transfer validates its
SHA-256 address, every storage lease is consumed once, every worker encrypts chunks with its own
local key, and replication, retrieval, input staging, and output registration become ledger events.
Jobs declare artifact references; the controller stages missing inputs before issuing work, and
receipts can return remotely retrievable outputs.

## Scheduling hierarchy

1. Place the whole workload on one capable device.
2. Split independent map tasks into a bounded `ShardSetV1`.
3. Add replicas for throughput or evidence.
4. Prefer data-local execution.
5. Use certified local multi-GPU adapters.
6. Use certified cross-host sharding only when the measured topology wins.

Admission is computed as:

```text
min(host_free, engine_free)
- pending_reservations
- protected_owner_reserve
- safety_guardband
```

Shard-set planning evaluates members in a deterministic order against one evolving provisional
reservation book. It either finds a placement for every shard or returns the exact blocked job with
no execution lease. After planning, the Governor checks every member, all inputs stage, and one
authoritative `shard_set.admitted` event makes the signed leases visible. Every shard then claims,
executes, fails, and receipts independently. The set's success threshold is explicit; partial
success is never silently reported as complete success. Completed status and individual results
survive controller restart through the evidence ledger.

This is useful aggregation across heterogeneous devices without a distributed shared address
space. It does not claim cross-host tensor sharding, coherent remote RAM, or automatic speedup for
work that cannot be partitioned.

For model planning, a signed resource offer may include `ModelRuntimeOfferV1`. The controller groups
only exact compatibility keys, caps runtime claims by observed RAM/VRAM, and returns a potential,
qualified, or capacity-blocked preview. The separate whole-model Ollama gateway may select one
eligible installed-model digest and ask the Governor for `ModelSessionLeaseV1`; it cannot turn a
distributed preview into authority. Pipeline and tensor launch must still pass the separate backend
gates.

## Recursive improvement

The promotion pipeline is `Record -> Analyze -> Mutate -> Prove -> Audit -> Gate -> Enshrine`.
Evidence ascends through schema/policy, deterministic replay, quality/reliability/cost, sealed
holdout, adversarial security, independent replication, shadow, canary, then signed promotion.
Unknown, ambiguous, or missing evidence fails closed.

Promotion has no per-change approval step inside a preconfigured owner envelope. R0 configuration,
R1 allowlisted source, and explicitly enabled R2 protected changes may become bounded canaries only
after every evidence gate passes. R3 authority-critical changes, path escapes, and attempts to
change the envelope are automatically denied. The intelligence sidecar remains proposal-only; the
Rust Governor independently owns authority.

The controller also runs a deterministic self-scan every 60 seconds. It hashes stable metrics and
findings, appends evidence only when that digest changes, and proposes bounded remediation for
capacity, topology, thermal/battery, failure, denial, model-inventory, and storage-replication gaps.

## Backend selection

Execution engines are replaceable adapters below the trust boundary. Whole-job native adapters and
remote whole-model execution through a worker's loopback Ollama are enabled today. The owner-facing
API stays loopback-only and bearer protected; the controller-to-worker hop uses an authenticated
model QUIC protocol, an exact signed lease, and a signed terminal transcript receipt. Distributed
model engines remain subject to the
[backend gates](BACKEND_GATES.md); in particular, llama.cpp RPC is hard-disabled because its current
upstream security state does not meet Rampage's admission standard.

The same trust boundary now exposes bounded OpenAI, Anthropic Messages, and OpenRouter-style text
surfaces. Universal non-AI work is described by signed `WorkloadCapabilityV1` profiles. The profile
binds an exact adapter and operation to its workload domain, execution pattern, resource classes,
isolation, runtime digest, and qualification state. Candidate profiles never authorize execution;
see [Universal compute](UNIVERSAL_COMPUTE.md).
