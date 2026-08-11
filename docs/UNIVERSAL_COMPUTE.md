# Universal compute contract

Rampage does not pretend that RAM or VRAM on unrelated machines becomes one transparent hardware
device. It exposes a signed, operation-exact capability fabric. Applications can then use the
distribution pattern that actually matches their workload:

| Pattern | Best use | Current authority |
| --- | --- | --- |
| Whole workload | A complete LLM request, render, transcode, build, or simulation on one stronger node | Shipped for local Ollama chat |
| Independent shard | Evaluation cases, Blender frames, media segments, test partitions, data batches | Shipped for bounded built-in evaluation/hash work; additional adapters require qualification |
| Replica | Concurrent model or service requests | Contract shipped; backend-specific execution requires qualification |
| Streaming service | Interactive inference, remote rendering, encoding, game streaming | Contract shipped; Ollama text streaming is shipped |
| Application-native distributed | Shader workers, render farms, build workers, simulation workers | Contract shipped; no generic vendor adapter is claimed yet |
| Tensor or pipeline parallel | One model split across compatible machines | Planner only until an exact runtime/topology campaign qualifies it |

Every live `ResourceOfferV1` may contain `WorkloadCapabilityV1` records. Each record binds:

- one adapter and an exact set of operations;
- a workload domain and allowed execution patterns;
- the resource classes it can consume;
- its isolation boundary and runtime digest;
- checkpoint, preemption, and network requirements; and
- `shipped`, evidence-backed `qualified`, or non-authorizing `candidate` status.

The Rust Governor rechecks the requested adapter, operation, resources, node, expiry, mobile safety
policy, and signed offer before it mints a lease. A `candidate` profile never authorizes execution.
Unknown adapters are omitted rather than guessed.

## Universal AI gateway

The loopback-only gateway exposes a deliberately bounded text subset:

- OpenAI-style `GET /v1/models` and `POST /v1/chat/completions`;
- OpenRouter-style aliases at `/api/v1/models` and `/api/v1/chat/completions`;
- Anthropic Messages at `POST /v1/messages`;
- capability discovery at `GET /v1/capabilities` and
  `GET /.well-known/rampage-capabilities`.

All three request styles converge on the same exact installed-model selection, one-shot signed
model lease, authenticated QUIC worker channel, bounded streaming response, and signed terminal
transcript receipt. Bearer tokens and Anthropic `x-api-key` authentication are supported. Tools,
vision, audio, provider routing, and cross-host tensor/pipeline launch fail closed until separately
implemented and qualified.

## Continuous self-scan

`GET /v1/diagnostics/self-scan` returns `rampage.fabric-diagnostic-report.v1`. The controller also
runs the scan every 60 seconds and writes a hash-chained event only when the stable evidence digest
changes. The scan currently checks:

- missing or expired offers and operation-exact capability contracts;
- missing authenticated routes and unqualified links;
- high RTT, low bandwidth, thermal pressure, and battery reserve;
- empty local-model inventories and useful idle capacity;
- repeated authority denials and failed terminal receipts; and
- under-replicated protected artifacts; and
- missing, stale, topology-mismatched, or contract-invalid Compute Dividend history.

Each finding includes evidence and a bounded proposal. Promotion is autonomous inside a
preconfigured owner envelope: deterministic replay, quality/reliability/cost, sealed holdout,
adversarial security, independent replication, shadow, and canary/rollback gates must all pass.
There is no per-change approval queue inside that envelope. Authority-critical changes and any path
or risk outside the envelope are automatically denied; the AI cannot widen its own envelope.

When the intelligence sidecar finds every gate present, passing, content-addressed, and independently
replicated where required, it emits `rampage.promotion-candidate.v1`. The loopback-only
`POST /v1/improvements/canary` route makes the Rust Governor revalidate the complete bundle,
reclassify every changed path, enforce the preconfigured project ceiling, and return a signed
`rampage.promotion-canary-lease.v1`. The lease caps traffic and error, latency, and cost regression;
it expires after at most ten minutes and is fenced by the controller epoch. Reusing a proposal ID
with different content is rejected.

R1 and R2 project envelopes are loaded at controller start from
`RAMPAGE_AUTONOMY_R1_PROJECTS` and `RAMPAGE_AUTONOMY_R2_PROJECTS` (comma- or semicolon-separated
UUIDs). R2 also requires the explicit protected-project set. R3 is always denied. This is an owner
configuration boundary, not a per-change approval prompt.

## Durable measurement and automatic break-even

`POST /v1/dividends` accepts only `rampage.fabric-benchmark-result.v1`. Before committing the
projection, the controller reconciles its set, jobs, lease-node mapping, accepted signed receipts,
signed result bodies, digests, rates, and recomputed aggregates. `GET /v1/dividends?limit=24`
returns the bounded chronological ledger projection with previous-scale comparisons.

`POST /v1/plans/break-even` accepts `rampage.break-even-request.v1` and returns a preview-only
`rampage.break-even-plan.v1`. It can choose `use_fabric`, `stay_on_fastest_node`, or
`insufficient_evidence`; it cannot issue a job or lease. The five workload classes apply separate
minimum-gain and p90 safety factors. Artifact movement is not inferred from CPU evidence, and every
projection carries the explicit boundary that it is not a general speed guarantee.

`GET /v1/network/autopilot` returns `rampage.network-autopilot-status.v1`. Direct and owner-relay
labels distinguish observed active transport from advertised candidates. Authority control can use
an authenticated fallback, while interactive AI, remote media, artifacts, and bulk background work
must independently meet the current measured thresholds.

The operational R0 lane already acts without a prompt: the Rust Governor may convert direct signed
evidence of an unroutable, thermally constrained, or low-battery contributor into a reversible
placement exclusion. The controller records the applied constraint and automatically removes it
when fresh evidence clears the threshold. This lane can only reduce scheduling authority; it cannot
enroll peers, add routes, grant resources, or widen an adapter/network/filesystem boundary.

## Honest device roles

Older PCs can be excellent whole-workload, shard, replica, cache, storage, build, render, transcode,
or simulation nodes when measured evidence says they help. Phones and tablets are most useful for
foreground, restart-tolerant, thermally bounded preprocessing, scoring, validation, caching, relay,
sensor, and small-model work. Consoles require platform-approved software and APIs. Rampage never
claims that a device can donate an inaccessible GPU or bypass an operating-system sandbox.
