# Model Fabric

Rampage exposes one owner-facing **Compute Strategy** while keeping model placement evidence and
execution authority separate. The initial strategy plane is implemented as a read-only controller
planner, CLI command, TypeScript SDK method, and desktop selector. It never starts a backend or
issues a lease.

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

The shipped local Ollama adapter advertises a `shipped_local` whole-model profile automatically.
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

## Next executable gate

The generic durable-authority foundation is now implemented. Controller epochs live in the
hash-chained ledger, survive normal restart, and advance on owner STOP. Job and storage leases carry
that signed epoch; workers and artifact gateways durably consume each nonce once and reject lower
epochs after restart. The end-to-end campaign proves restart recovery, STOP advancement, stale
claim denial, and authenticated artifact transfer under the new rules.

The distributed launcher and loopback OpenAI-compatible model gateway are not shipped yet. They
still require a dedicated model-session lease, backend process isolation, peer-specific allowlists,
model/runtime digest verification, streaming failure semantics, deterministic process cleanup, and
a completed backend qualification campaign. Until those exist, Rampage may prove a placement
candidate but cannot claim that cross-host single-model inference is operational.
