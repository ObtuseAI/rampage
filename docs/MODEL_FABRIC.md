# Model Fabric

Rampage exposes one owner-facing **Compute Strategy** while keeping model placement evidence and
execution authority separate. The strategy planner, CLI preview, SDK method, and desktop selector
remain read-only and never mint a lease. A separate shipped whole-model lane can execute an exact
installed Ollama model on one authenticated contributor through a dedicated model-session lease.

## The five strategies

| Strategy | Objective | Placement rule |
| --- | --- | --- |
| Maximum Model | Fit the largest useful local LLM | Sum only compatible runtime memory; use qualified pipeline placement when one node cannot fit |
| Speed Boost | Improve one interactive model session | Use low-latency tensor peers only when the conservative speed model exceeds 1.10x; otherwise use the fastest whole-model node |
| Maximum Throughput | Serve the most concurrent work | Create independent whole-model replicas; never describe replicas as a larger or faster single request |
| Efficiency | Reduce resources and energy | Select the smallest qualified whole-model node that fits |
| Autonomous Balanced | Adapt from measured evidence | Prefer a simple whole-model fit; proposal-only intelligence may recommend changes but cannot authorize them |

Maximum Model is the product focus and the default desktop selection. Speed Boost is deliberately
separate because aggregate memory and token speed are different objectives.

## What the planner proves

`POST /v1/model-sessions/plan` accepts `rampage.model-session-request.v1` and returns
`rampage.model-session-plan.v1`. It validates model-weight, KV-cache, context, deadline, and node
bounds; filters expired, hot, disallowed, and low-battery offers; groups model runtimes by exact
backend compatibility key; caps advertised runtime memory by currently available signed resources;
and emits placements, blockers, warnings, visible memory, compatible memory, and a conservative
speedup estimate.

Every response contains:

```json
"execution_authority": "none_preview_only"
```

That value is not a disclaimer pasted onto an operational endpoint. The planning handler has no
code path that mints a capability lease or launches a process.

## Runtime profiles

On Windows, the desktop's Local AI Autopilot detects or installs the exact Ollama 0.32.5 package,
waits for its loopback API, pulls `qwen3:4b`, and verifies the model's complete artifact digest
before marking the runtime ready. The bootstrap is bounded, idempotent, disabled in diagnostic
builds, and never receives controller credentials. The worker refreshes runtime inventory while it
is running, so qualification does not require an agent restart.

The shipped local Ollama adapter advertises a `shipped_local` whole-model profile automatically,
including a bounded inventory of locally installed model identifiers, artifact sizes, and content
digests from `/api/tags`. A machine with separate system RAM and VRAM advertises `hybrid` capacity;
that means one host's Ollama layer offload, never a shared address space across hosts.
Advanced engines must arrive through a strict `rampage.model-runtime-manifest.v1` profile containing
an exact runtime digest, compatibility key, supported parallelism, available model memory, and—when
declared qualified—a certification evidence digest. Invalid manifests are rejected as a unit and
the agent continues without advertising those runtimes.

The two advanced adapter identities are:

- `rampage.exo-mlx.v1` for Exo/MLX compatible clusters.
- `rampage.vllm-ray.v1` for qualified homogeneous vLLM/Ray CUDA clusters.

The manifest is an admission input, not execution permission. The Governor must still verify the
campaign evidence, exact artifacts, selected peers, network allowlist, lease expiry, and fencing
epoch before a future launcher can start a distributed session.

## Current topology thresholds

The ordinary authenticated QUIC benchmark remains a conservative admission signal:

- Maximum Model pipeline preview requires each remote rank to measure at most 25 ms controller RTT
  and at least 250 Mbps in both directions.
- Speed Boost tensor preview requires at most 5 ms RTT and at least 1 Gbps in both directions.
- The speed model grants no predicted improvement below 1.10x.

These thresholds do not certify NCCL, RDMA, GPU-direct paths, or a complete peer-to-peer graph. A
backend campaign must still run the deeper gates in [BACKEND_GATES.md](BACKEND_GATES.md).

## Why adapters—not invented distributed kernels

[Exo](https://github.com/exo-explore/exo) currently demonstrates automatic device discovery,
topology-aware model partitioning, tensor parallelism, and local OpenAI-compatible APIs. Apple’s
[MLX distributed documentation](https://ml-explore.github.io/mlx/build/html/usage/distributed.html)
documents ring, JACCL/Thunderbolt RDMA, NCCL, and MPI communication backends. Current
[vLLM parallelism guidance](https://docs.vllm.ai/en/stable/serving/parallelism_scaling/) describes
tensor parallelism within a node and pipeline parallelism across nodes.

Rampage should govern and measure proven engines rather than write a new tensor kernel or expose an
unsafe raw RPC service. In particular, llama.cpp RPC remains blocked under the security boundary in
[BACKEND_GATES.md](BACKEND_GATES.md).

## Operator preview

```powershell
rampage model-plan local/70b-quantized `
  --weights-gib 40 `
  --kv-cache-gib 4 `
  --strategy maximum-model-size `
  --max-nodes 8

rampage model-plan local/fast-chat `
  --weights-gib 20 `
  --kv-cache-gib 2 `
  --strategy speed-boost
```

The desktop provides the same five-way selection, target model and memory inputs, visible versus
compatible memory, predicted speed, selected ranks, and the first fail-closed gate reason.

## Shipped whole-model gateway

The owner controller exposes a loopback-only OpenAI Chat Completions subset:

- `GET /v1/models` lists only live, eligible model identifiers whose digest is consistent across
  every advertising contributor.
- `POST /v1/chat/completions` supports text-only `system`, `user`, and `assistant` messages,
  non-streaming responses, and SSE streaming.
- `POST /v1/model-sessions/{id}/cancel` cancels an active session. Owner STOP also cancels all
  active sessions and advances the durable authority epoch.

Every route requires `Authorization: Bearer <controller token>`. The controller selects a signed
offer, and the non-agentic Governor mints `rampage.model-session-lease.v1` for one node, exact model
digest, exact runtime, whole-model parallelism, prompt/output bounds, authenticated controller peer,
expiry, one-shot nonce, and fencing epoch. The worker re-queries its loopback Ollama inventory before
execution, verifies the Governor signature, durably consumes the nonce/epoch, and calls only its
configured loopback Ollama origin. The terminal receipt signs the exact output SHA-256, byte count,
state, timestamps, and Ollama-reported usage. The controller verifies the signer and transcript
before returning non-streaming success or the final streaming completion frame.

Thinking-capable models remain in Ollama's structured thinking mode. Rampage ignores the private
`message.thinking` field and signs only the answer transcript emitted through `message.content`.
This prevents reasoning text from being relabeled as an ordinary OpenAI or Anthropic response.

This is real remote whole-model execution. It can let a smaller owner PC use a model that fits a
single stronger contributor. It does not combine memory from multiple hosts for one inference.

## Signed capacity proof

`rampage benchmark` and the desktop's **Prove my speed** action run deterministic SHA-256 chains
under a CPU-only capability lease. The controller creates one node-pinned job per live contributor
that advertises `rampage.benchmark.v1`, admits the complete set atomically, and reports a node only
after its signed receipt exists. Output includes each node's lanes, total hashes, elapsed time,
result digest, receipt ID, hashes/second, aggregate fabric rate, and effective scale over the
fastest node. It is deliberately sustained work—not a hardware-name estimate or a network-speed
claim.

## Next distributed executable gate

The generic durable-authority foundation is now implemented. Controller epochs live in the
hash-chained ledger, survive normal restart, and advance on owner STOP. Job and storage leases carry
that signed epoch; workers and artifact gateways durably consume each nonce once and reject lower
epochs after restart. The end-to-end campaign proves restart recovery, STOP advancement, stale
claim denial, and authenticated artifact transfer under the new rules.

The dedicated whole-model lease, peer binding, model digest revalidation, bounded streaming failure
semantics, cancellation, durable replay protection, deterministic process campaign, and loopback
OpenAI-compatible gateway are shipped for local Ollama. The remaining gate is a genuinely
distributed launcher for pipeline or tensor ranks. It still requires backend-specific process
isolation, exact peer allowlists, model/rank artifact verification, teardown, injected failure, and
performance qualification. Until that campaign passes, Rampage cannot claim that multiple hosts
combine memory for one inference.
