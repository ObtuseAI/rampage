# Distributed backend gates

Rampage does not equate “an upstream engine supports multiple machines” with permission to expose
that engine to the fabric. The Governor admits only an allowlisted adapter whose exact version,
runtime boundary, topology, failure behavior, and measured benefit have passed these gates.

## Current decisions

| Backend | State | Decision |
| --- | --- | --- |
| Rampage native whole-job adapters | Shipped | Preferred for heterogeneous consumer machines and independent work |
| Local Ollama | Shipped | Loopback-only, model must already exist, signed lease and receipt required |
| Model strategy planner | Shipped | Read-only previews for maximum model, speed, throughput, efficiency, and balanced objectives; no execution authority |
| Durable authority substrate | Shipped | Hash-chained controller epoch, one-shot durable worker/storage nonces, restart-preserving recovery, and STOP fencing |
| Exo / MLX distributed | Candidate | Exact runtime/compatibility manifest and backend-specific topology campaign required before launch |
| llama.cpp RPC | Blocked | No adapter or port exposure until an upstream patched release and Rampage isolation campaign exist |
| vLLM single-node | Candidate | Linux/WSL qualification required; keep tensor parallelism inside a proven node topology |
| vLLM multi-node pipeline/Ray | Candidate | Only for a model that cannot fit one node and only after measured topology beats whole-job/replica baselines |
| PyTorch pipeline | Research | API is alpha; no automatic promotion or generated deployment |

### Why llama.cpp RPC is blocked

The current [llama.cpp RPC documentation](https://github.com/ggml-org/llama.cpp/blob/master/tools/rpc/README.md)
labels the backend proof-of-concept, fragile, and insecure. The March 2026
[CVE-2026-34159 advisory](https://github.com/ggml-org/llama.cpp/security/advisories/GHSA-j8rj-fmpv-wcxw)
describes unauthenticated remote code execution and lists no patched version. Wrapping that raw TCP
port in Rampage authentication would reduce reachability but would not make the parser or process a
safe execution boundary. Rampage therefore does not ship, launch, proxy, or firewall-punch this
backend.

### Preferred advanced direction

Current [vLLM parallelism guidance](https://docs.vllm.ai/en/stable/serving/parallelism_scaling/)
uses tensor parallelism within a node and pipeline parallelism across nodes, with Ray or native
multiprocessing as the runtime. It also calls for identical execution environments and recommends
pre-positioned model data. Rampage will treat those as hard manifest requirements, not setup tips.

NVIDIA's current [NCCL networking guidance](https://docs.nvidia.com/deeplearning/nccl/user-guide/docs/troubleshooting/networking_troubleshooting.html)
separately measures bandwidth and latency and inspects interface/RDMA health. Rampage's lightweight
QUIC observation is an admission signal for ordinary work; it is not sufficient certification for
NCCL, GPUDirect RDMA, tensor parallelism, or pipeline parallelism. A backend-specific campaign must
also capture GPU-to-GPU and GPU-to-NIC topology and compare host-memory with GPU-memory paths.

PyTorch's [pipeline API](https://docs.pytorch.org/docs/stable/distributed.pipelining.html) is marked
alpha. Rampage may use it in research fixtures, but not as an autonomous production authority.

## Required evidence before enabling a distributed model adapter

1. Pin an upstream source revision and dependency lock with no unresolved critical advisory in the
   exposed path.
2. Run the engine as an unprivileged, disposable workload with no host shell, arbitrary mounts,
   inherited credentials, or public listener.
3. Verify model, container, runtime, and adapter content digests before launch.
4. Require a signed Rampage lease whose network allowlist contains only the selected peers.
5. Measure point-to-point latency, sustained host transfer, GPU-memory transfer where supported,
   GPU-to-NIC topology, VRAM capacity/bandwidth, thermal stability, and reconnect behavior.
6. Compare time-to-first-token, tokens/second, energy/token, load time, and failure recovery against
   single-node, whole-job, and replica baselines on the same sealed workload.
7. Inject worker loss, stale lease, corrupted model chunk, network partition, and controller loss;
   prove fencing, cleanup, and deterministic recovery.
8. Promote only when the measured configuration wins its preregistered objective and does not
   regress safety or local responsiveness. Promotion cannot modify the Governor or these gates.

This design lets Rampage adopt a safer or faster backend later without letting a model or plugin
turn an experimental network service into authority.

The generic authority substrate for gate 4 and the stale-lease portion of gate 7 is implemented:
signed job and storage authority carries the controller epoch; worker-local nonce consumption and
highest-seen epochs survive restart; owner STOP advances the hash-chained epoch; and the controller
rejects claims and receipts from the old generation. A distributed backend still needs its own
model-session lease, peer allowlist enforcement, process teardown, and injected-failure campaign.

The implemented strategy and runtime-profile contracts are described in
[MODEL_FABRIC.md](MODEL_FABRIC.md). A `ready` planning result is still a preview; it cannot mint a
lease or start a backend.
