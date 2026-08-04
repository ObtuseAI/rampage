export interface RampageHealth {
  service: "rampage-controller";
  status: "ready" | "stopped";
  authority: "non-agentic-governor";
  kill_latch: boolean;
  mesh_mode: "local_only" | "private_relay";
  mesh_endpoint_id: string;
  mesh_sockets: string[];
}

export interface EnrollmentInvite {
  schema: "rampage.enrollment-invite.v1";
  invite_id: string;
  enrollment_code: string;
  expires_at: string;
  controller_mesh: {
    schema: "rampage.mesh-endpoint.v1";
    endpoint_id: string;
    direct_addresses: string[];
    relay_urls: string[];
    issued_at: string;
    expires_at: string;
    signature: string;
  };
  governor_public_key: string;
}

export interface NodeIdentity {
  schema: "rampage.node-identity.v1";
  node_id: string;
  owner_id: string;
  display_name: string;
  device_kind: "desktop" | "laptop" | "server" | "steam_deck" | "phone" | "tablet" | "console";
  platform: string;
  public_key: string;
  enrolled_at: string;
  fencing_epoch: number;
}

export interface NodeRevocationReceipt {
  schema: "rampage.node-revocation-receipt.v1";
  node_id: string;
  revoked: true;
  remote_assist_sessions_closed: number;
}

export interface ExecutionReceipt {
  schema: "rampage.execution-receipt.v1";
  receipt_id: string;
  lease_id: string;
  job_id: string;
  node_id: string;
  state: "succeeded" | "failed";
  result?: unknown;
  stdout_digest?: string;
  stderr_digest?: string;
  metrics: Record<string, number>;
}

export type StorageClass = "cache" | "scratch" | "protected";

export interface ArtifactRef {
  schema: "rampage.artifact-ref.v1";
  digest: string;
  size_bytes: number;
  media_type: string;
  storage_class: StorageClass;
  encrypted: boolean;
}

export interface ArtifactReplicaReceipt {
  schema: "rampage.artifact-replica-receipt.v1";
  receipt_id: string;
  session_id: string;
  lease_id: string;
  node_id: string;
  digest: string;
  size_bytes: number;
  storage_class: StorageClass;
  challenge_nonce: string;
  verified_at: string;
  expires_at: string;
  fencing_epoch: number;
  signature: string;
}

export interface ArtifactReplicationResult {
  artifact: ArtifactRef;
  node_id: string;
  storage_lease_id: string;
  transfer_session_id: string;
  resumed_chunks: number;
  chunk_count: number;
  replica_receipt: ArtifactReplicaReceipt;
}

export interface ProtectedStorageReconciliation {
  schema: "rampage.protected-storage-reconciliation.v1";
  status: "reconciled";
  fresh_replica_receipts: number;
  per_change_approval_required: false;
  authority_expansion: "denied";
}

export interface LinkBenchmark {
  schema: "rampage.link-benchmark.v1";
  controller_endpoint_id: string;
  observed_at: string;
  expires_at: string;
  rtt_micros_p50: number;
  uplink_bps: number;
  downlink_bps: number;
  transfer_bytes: number;
  samples: number;
  transport: "authenticated_quic";
}

export interface ResourceOffer {
  schema: "rampage.resource-offer.v1";
  offer_id: string;
  node_id: string;
  observed_at: string;
  expires_at: string;
  link_benchmark?: LinkBenchmark;
  mesh_endpoint?: EnrollmentInvite["controller_mesh"];
  model_runtimes?: ModelRuntimeOffer[];
  workload_capabilities?: WorkloadCapability[];
}

export interface WorkloadCapability {
  schema: "rampage.workload-capability.v1";
  adapter: string;
  domain: "ai_inference" | "ai_evaluation" | "gaming" | "creative_production" | "software_build" | "scientific_computing" | "data_processing" | "storage" | "edge_utility";
  operations: string[];
  execution_patterns: Array<"whole_workload" | "independent_shard" | "replica" | "streaming_service" | "application_native_distributed" | "tensor_parallel" | "pipeline_parallel">;
  resource_classes: string[];
  isolation: "allowlisted_in_process" | "dedicated_process" | "container" | "wasm_sandbox" | "external_service" | "vendor_worker";
  runtime_digest: string;
  checkpointable: boolean;
  preemptible: boolean;
  network_allowlist_required: boolean;
  status: "shipped" | "qualified" | "candidate";
  qualification_digest?: string;
}

export interface WorkloadCapabilityInventory {
  schema: "rampage.workload-capability-inventory.v1";
  authority: "exact_adapter_operation_from_verified_signed_offer";
  candidate_authority: false;
  nodes: Array<{
    node_id: string;
    offer_id: string;
    observed_at: string;
    expires_at: string;
    signed_offer: boolean;
    capabilities: WorkloadCapability[];
  }>;
}

export interface FabricDiagnosticReport {
  schema: "rampage.fabric-diagnostic-report.v1";
  generated_at: string;
  status: "healthy" | "attention" | "degraded" | "stopped";
  health_score: number;
  evidence_digest: string;
  metrics: Record<string, unknown>;
  autonomy: {
    mode: "deterministic_thresholded_governor";
    per_change_approval_required: false;
    eligible_within_envelope: string[];
    authority_expansion: "automatically_denied_outside_owner_envelope";
    promotion_requirements: string[];
  };
  findings: Array<{
    severity: "info" | "warning" | "critical";
    code: string;
    scope: string;
    evidence: string;
    proposal: Record<string, unknown>;
  }>;
}

export interface RelayAccessManifest {
  schema: "rampage.relay-access-manifest.v1";
  fabric_id: string;
  generation: number;
  allowed_endpoint_ids: string[];
  issued_at: string;
  expires_at: string;
  signature: string;
}

export type PromotionRisk =
  | "r0_configuration"
  | "r1_allowlisted_source"
  | "r2_protected_change"
  | "r3_authority_critical";

export interface PromotionCandidate {
  schema: "rampage.promotion-candidate.v1";
  proposal_id: string;
  project_id: string;
  base_revision: string;
  candidate_digest: string;
  changed_paths: string[];
  risk: PromotionRisk;
  gates: Array<{
    name: string;
    passed: boolean;
    evidence_digest: string;
    independent: boolean;
  }>;
  requested_at: string;
  expires_at: string;
}

export interface PromotionCanaryLease {
  schema: "rampage.promotion-canary-lease.v1";
  canary_id: string;
  proposal_id: string;
  project_id: string;
  candidate_digest: string;
  risk: PromotionRisk;
  max_traffic_basis_points: number;
  max_error_regression_basis_points: number;
  max_latency_regression_basis_points: number;
  max_cost_regression_basis_points: number;
  issued_at: string;
  expires_at: string;
  nonce: string;
  fencing_epoch: number;
  signature: string;
}

export type ComputeStrategy =
  | "maximum_model_size"
  | "speed_boost"
  | "maximum_throughput"
  | "efficiency"
  | "autonomous_balanced";

export interface ModelRuntimeOffer {
  schema: "rampage.model-runtime-offer.v1";
  adapter: string;
  backend: "local_ollama" | "exo_mlx" | "vllm_ray";
  runtime_version: string;
  runtime_digest: string;
  compatibility_key: string;
  memory_kind: "dedicated_gpu" | "unified" | "host" | "hybrid";
  available_model_bytes: number;
  supported_parallelism: Array<"whole_model" | "pipeline" | "tensor" | "replica" | "speculative">;
  status: "shipped_local" | "candidate" | "qualified";
  installed_models?: InstalledModel[];
  certification_digest?: string;
}

export interface InstalledModel {
  schema: "rampage.installed-model.v1";
  model_id: string;
  artifact_digest: string;
  artifact_size_bytes: number;
}

export interface OpenAiModelList {
  object: "list";
  data: Array<{ id: string; object: "model"; created: number; owned_by: "rampage-fabric" }>;
}

export interface OpenAiChatCompletionRequest {
  model: string;
  messages: Array<{ role: "system" | "user" | "assistant"; content: string }>;
  stream?: false;
  max_tokens?: number;
  max_completion_tokens?: number;
  temperature?: number;
  top_p?: number;
}

export interface OpenAiChatCompletion {
  id: string;
  object: "chat.completion";
  created: number;
  model: string;
  choices: Array<{
    index: number;
    message: { role: "assistant"; content: string };
    finish_reason: string;
  }>;
  usage?: { prompt_tokens: number; completion_tokens: number; total_tokens: number };
}

export interface GatewayCapabilities {
  schema: "rampage.gateway-capabilities.v1";
  execution: {
    topology: "whole_model_one_contributor";
    cross_host_shared_memory: false;
    terminal_success_requires_signed_receipt: true;
  };
  protocols: Array<{
    id: "openai.chat_completions" | "anthropic.messages";
    paths: string[];
    streaming: string;
    content: string[];
  }>;
  unsupported: string[];
}

export interface AnthropicMessageRequest {
  model: string;
  max_tokens: number;
  messages: Array<{
    role: "user" | "assistant";
    content: string | Array<{ type: "text"; text: string }>;
  }>;
  system?: string | Array<{ type: "text"; text: string }>;
  stream?: false;
  temperature?: number;
  top_p?: number;
}

export interface AnthropicMessage {
  id: string;
  type: "message";
  role: "assistant";
  content: Array<{ type: "text"; text: string }>;
  model: string;
  stop_reason: "end_turn" | "max_tokens";
  stop_sequence: null;
  usage: { input_tokens: number; output_tokens: number };
}

export interface ModelSessionRequest {
  schema: "rampage.model-session-request.v1";
  session_id: string;
  model_id: string;
  estimated_weight_bytes: number;
  kv_cache_bytes: number;
  context_tokens: number;
  strategy: ComputeStrategy;
  max_nodes: number;
  deadline: string;
  idempotency_key: string;
}

export interface ModelSessionPlan {
  schema: "rampage.model-session-plan.v1";
  session_id: string;
  strategy: ComputeStrategy;
  state: "ready" | "qualification_required" | "capacity_blocked";
  backend?: "local_ollama" | "exo_mlx" | "vllm_ray";
  parallelism?: "whole_model" | "pipeline" | "tensor" | "replica" | "speculative";
  distributed: boolean;
  required_bytes: number;
  observed_fabric_bytes: number;
  maximum_supported_bytes: number;
  predicted_speedup_milli: number;
  placements: Array<{
    node_id: string;
    rank: number;
    assigned_bytes: number;
    available_model_bytes: number;
    role: string;
    topology_confidence: string;
  }>;
  blockers: string[];
  warnings: string[];
  proposed_local_endpoint: string | null;
  execution_authority: "none_preview_only";
  reason: string;
}

export interface ShardSet {
  schema: "rampage.shard-set.v1";
  set_id: string;
  project_id: string;
  submitted_by: string;
  shards: Record<string, unknown>[];
  minimum_successes: number;
  deadline: string;
  idempotency_key: string;
}

export interface ShardSetPlan {
  schema: "rampage.shard-set-plan.v1";
  set_id: string;
  admissible: boolean;
  all_or_nothing: true;
  placements: Array<{ job_id: string; node_id: string; score: Record<string, unknown> }>;
  blocked_job_id?: string;
  reason?: string;
  mutated: false;
}

export interface ShardSetAdmission {
  schema: "rampage.shard-set-admission.v1";
  set_id: string;
  minimum_successes: number;
  leases: Record<string, unknown>[];
  all_admitted: true;
  idempotent_replay: boolean;
}

export interface ShardSetStatus {
  schema: "rampage.shard-set-status.v1";
  set_id: string;
  status: "running" | "succeeded" | "failed";
  total: number;
  succeeded: number;
  failed: number;
  terminal: number;
  minimum_successes: number;
  threshold_met: boolean;
  threshold_still_possible: boolean;
  members: Array<{
    job_id: string;
    node_id: string | null;
    status: "admitted" | "running" | "succeeded" | "failed" | "ambiguous";
    receipt_id: string | null;
    result?: unknown;
  }>;
}

export class RampageClient {
  readonly baseUrl: string;
  readonly token?: string;

  constructor(baseUrl = "http://127.0.0.1:47831", token?: string) {
    const parsed = new URL(baseUrl);
    if (parsed.hostname !== "127.0.0.1" && parsed.hostname !== "localhost") {
      throw new Error("Rampage SDK only connects to the loopback controller API");
    }
    this.baseUrl = baseUrl.replace(/\/$/, "");
    this.token = token;
  }

  health(): Promise<RampageHealth> {
    return this.request("/health");
  }

  invite(): Promise<EnrollmentInvite> {
    return this.request("/v1/enrollment/invites", { method: "POST", body: "{}" });
  }

  nodes(): Promise<NodeIdentity[]> {
    return this.request("/v1/nodes");
  }

  revokeNode(nodeId: string): Promise<NodeRevocationReceipt> {
    const encodedNodeId = encodeURIComponent(nodeId);
    return this.request(
      `/v1/nodes/${encodedNodeId}/revoke`,
      this.json({ confirmation: `FORGET ${nodeId}` }),
    );
  }

  discover(path: string): Promise<unknown> {
    return this.request("/v1/projects/discover", this.json({ path }));
  }

  plan(job: unknown): Promise<unknown> {
    return this.request("/v1/jobs/plan", this.json(job));
  }

  topology(): Promise<ResourceOffer[]> {
    return this.request("/v1/offers");
  }

  workloadCapabilities(): Promise<WorkloadCapabilityInventory> {
    return this.request("/v1/workload-capabilities");
  }

  selfScan(): Promise<FabricDiagnosticReport> {
    return this.request("/v1/diagnostics/self-scan");
  }

  relayAccessManifest(): Promise<RelayAccessManifest> {
    return this.request("/v1/mesh/relay-access");
  }

  authorizePromotionCanary(candidate: PromotionCandidate): Promise<PromotionCanaryLease> {
    return this.request("/v1/improvements/canary", this.json(candidate));
  }

  planModelSession(request: ModelSessionRequest): Promise<ModelSessionPlan> {
    return this.request("/v1/model-sessions/plan", this.json(request));
  }

  openAiConfig(): { baseURL: string; apiKey: string } {
    if (!this.token) throw new Error("Rampage gateway requires the local controller token");
    return { baseURL: `${this.baseUrl}/v1`, apiKey: this.token };
  }

  openRouterConfig(): { baseURL: string; apiKey: string } {
    if (!this.token) throw new Error("Rampage gateway requires the local controller token");
    return { baseURL: `${this.baseUrl}/api/v1`, apiKey: this.token };
  }

  anthropicConfig(): { baseURL: string; apiKey: string } {
    if (!this.token) throw new Error("Rampage gateway requires the local controller token");
    return { baseURL: this.baseUrl, apiKey: this.token };
  }

  gatewayCapabilities(): Promise<GatewayCapabilities> {
    return this.gatewayRequest("/v1/capabilities");
  }

  models(): Promise<OpenAiModelList> {
    return this.gatewayRequest("/v1/models");
  }

  chatCompletion(request: OpenAiChatCompletionRequest): Promise<OpenAiChatCompletion> {
    return this.gatewayRequest("/v1/chat/completions", this.json(request));
  }

  anthropicMessage(request: AnthropicMessageRequest): Promise<AnthropicMessage> {
    return this.gatewayRequest("/v1/messages", this.json(request));
  }

  cancelModelSession(sessionId: string): Promise<{ session_id: string; cancelled: true }> {
    return this.gatewayRequest(
      `/v1/model-sessions/${encodeURIComponent(sessionId)}/cancel`,
      { method: "POST", body: "{}" },
    );
  }

  planShardSet(set: ShardSet): Promise<ShardSetPlan> {
    return this.request("/v1/shard-sets/plan", this.json(set));
  }

  runShardSet(set: ShardSet): Promise<ShardSetAdmission> {
    return this.request("/v1/shard-sets", this.json(set));
  }

  shardSetStatus(setId: string): Promise<ShardSetStatus> {
    return this.request(`/v1/shard-sets/${encodeURIComponent(setId)}`);
  }

  run(job: unknown): Promise<unknown> {
    return this.request("/v1/jobs", this.json(job));
  }

  receipts(jobId: string): Promise<ExecutionReceipt[]> {
    return this.request(`/v1/receipts?job_id=${encodeURIComponent(jobId)}`);
  }

  putArtifact(
    payload: Uint8Array,
    mediaType = "application/octet-stream",
    storageClass: StorageClass = "cache",
  ): Promise<ArtifactRef> {
    return this.request(
      "/v1/artifacts/put",
      this.json({
        data_base64: encodeBase64(payload),
        media_type: mediaType,
        storage_class: storageClass,
      }),
    );
  }

  async getArtifact(digest: string): Promise<Uint8Array> {
    const response = await this.request<{ data_base64: string }>(
      `/v1/artifacts/get?digest=${encodeURIComponent(digest)}`,
    );
    return decodeBase64(response.data_base64);
  }

  replicateArtifact(
    digest: string,
    nodeId: string,
    mediaType = "application/octet-stream",
    storageClass: StorageClass = "cache",
  ): Promise<ArtifactReplicationResult> {
    return this.request(
      "/v1/artifacts/replicate",
      this.json({
        digest,
        node_id: nodeId,
        media_type: mediaType,
        storage_class: storageClass,
      }),
    );
  }

  repairProtectedArtifacts(): Promise<ProtectedStorageReconciliation> {
    return this.request("/v1/artifacts/repair", {
      method: "POST",
      body: "{}",
    });
  }

  async retrieveArtifact(digest: string, nodeId: string): Promise<Uint8Array> {
    const response = await this.request<{ data_base64: string }>(
      "/v1/artifacts/retrieve",
      this.json({ digest, node_id: nodeId }),
    );
    return decodeBase64(response.data_base64);
  }

  async waitForReceipt(jobId: string, timeoutMs = 120_000): Promise<ExecutionReceipt> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const receipts = await this.receipts(jobId);
      if (receipts.length) return receipts.at(-1)!;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    throw new Error(`Rampage job ${jobId} did not finish before the deadline`);
  }

  events(after = 0): Promise<unknown[]> {
    return this.request(`/v1/events?after=${after}`);
  }

  stop(): Promise<unknown> {
    return this.request("/v1/stop", { method: "POST", body: "{}" });
  }

  resume(): Promise<unknown> {
    return this.request(
      "/v1/resume",
      this.json({ confirmation: "OWNER_RESUME" }),
    );
  }

  private json(body: unknown): RequestInit {
    return {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    };
  }

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const headers = new Headers(init?.headers);
    if (this.token) headers.set("x-rampage-token", this.token);
    const response = await fetch(`${this.baseUrl}${path}`, { ...init, headers });
    if (!response.ok) {
      throw new Error(`Rampage ${response.status}: ${await response.text()}`);
    }
    return (await response.json()) as T;
  }

  private async gatewayRequest<T>(path: string, init?: RequestInit): Promise<T> {
    if (!this.token) throw new Error("Rampage gateway requires the local controller token");
    const headers = new Headers(init?.headers);
    headers.set("authorization", `Bearer ${this.token}`);
    const response = await fetch(`${this.baseUrl}${path}`, { ...init, headers });
    if (!response.ok) {
      throw new Error(`Rampage gateway ${response.status}: ${await response.text()}`);
    }
    return (await response.json()) as T;
  }
}

function encodeBase64(payload: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < payload.length; offset += chunkSize) {
    binary += String.fromCharCode(...payload.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

function decodeBase64(encoded: string): Uint8Array {
  const binary = atob(encoded);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
