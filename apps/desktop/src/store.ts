import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { CapabilityState, ComputeStrategy, ControllerHealth, FabricDiagnosticReport, FabricNode, LedgerEvent, ModelSessionPlan, RemoteAssistStatus, RemoteDesktopFramePayload, RemoteDesktopSession, RemoteInputEvent, ResourceOffer } from "./types";

const controller = import.meta.env.VITE_RAMPAGE_CONTROLLER ?? "http://127.0.0.1:47831";
const intelligence = import.meta.env.VITE_RAMPAGE_INTELLIGENCE ?? "http://127.0.0.1:47832";
let localControllerToken: string | null = null;
let remoteInputQueue: Promise<void> = Promise.resolve();
let ownerPairingListenerAttempted = false;

const computeStrategies: ComputeStrategy[] = [
  "maximum_model_size",
  "speed_boost",
  "maximum_throughput",
  "efficiency",
  "autonomous_balanced",
];

function storedComputeStrategy(): ComputeStrategy {
  const stored = localStorage.getItem("rampage.compute-strategy") as ComputeStrategy | null;
  return stored && computeStrategies.includes(stored) ? stored : "autonomous_balanced";
}

function controllerHeaders(json = false): HeadersInit {
  const headers: Record<string, string> = {};
  if (localControllerToken) headers["x-rampage-token"] = localControllerToken;
  if (json) headers["content-type"] = "application/json";
  return headers;
}

function controllerBearerHeaders(json = false): HeadersInit {
  const headers: Record<string, string> = {};
  if (localControllerToken) headers.authorization = `Bearer ${localControllerToken}`;
  if (json) headers["content-type"] = "application/json";
  return headers;
}

interface IntelligenceHealth {
  status: "ready";
  capability: CapabilityState;
  authority: "proposal_only";
}

export interface PairingRequest {
  request_id: string;
  device_name: string;
  device_kind: string;
  verification_code: string;
  expires_at_ms: number;
  state: "awaiting_approval" | "approved" | "completed";
}

export interface PairingWindow {
  schema: "rampage.pairing-window.v1";
  open: boolean;
  open_until_ms: number;
  requests: PairingRequest[];
}

export type WorkerPairing =
  | { state: "idle" }
  | { state: "searching"; request_id: string; expires_at_ms: number }
  | { state: "waiting_approval"; request_id: string; owner_name: string; verification_code: string; expires_at_ms: number }
  | { state: "approved"; request_id: string; owner_name: string }
  | { state: "failed"; message: string };

export interface WorkerRuntime {
  state: "inactive" | "starting" | "retrying" | "active" | "failed";
  nodeId: string | null;
  message: string | null;
}

export interface LocalAiRuntime {
  state: "detecting" | "installing" | "pulling_model" | "ready" | "failed" | "disabled";
  modelId: string;
  runtimeVersion: string | null;
  modelDigest: string | null;
  message: string;
}

export interface FabricBenchmarkResult {
  schema: "rampage.fabric-benchmark-result.v1";
  set_id: string;
  status: "succeeded";
  nodes: Array<{
    job_id: string;
    node_id: string;
    name: string;
    receipt_id: string;
    lanes: number;
    total_hashes: number;
    elapsed_ms: number;
    hashes_per_second: number;
    result_digest: string;
  }>;
  fabric_hashes_per_second: number;
  fastest_node_hashes_per_second: number;
  effective_scale_over_fastest_node: number;
  verified_extra_capacity_percent: number;
  estimated_time_saved_percent: number;
  time_returned_hours_per_100: number;
  proof_basis: "concurrent_signed_sustained_cpu_receipts";
  applicability: "matching_fully_divisible_cpu_work_only";
  all_results_signed: true;
}

export interface FabricDividendRecord {
  schema: "rampage.fabric-dividend-record.v1";
  ledger_sequence: number;
  recorded_at: string;
  result: FabricBenchmarkResult;
  previous_effective_scale?: number;
  scale_change_percent?: number;
}

export interface BreakEvenPlan {
  schema: "rampage.break-even-plan.v1";
  decision: "use_fabric" | "stay_on_fastest_node" | "insufficient_evidence";
  workload_class: "interactive_ai" | "batch_ai" | "build_test" | "render_transcode" | "artifact_movement";
  baseline_node_id: string | null;
  selected_node_ids: string[];
  p90_baseline_ms: number;
  p90_fabric_ms: number | null;
  estimated_gain_percent: number | null;
  required_gain_percent: number;
  evidence_set_id: string | null;
  evidence_age_seconds: number | null;
  topology_confidence: string;
  reason: string;
  claim_boundary: string;
}

export interface NetworkAutopilotStatus {
  schema: "rampage.network-autopilot-status.v1";
  generated_at: string;
  mode: "automatic_evidence_gated";
  nodes: Array<{
    node_id: string;
    preferred_path: "controller_local" | "direct_measured" | "owner_relay_measured" | "direct_candidate" | "owner_relay_bootstrap" | "recovering";
    evidence: string;
    direct_candidates: number;
    owner_relays: number;
    rtt_millis_p50: number | null;
    uplink_mbps: number | null;
    downlink_mbps: number | null;
    link_expires_at: string | null;
    traffic: Array<{
      traffic_class: "authority_control" | "interactive_ai" | "remote_media" | "artifact" | "bulk_background";
      admitted: boolean;
      reason: string;
    }>;
  }>;
  policy: string;
}

export interface RecoveryNode {
  nodeId: string;
  displayName: string;
  platform: string;
  deviceKind: string;
  live: boolean;
  local: boolean;
}

export interface RecoveryStatus {
  schema: "rampage.recovery-status.v1";
  version: string;
  role: "owner" | "worker" | "setup";
  state: string;
  healthy: boolean;
  issues: string[];
  canLeaveFabric: boolean;
  canFactoryReset: boolean;
  nodes: RecoveryNode[];
}

const initialLocalAiRuntime: LocalAiRuntime = {
  state: "detecting",
  modelId: "qwen3:4b",
  runtimeVersion: null,
  modelDigest: null,
  message: "Checking the automatic local AI runtime.",
};

const demoNodes: FabricNode[] = [
  { id: "home", name: "Command Rig", kind: "desktop", state: "ready", cpu: 31, memory: 46, gpu: 18, storage: 22, storageAvailableGb: 120, modelMemoryAvailableGb: 10, modelRuntimeCount: 1, artifactEndpoint: false, remoteAssist: false, latencyMs: 0, topologyConfidence: "controller_local", x: 0, y: 0, z: 0 },
  { id: "deck", name: "Steam Deck", kind: "steam_deck", state: "working", cpu: 64, memory: 53, gpu: 72, storage: 35, storageAvailableGb: 18, modelMemoryAvailableGb: 8, modelRuntimeCount: 0, artifactEndpoint: true, remoteAssist: false, latencyMs: 18.4, downlinkMbps: 386, uplinkMbps: 201, topologyConfidence: "measured", x: -3.2, y: -0.4, z: 1.8 },
  { id: "laptop", name: "Studio Laptop", kind: "laptop", state: "ready", cpu: 22, memory: 38, gpu: 12, storage: 14, storageAvailableGb: 42, modelMemoryAvailableGb: 11, modelRuntimeCount: 0, artifactEndpoint: true, remoteAssist: true, latencyMs: 7.1, downlinkMbps: 932, uplinkMbps: 908, topologyConfidence: "measured", x: 3.4, y: 0.3, z: 1.5 },
  { id: "phone", name: "Phone", kind: "phone", state: "sleeping", cpu: 0, memory: 0, gpu: 0, storage: 0, storageAvailableGb: 0, artifactEndpoint: false, remoteAssist: false, topologyConfidence: "unmeasured", x: 2.5, y: -0.8, z: -2.5 },
  { id: "nas", name: "Archive", kind: "storage", state: "ready", cpu: 9, memory: 16, gpu: 0, storage: 48, storageAvailableGb: 540, artifactEndpoint: true, remoteAssist: false, latencyMs: 2.2, downlinkMbps: 941, uplinkMbps: 936, topologyConfidence: "measured", x: -2.6, y: 0.6, z: -2.7 },
];
const initialNodes = import.meta.env.DEV ? demoNodes : [];
const demoDiagnostic: FabricDiagnosticReport = {
  schema: "rampage.fabric-diagnostic-report.v1",
  status: "healthy",
  health_score: 96,
  evidence_digest: `sha256:${"d".repeat(64)}`,
  autonomy: {
    mode: "deterministic_thresholded_governor",
    per_change_approval_required: false,
    authority_expansion: "automatically_denied_outside_owner_envelope",
  },
  findings: [{
    severity: "info",
    code: "IDLE_CAPACITY_AVAILABLE",
    scope: "fabric",
    evidence: "Fresh signed offers show useful idle capacity inside every owner reserve.",
  }],
};
const demoRecoveryStatus: RecoveryStatus = {
  schema: "rampage.recovery-status.v1",
  version: "0.3.1",
  role: "owner",
  state: "owner_active",
  healthy: true,
  issues: [],
  canLeaveFabric: false,
  canFactoryReset: true,
  nodes: [
    { nodeId: "0198f1aa-9f18-7dc3-81a3-d78f22efb660", displayName: "This Device", platform: "windows-x86_64", deviceKind: "desktop", live: true, local: true },
    { nodeId: "0198f1aa-9f18-7dc3-81a3-d78f22efb662", displayName: "Studio Laptop", platform: "windows-x86_64", deviceKind: "laptop", live: false, local: false },
  ],
};

interface RampageState {
  mode: "arena" | "grid";
  onboarding: boolean;
  connected: boolean;
  capability: CapabilityState;
  nodes: FabricNode[];
  events: LedgerEvent[];
  selectedNode: string;
  commandOpen: boolean;
  reducedMotion: boolean;
  lastSync: Date | null;
  meshMode: "local_only" | "private_relay";
  meshEndpointId: string | null;
  inviteCode: string | null;
  inviteBundle: string | null;
  fabricRole: "owner" | "worker" | "setup";
  lastAction: string | null;
  runAtLogin: boolean;
  killLatch: boolean;
  computeStrategy: ComputeStrategy;
  targetModelId: string;
  targetModelGiB: number;
  kvCacheGiB: number;
  modelPlan: ModelSessionPlan | null;
  modelPlanPending: boolean;
  gatewayModels: string[];
  diagnostic: FabricDiagnosticReport | null;
  pairingWindow: PairingWindow | null;
  workerPairing: WorkerPairing;
  workerRuntime: WorkerRuntime;
  localAiRuntime: LocalAiRuntime;
  fabricBenchmark: FabricBenchmarkResult | null;
  dividendHistory: FabricDividendRecord[];
  breakEvenPlan: BreakEvenPlan | null;
  networkAutopilot: NetworkAutopilotStatus | null;
  fabricBenchmarkPending: boolean;
  remoteAssistStatus: RemoteAssistStatus;
  remoteDesktopSession: RemoteDesktopSession | null;
  remoteDesktopFrame: RemoteDesktopFramePayload | null;
  remoteDesktopPending: boolean;
  remoteDesktopInputSequence: number;
  recoveryOpen: boolean;
  recoveryStatus: RecoveryStatus | null;
  setMode: (mode: "arena" | "grid") => void;
  setSelectedNode: (id: string) => void;
  setCommandOpen: (open: boolean) => void;
  setReducedMotion: (value: boolean) => void;
  setComputeStrategy: (strategy: ComputeStrategy) => void;
  setTargetModelId: (model: string) => void;
  setTargetModelGiB: (gib: number) => void;
  setKvCacheGiB: (gib: number) => void;
  planModelSession: () => Promise<void>;
  copyGatewayConfig: () => Promise<void>;
  finishOnboarding: () => Promise<void>;
  refresh: () => Promise<void>;
  createInvite: () => Promise<void>;
  joinFabric: (invitation: string) => Promise<void>;
  openPairingWindow: () => Promise<void>;
  refreshPairing: () => Promise<void>;
  beginPairing: () => Promise<void>;
  cancelPairing: () => Promise<void>;
  approvePairing: (requestId: string) => Promise<void>;
  rejectPairing: (requestId: string) => Promise<void>;
  runDemo: () => Promise<void>;
  runPoolProof: () => Promise<void>;
  runFabricBenchmark: () => Promise<void>;
  storeFile: (file: File, nodeId: string) => Promise<void>;
  localStop: () => void;
  localResume: () => Promise<void>;
  toggleAutostart: () => Promise<void>;
  refreshRemoteAssistStatus: () => Promise<void>;
  setRemoteAssistEnabled: (enabled: boolean) => Promise<void>;
  openRemoteDesktop: (nodeId: string, mode: "view" | "control") => Promise<void>;
  refreshRemoteDesktopFrame: () => Promise<void>;
  sendRemoteDesktopInput: (events: RemoteInputEvent[]) => Promise<void>;
  closeRemoteDesktop: () => Promise<void>;
  setRecoveryOpen: (open: boolean) => void;
  refreshRecovery: () => Promise<void>;
  repairConnection: () => Promise<void>;
  leaveFabric: (confirmation: string) => Promise<void>;
  factoryReset: (confirmation: string) => Promise<void>;
  forgetNode: (nodeId: string, confirmation: string) => Promise<void>;
}

function offersToNodes(offers: ResourceOffer[]): FabricNode[] {
  return offers.map((offer, index) => {
    const cpu = offer.resources.find((resource) => resource.class === "cpu_compute");
    const memory = offer.resources.find((resource) => resource.class === "ram_working_set");
    const gpu = offer.resources.find((resource) => resource.class === "gpu_compute");
    const gpuMemory = offer.resources.find((resource) => resource.class === "gpu_memory");
    const runtimeMemory = Math.max(0, ...(offer.model_runtimes ?? []).map((runtime) => runtime.available_model_bytes));
    const storageResources = offer.resources.filter((resource) => resource.class.startsWith("storage_") || resource.class === "protected_store");
    const storageCapacity = storageResources.reduce((total, resource) => total + resource.capacity, 0);
    const storageAvailable = storageResources.reduce((total, resource) => total + resource.available, 0);
    const pct = (resource?: ResourceOffer["resources"][number]) =>
      resource && resource.capacity > 0
        ? Math.round(100 - (resource.available / resource.capacity) * 100)
        : 0;
    const angle = (index / Math.max(offers.length, 1)) * Math.PI * 2;
    return {
      id: offer.node_id,
      name: cpu?.labels.device_name ?? `Node ${index + 1}`,
      kind: cpu?.labels.device_kind ?? "device",
      state: "ready" as const,
      cpu: pct(cpu),
      memory: pct(memory),
      gpu: pct(gpu),
      storage: storageCapacity > 0 ? Math.round(100 - (storageAvailable / storageCapacity) * 100) : 0,
      storageAvailableGb: Math.round((storageAvailable / (1024 ** 3)) * 10) / 10,
      modelMemoryAvailableGb: Math.round((runtimeMemory || gpuMemory?.available || memory?.available || 0) / (1024 ** 3) * 10) / 10,
      modelRuntimeCount: offer.model_runtimes?.length ?? 0,
      artifactEndpoint: Boolean(offer.mesh_endpoint?.signature),
      remoteAssist: (offer.workload_capabilities ?? []).some((capability) =>
        capability.adapter === "rampage.remote-assist.v1"
        && capability.status === "shipped"
        && capability.operations.includes("control")
      ),
      latencyMs: offer.mesh_endpoint ? Math.round((offer.link_benchmark?.rtt_micros_p50 ?? 0) / 100) / 10 : 0,
      downlinkMbps: offer.link_benchmark ? Math.round(offer.link_benchmark.downlink_bps / 100_000) / 10 : undefined,
      uplinkMbps: offer.link_benchmark ? Math.round(offer.link_benchmark.uplink_bps / 100_000) / 10 : undefined,
      topologyConfidence: offer.mesh_endpoint
        ? offer.link_benchmark ? "measured" : "unmeasured"
        : "controller_local",
      x: Math.cos(angle) * 3.4,
      y: index % 2 ? 0.4 : -0.35,
      z: Math.sin(angle) * 3.4,
    };
  });
}

export const useRampage = create<RampageState>((set, get) => ({
  mode: "arena",
  onboarding: localStorage.getItem("rampage.onboarded") !== "true",
  connected: false,
  capability: "deterministic_only",
  nodes: initialNodes,
  events: [],
  selectedNode: initialNodes[0]?.id ?? "",
  commandOpen: false,
  reducedMotion: window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false,
  lastSync: null,
  meshMode: "local_only",
  meshEndpointId: null,
  inviteCode: null,
  inviteBundle: null,
  fabricRole: "owner",
  lastAction: null,
  runAtLogin: false,
  killLatch: false,
  computeStrategy: storedComputeStrategy(),
  targetModelId: localStorage.getItem("rampage.target-model") ?? "local/target-model",
  targetModelGiB: Number(localStorage.getItem("rampage.target-model-gib") ?? 40),
  kvCacheGiB: Number(localStorage.getItem("rampage.kv-cache-gib") ?? 4),
  modelPlan: null,
  modelPlanPending: false,
  gatewayModels: [],
  diagnostic: import.meta.env.DEV ? demoDiagnostic : null,
  pairingWindow: null,
  workerPairing: { state: "idle" },
  workerRuntime: { state: "inactive", nodeId: null, message: null },
  localAiRuntime: initialLocalAiRuntime,
  fabricBenchmark: null,
  dividendHistory: [],
  breakEvenPlan: null,
  networkAutopilot: null,
  fabricBenchmarkPending: false,
  remoteAssistStatus: {
    supported: false,
    enabled: false,
    active: false,
    sessionId: null,
    mode: null,
    expiresAt: null,
  },
  remoteDesktopSession: null,
  remoteDesktopFrame: null,
  remoteDesktopPending: false,
  remoteDesktopInputSequence: 0,
  recoveryOpen: false,
  recoveryStatus: null,
  setMode: (mode) => set({ mode }),
  setSelectedNode: (selectedNode) => set({ selectedNode }),
  setCommandOpen: (commandOpen) => set({ commandOpen }),
  setReducedMotion: (reducedMotion) => set({ reducedMotion }),
  setComputeStrategy: (computeStrategy) => {
    localStorage.setItem("rampage.compute-strategy", computeStrategy);
    set({ computeStrategy });
    void get().planModelSession();
  },
  setTargetModelId: (targetModelId) => {
    localStorage.setItem("rampage.target-model", targetModelId);
    set({ targetModelId });
  },
  setTargetModelGiB: (targetModelGiB) => {
    const bounded = Math.max(1, Math.min(16_000, Number.isFinite(targetModelGiB) ? targetModelGiB : 1));
    localStorage.setItem("rampage.target-model-gib", String(bounded));
    set({ targetModelGiB: bounded });
  },
  setKvCacheGiB: (kvCacheGiB) => {
    const bounded = Math.max(0, Math.min(1_000, Number.isFinite(kvCacheGiB) ? kvCacheGiB : 0));
    localStorage.setItem("rampage.kv-cache-gib", String(bounded));
    set({ kvCacheGiB: bounded });
  },
  planModelSession: async () => {
    if (get().fabricRole !== "owner") return;
    set({ modelPlanPending: true });
    try {
      localControllerToken ??= await invoke<string>("controller_token").catch(() => null);
      const current = get();
      const gib = 1024 ** 3;
      const sessionId = crypto.randomUUID();
      const response = await fetch(`${controller}/v1/model-sessions/plan`, {
        method: "POST",
        headers: controllerHeaders(true),
        body: JSON.stringify({
          schema: "rampage.model-session-request.v1",
          session_id: sessionId,
          model_id: current.targetModelId.trim() || "local/target-model",
          estimated_weight_bytes: Math.round(current.targetModelGiB * gib),
          kv_cache_bytes: Math.round(current.kvCacheGiB * gib),
          context_tokens: 32_768,
          strategy: current.computeStrategy,
          max_nodes: 16,
          deadline: new Date(Date.now() + 10 * 60_000).toISOString(),
          idempotency_key: sessionId,
        }),
      });
      if (!response.ok) throw new Error(await response.text());
      const modelPlan = (await response.json()) as ModelSessionPlan;
      set({
        modelPlan,
        modelPlanPending: false,
        lastAction: modelPlan.state === "ready"
          ? `Model plan ready: ${modelPlan.placements.length} rank${modelPlan.placements.length === 1 ? "" : "s"}, ${modelPlan.parallelism?.replaceAll("_", " ")}.`
          : modelPlan.reason,
      });
    } catch (error) {
      set({
        modelPlanPending: false,
        lastAction: error instanceof Error ? `Model planning failed: ${error.message}` : "Model planning failed.",
      });
    }
  },
  copyGatewayConfig: async () => {
    localControllerToken ??= await invoke<string>("controller_token");
    await navigator.clipboard.writeText(
      `OPENAI_BASE_URL=${controller}/v1\nOPENAI_API_KEY=${localControllerToken}\nANTHROPIC_BASE_URL=${controller}\nANTHROPIC_API_KEY=${localControllerToken}\nRAMPAGE_OPENROUTER_BASE_URL=${controller}/api/v1`,
    );
    set({
      lastAction: "Universal AI gateway settings copied. Treat the shared local API key as a secret.",
    });
  },
  finishOnboarding: async () => {
    localStorage.setItem("rampage.onboarded", "true");
    try {
      await invoke("activate_owner_fabric");
      set({ onboarding: false, fabricRole: "owner" });
    } catch (error) {
      localStorage.removeItem("rampage.onboarded");
      throw error;
    }
  },
  refresh: async () => {
    try {
      const [fabricRole, runAtLogin, localAiRuntime, remoteAssistStatus] = await Promise.all([
        invoke<"owner" | "worker" | "setup">("fabric_mode").catch(() => "owner" as const),
        invoke<boolean>("autostart_enabled").catch(() => get().runAtLogin),
        invoke<LocalAiRuntime>("local_ai_runtime_status").catch(() => get().localAiRuntime),
        invoke<RemoteAssistStatus>("remote_assist_status").catch(() => get().remoteAssistStatus),
      ]);
      if (fabricRole === "setup") {
        localStorage.removeItem("rampage.onboarded");
        set({
          onboarding: true,
          fabricRole,
          connected: false,
          capability: "blocked",
          nodes: [],
          selectedNode: "",
          workerRuntime: { state: "inactive", nodeId: null, message: null },
          localAiRuntime,
          remoteAssistStatus,
          runAtLogin,
          killLatch: false,
          gatewayModels: [],
          diagnostic: null,
          fabricBenchmark: null,
          dividendHistory: [],
          breakEvenPlan: null,
          networkAutopilot: null,
          lastAction: "This device is clean and ready to create or join a fabric.",
          lastSync: new Date(),
        });
        return;
      }
      if (fabricRole === "worker") {
        const workerRuntime = await invoke<WorkerRuntime>("worker_runtime_status").catch(() => ({
          state: "failed" as const,
          nodeId: null,
          message: "Contributor runtime status is unavailable.",
        }));
        const active = workerRuntime.state === "active";
        localStorage.setItem("rampage.onboarded", "true");
        set({
          onboarding: false,
          fabricRole,
          workerRuntime,
          localAiRuntime,
          remoteAssistStatus,
          runAtLogin,
          killLatch: false,
          connected: active,
          capability: active ? "local_reduced" : "blocked",
          gatewayModels: [],
          diagnostic: null,
          fabricBenchmark: null,
          dividendHistory: [],
          breakEvenPlan: null,
          networkAutopilot: null,
          nodes: [{
            id: workerRuntime.nodeId ?? "worker",
            name: "This Worker",
            kind: "desktop",
            state: active ? "ready" : workerRuntime.state === "starting" || workerRuntime.state === "retrying" ? "sleeping" : "offline",
            cpu: 0,
            memory: 0,
            gpu: 0,
            storage: 0,
            storageAvailableGb: 0,
            modelMemoryAvailableGb: 0,
            modelRuntimeCount: localAiRuntime.state === "ready" ? 1 : 0,
            artifactEndpoint: false,
            remoteAssist: false,
            x: 0,
            y: 0,
            z: 0,
          }],
          selectedNode: "worker",
          lastAction: workerRuntime.message ?? "Contributor runtime is starting.",
          lastSync: new Date(),
        });
        return;
      }
      if (!ownerPairingListenerAttempted) {
        ownerPairingListenerAttempted = true;
        if (localStorage.getItem("rampage.onboarded") === "true") {
          await invoke("confirm_owner_fabric").catch(() => undefined);
        }
        void invoke<PairingWindow>("open_pairing_window")
          .then((pairingWindow) => set({ pairingWindow }))
          .catch(() => undefined);
      }
      localControllerToken ??= await invoke<string>("controller_token").catch(() => null);
      const [healthResponse, offersResponse, eventsResponse, modelsResponse, diagnosticResponse, intelligenceResponse, dividendsResponse, networkResponse] = await Promise.all([
        fetch(`${controller}/health`),
        fetch(`${controller}/v1/offers`, { headers: controllerHeaders() }),
        fetch(`${controller}/v1/events?latest=true&limit=120`, { headers: controllerHeaders() }),
        fetch(`${controller}/v1/models`, { headers: controllerBearerHeaders() }).catch(() => null),
        fetch(`${controller}/v1/diagnostics/self-scan`, { headers: controllerHeaders() }),
        fetch(`${intelligence}/health`).catch(() => null),
        fetch(`${controller}/v1/dividends?limit=24`, { headers: controllerHeaders() }).catch(() => null),
        fetch(`${controller}/v1/network/autopilot`, { headers: controllerHeaders() }).catch(() => null),
      ]);
      if (!healthResponse.ok || !offersResponse.ok || !eventsResponse.ok || !diagnosticResponse.ok) throw new Error("controller unavailable");
      const health = (await healthResponse.json()) as ControllerHealth;
      const offers = (await offersResponse.json()) as ResourceOffer[];
      const events = (await eventsResponse.json()) as LedgerEvent[];
      const diagnostic = (await diagnosticResponse.json()) as FabricDiagnosticReport;
      const gatewayModels = modelsResponse?.ok
        ? ((await modelsResponse.json()) as { data: Array<{ id: string }> }).data.map((model) => model.id)
        : [];
      const intelligenceHealth = intelligenceResponse?.ok
        ? ((await intelligenceResponse.json()) as IntelligenceHealth)
        : null;
      const dividendHistory = dividendsResponse?.ok
        ? (await dividendsResponse.json()) as FabricDividendRecord[]
        : [];
      const networkAutopilot = networkResponse?.ok
        ? (await networkResponse.json()) as NetworkAutopilotStatus
        : null;
      const latestDividend = dividendHistory.at(-1)?.result ?? null;
      let breakEvenPlan: BreakEvenPlan | null = null;
      if (latestDividend) {
        const fastest = latestDividend.nodes.reduce((best, node) =>
          node.hashes_per_second > best.hashes_per_second ? node : best,
        );
        const plannerResponse = await fetch(`${controller}/v1/plans/break-even`, {
          method: "POST",
          headers: controllerHeaders(true),
          body: JSON.stringify({
            schema: "rampage.break-even-request.v1",
            workload_class: "build_test",
            fastest_node_compute_ms: Math.max(1, Math.round(fastest.elapsed_ms)),
            input_bytes: 0,
            output_bytes: 0,
            startup_ms: 0,
            restart_tolerant: true,
            minimum_gain_percent: 12,
          }),
        }).catch(() => null);
        breakEvenPlan = plannerResponse?.ok
          ? (await plannerResponse.json()) as BreakEvenPlan
          : null;
      }
      set({
        connected: true,
        fabricRole,
        runAtLogin,
        localAiRuntime,
        remoteAssistStatus: {
          supported: false,
          enabled: false,
          active: false,
          sessionId: null,
          mode: null,
          expiresAt: null,
        },
        capability: health.kill_latch
          ? "read_only"
          : intelligenceHealth?.authority === "proposal_only"
            ? intelligenceHealth.capability
            : "deterministic_only",
        meshMode: health.mesh_mode,
        meshEndpointId: health.mesh_endpoint_id,
        nodes: offersToNodes(offers),
        selectedNode: offers.length
          ? (offers.some((offer) => offer.node_id === get().selectedNode)
              ? get().selectedNode
              : offers[0].node_id)
          : "",
        events,
        gatewayModels,
        diagnostic,
        fabricBenchmark: latestDividend ?? get().fabricBenchmark,
        dividendHistory,
        breakEvenPlan,
        networkAutopilot,
        killLatch: health.kill_latch,
        lastAction: (() => {
          const latest = events.at(-1);
          if (latest?.event_type === "artifact.replicated") return `Encrypted replica ${latest.subject_id.slice(0, 18)}… committed.`;
          if (latest?.event_type === "artifact.retrieved") return `Replica ${latest.subject_id.slice(0, 18)}… retrieved and verified.`;
          return get().lastAction;
        })(),
        lastSync: new Date(),
      });
      void get().planModelSession();
    } catch (error) {
      set({
        connected: false,
        capability: "deterministic_only",
        nodes: import.meta.env.DEV ? get().nodes : [],
        gatewayModels: [],
        diagnostic: import.meta.env.DEV ? demoDiagnostic : null,
        selectedNode: import.meta.env.DEV ? get().selectedNode : "",
        lastAction: import.meta.env.DEV
          ? "Showcase topology · production builds display only verified controller evidence."
          : error instanceof Error ? `Fabric unavailable: ${error.message}` : "Fabric unavailable.",
        lastSync: new Date(),
      });
    }
  },
  createInvite: async () => {
    localControllerToken ??= await invoke<string>("controller_token");
    const response = await fetch(`${controller}/v1/enrollment/invites`, {
      method: "POST",
      headers: controllerHeaders(),
    });
    if (!response.ok) throw new Error(`Invite creation failed: ${await response.text()}`);
    const invite = (await response.json()) as { enrollment_code: string };
    set({
      inviteCode: invite.enrollment_code,
      inviteBundle: JSON.stringify(invite),
      lastAction: "Signed one-time mesh invite created for 10 minutes.",
    });
  },
  joinFabric: async (invitation) => {
    const parsed = JSON.parse(invitation) as { schema?: string };
    if (parsed.schema !== "rampage.enrollment-invite.v1") throw new Error("This is not a Rampage invite.");
    await invoke("join_remote", { invitation });
  },
  openPairingWindow: async () => {
    const pairingWindow = await invoke<PairingWindow>("open_pairing_window");
    set({
      pairingWindow,
      lastAction: "Listening locally for a nearby Rampage laptop. No invite secret is being broadcast.",
    });
  },
  refreshPairing: async () => {
    try {
      // Owner admission is the critical path. Publish it as soon as the native command returns
      // instead of discarding a valid request when the unrelated worker-status read fails.
      const pairingWindow = await invoke<PairingWindow>("pairing_window");
      set({ pairingWindow });
      const workerPairing = await invoke<WorkerPairing>("pairing_status");
      set({ workerPairing });
    } catch {
      // Browser showcases and an app that is still starting have no native pairing bridge.
    }
  },
  beginPairing: async () => {
    const workerPairing = await invoke<WorkerPairing>("begin_pairing");
    set({
      workerPairing,
      lastAction: "Laptop waiting locally for its owner PC. Nothing needs to be copied.",
    });
  },
  cancelPairing: async () => {
    await invoke("cancel_pairing");
    set({ workerPairing: { state: "idle" }, lastAction: "Laptop pairing cancelled." });
  },
  approvePairing: async (requestId) => {
    await invoke<PairingRequest>("approve_pairing", { requestId });
    await get().refreshPairing();
    set({ lastAction: "Encrypted enrollment approved and delivered directly to the laptop." });
  },
  rejectPairing: async (requestId) => {
    await invoke("reject_pairing", { requestId });
    await get().refreshPairing();
    set({ lastAction: "Pairing request declined." });
  },
  runDemo: async () => {
    const now = Date.now();
    const job = {
      schema: "rampage.job-spec.v1",
      job_id: crypto.randomUUID(),
      project_id: crypto.randomUUID(),
      submitted_by: crypto.randomUUID(),
      adapter: "rampage.echo.v1",
      operation: "echo",
      arguments: { value: "Hello from the Rampage Arena" },
      inputs: [],
      requests: [{ class: "cpu_compute", minimum: 1, preferred: 1, unit: "logical_core", required_labels: {} }],
      trust: "native_trusted",
      restart_tolerant: true,
      network_allowlist: [],
      deadline: new Date(now + 10 * 60_000).toISOString(),
      idempotency_key: crypto.randomUUID(),
    };
    const response = await fetch(`${controller}/v1/jobs`, {
      method: "POST",
      headers: controllerHeaders(true),
      body: JSON.stringify(job),
    });
    if (!response.ok) throw new Error(await response.text());
    const lease = (await response.json()) as { node_id: string };
    set({ lastAction: `Governed demo assigned to ${lease.node_id.slice(0, 8)}…` });
    await get().refresh();
  },
  runPoolProof: async () => {
    localControllerToken ??= await invoke<string>("controller_token").catch(() => null);
    const setId = crypto.randomUUID();
    const projectId = crypto.randomUUID();
    const submittedBy = crypto.randomUUID();
    const deadline = new Date(Date.now() + 10 * 60_000).toISOString();
    const partitions = ["1,2,3", "4,5,6", "7,8,9"];
    const shardSet = {
      schema: "rampage.shard-set.v1",
      set_id: setId,
      project_id: projectId,
      submitted_by: submittedBy,
      minimum_successes: partitions.length,
      deadline,
      idempotency_key: `${setId}:set`,
      shards: partitions.map((values, index) => ({
        schema: "rampage.job-spec.v1",
        job_id: crypto.randomUUID(),
        project_id: projectId,
        submitted_by: submittedBy,
        adapter: "rampage.eval-shard.v1",
        operation: "score",
        arguments: { values },
        inputs: [],
        requests: [{ class: "cpu_compute", minimum: 1, preferred: 1, unit: "logical_core", required_labels: {} }],
        trust: "native_trusted",
        restart_tolerant: true,
        network_allowlist: [],
        deadline,
        idempotency_key: `${setId}:shard:${index}`,
      })),
    };
    const planResponse = await fetch(`${controller}/v1/shard-sets/plan`, {
      method: "POST",
      headers: controllerHeaders(true),
      body: JSON.stringify(shardSet),
    });
    if (!planResponse.ok) throw new Error(await planResponse.text());
    const plan = (await planResponse.json()) as { admissible: boolean; placements: Array<{ node_id: string }>; reason?: string };
    if (!plan.admissible) throw new Error(plan.reason ?? "The fabric cannot admit every shard.");
    const admissionResponse = await fetch(`${controller}/v1/shard-sets`, {
      method: "POST",
      headers: controllerHeaders(true),
      body: JSON.stringify(shardSet),
    });
    if (!admissionResponse.ok) throw new Error(await admissionResponse.text());
    const machines = new Set(plan.placements.map((placement) => placement.node_id)).size;
    set({ lastAction: `${partitions.length} proof shards admitted across ${machines} machine${machines === 1 ? "" : "s"}.` });
    const waitUntil = Date.now() + 60_000;
    while (Date.now() < waitUntil) {
      const statusResponse = await fetch(`${controller}/v1/shard-sets/${setId}`, { headers: controllerHeaders() });
      if (!statusResponse.ok) throw new Error(await statusResponse.text());
      const status = (await statusResponse.json()) as { status: "running" | "succeeded" | "failed"; succeeded: number; total: number };
      if (status.status === "succeeded") {
        set({ lastAction: `Pool proof complete: ${status.succeeded}/${status.total} signed shard receipts.` });
        await get().refresh();
        return;
      }
      if (status.status === "failed") throw new Error("Pool proof finished below its success threshold.");
      await new Promise((resolve) => window.setTimeout(resolve, 500));
    }
    set({ lastAction: `Pool proof is still running. Shard set ${setId.slice(0, 8)}… remains resumable.` });
  },
  runFabricBenchmark: async () => {
    if (get().fabricRole !== "owner" || get().fabricBenchmarkPending) return;
    set({ fabricBenchmarkPending: true, lastAction: "Running signed sustained work on every live machine…" });
    try {
      const fabricBenchmark = await invoke<FabricBenchmarkResult>("run_fabric_benchmark");
      const rate = (fabricBenchmark.fabric_hashes_per_second / 1_000_000).toFixed(2);
      await get().refresh();
      set({
        fabricBenchmark,
        lastAction: `Fabric proof complete: ${fabricBenchmark.nodes.length} signed node receipt${fabricBenchmark.nodes.length === 1 ? "" : "s"}, ${rate} MH/s combined.`,
      });
    } catch (error) {
      set({
        lastAction: error instanceof Error ? `Fabric benchmark failed: ${error.message}` : `Fabric benchmark failed: ${String(error)}`,
      });
    } finally {
      set({ fabricBenchmarkPending: false });
    }
  },
  storeFile: async (file, nodeId) => {
    if (file.size > 64 * 1024 * 1024) throw new Error("Artifact exceeds the 64 MiB transfer limit.");
    localControllerToken ??= await invoke<string>("controller_token");
    set({ lastAction: `Encrypting ${file.name} into the fabric…` });
    const payload = new Uint8Array(await file.arrayBuffer());
    let binary = "";
    for (let offset = 0; offset < payload.length; offset += 0x8000) {
      binary += String.fromCharCode(...payload.subarray(offset, offset + 0x8000));
    }
    const storedResponse = await fetch(`${controller}/v1/artifacts/put`, {
      method: "POST",
      headers: controllerHeaders(true),
      body: JSON.stringify({
        data_base64: btoa(binary),
        media_type: file.type || "application/octet-stream",
        storage_class: "cache",
      }),
    });
    if (!storedResponse.ok) throw new Error(await storedResponse.text());
    const stored = (await storedResponse.json()) as { digest: string };
    const selected = get().nodes.find((node) => node.id === nodeId);
    if (selected?.artifactEndpoint) {
      const replicaResponse = await fetch(`${controller}/v1/artifacts/replicate`, {
        method: "POST",
        headers: controllerHeaders(true),
        body: JSON.stringify({
          digest: stored.digest,
          node_id: nodeId,
          media_type: file.type || "application/octet-stream",
          storage_class: "cache",
        }),
      });
      if (!replicaResponse.ok) throw new Error(await replicaResponse.text());
      set({ lastAction: `${file.name} encrypted and replicated to ${selected.name}.` });
    } else {
      set({ lastAction: `${file.name} encrypted in the owner content store.` });
    }
    await get().refresh();
  },
  localStop: () => {
    set((state) => ({
      capability: "read_only",
      killLatch: true,
      nodes: state.nodes.map((node) => ({ ...node, state: "offline" })),
      remoteDesktopSession: null,
      remoteDesktopFrame: null,
    }));
    void invoke("local_stop").catch(() => undefined).finally(async () => {
      try {
        await fetch(`${controller}/v1/stop`, { method: "POST", headers: controllerHeaders() });
      } catch {
        // The Tauri latch is independent. The API call adds the durable remote fencing epoch when
        // the controller is reachable; browser preview still fails closed in visible local state.
      }
    });
  },
  localResume: async () => {
    localControllerToken ??= await invoke<string>("controller_token").catch(() => null);
    const response = await fetch(`${controller}/v1/resume`, {
      method: "POST",
      headers: controllerHeaders(true),
      body: JSON.stringify({ confirmation: "OWNER_RESUME" }),
    });
    if (!response.ok) throw new Error(`Resume failed: ${await response.text()}`);
    set({ killLatch: false, lastAction: "Owner-confirmed fabric resume accepted." });
    await get().refresh();
  },
  refreshRemoteAssistStatus: async () => {
    if (get().fabricRole !== "worker") return;
    try {
      const remoteAssistStatus = await invoke<RemoteAssistStatus>("remote_assist_status");
      set({ remoteAssistStatus });
    } catch {
      // Browser showcases do not have the native status bridge. The last exact native status stays
      // visible instead of inventing a transition or surfacing an unhandled polling failure.
    }
  },
  setRemoteAssistEnabled: async (enabled) => {
    try {
      const remoteAssistStatus = await invoke<RemoteAssistStatus>("set_remote_assist_enabled", { enabled });
      set({
        remoteAssistStatus,
        lastAction: enabled
          ? "Remote Assist enabled. Only your paired owner can request short, visible sessions."
          : "Remote Assist disabled. Active access was revoked immediately.",
      });
    } catch (error) {
      set({
        lastAction: error instanceof Error ? `Remote Assist failed: ${error.message}` : "Remote Assist could not be changed.",
      });
      throw error;
    }
  },
  openRemoteDesktop: async (nodeId, mode) => {
    if (get().remoteDesktopPending || get().killLatch) return;
    set({ remoteDesktopPending: true, remoteDesktopFrame: null });
    try {
      localControllerToken ??= await invoke<string>("controller_token");
      const response = await fetch(`${controller}/v1/remote-assist/sessions`, {
        method: "POST",
        headers: controllerHeaders(true),
        body: JSON.stringify({ node_id: nodeId, mode }),
      });
      if (!response.ok) throw new Error(await response.text());
      const payload = (await response.json()) as { session: RemoteDesktopSession };
      set({
        remoteDesktopSession: payload.session,
        remoteDesktopInputSequence: 0,
        lastAction: `${mode === "control" ? "Control" : "View"} session opened with a signed 30-second renewable lease.`,
      });
    } finally {
      set({ remoteDesktopPending: false });
    }
  },
  refreshRemoteDesktopFrame: async () => {
    const session = get().remoteDesktopSession;
    if (!session || get().remoteDesktopPending) return;
    set({ remoteDesktopPending: true });
    try {
      const response = await fetch(`${controller}/v1/remote-assist/sessions/${session.session_id}/frame`, {
        headers: controllerHeaders(),
        cache: "no-store",
      });
      if (!response.ok) throw new Error(await response.text());
      const remoteDesktopFrame = (await response.json()) as RemoteDesktopFramePayload;
      if (get().remoteDesktopSession?.session_id === session.session_id) {
        set({ remoteDesktopFrame });
      }
    } catch (error) {
      set({
        lastAction: error instanceof Error ? `Remote Assist frame failed: ${error.message}` : "Remote Assist frame failed.",
      });
    } finally {
      set({ remoteDesktopPending: false });
    }
  },
  sendRemoteDesktopInput: async (events) => {
    const send = async () => {
      const session = get().remoteDesktopSession;
      if (!session || session.mode !== "control" || !events.length || events.length > 64) return;
      const sequence = get().remoteDesktopInputSequence + 1;
      set({ remoteDesktopInputSequence: sequence });
      const response = await fetch(`${controller}/v1/remote-assist/sessions/${session.session_id}/input`, {
        method: "POST",
        headers: controllerHeaders(true),
        body: JSON.stringify({ sequence, events }),
      });
      if (!response.ok) throw new Error(await response.text());
    };
    const queued = remoteInputQueue.then(send, send);
    remoteInputQueue = queued.catch((error) => {
      set({
        lastAction: error instanceof Error ? `Remote input was refused: ${error.message}` : "Remote input was refused.",
      });
    });
    return remoteInputQueue;
  },
  closeRemoteDesktop: async () => {
    const session = get().remoteDesktopSession;
    set({ remoteDesktopSession: null, remoteDesktopFrame: null, remoteDesktopPending: false });
    if (!session) return;
    try {
      await fetch(`${controller}/v1/remote-assist/sessions/${session.session_id}/close`, {
        method: "POST",
        headers: controllerHeaders(),
      });
      set({ lastAction: "Remote Assist closed and the worker activity indicator cleared." });
    } catch {
      set({ lastAction: "Remote Assist viewer closed. The worker lease will expire within 30 seconds." });
    }
  },
  setRecoveryOpen: (recoveryOpen) => {
    set({ recoveryOpen });
    if (recoveryOpen) void get().refreshRecovery();
  },
  refreshRecovery: async () => {
    let native: Omit<RecoveryStatus, "nodes">;
    try {
      native = await invoke<Omit<RecoveryStatus, "nodes">>("recovery_status");
    } catch (error) {
      if (import.meta.env.DEV) {
        set({ recoveryStatus: demoRecoveryStatus });
        return;
      }
      const role = get().fabricRole;
      set({
        recoveryStatus: {
          schema: "rampage.recovery-status.v1",
          version: "unavailable",
          role,
          state: "status_unavailable",
          healthy: false,
          issues: [error instanceof Error ? error.message : "The native recovery bridge did not respond."],
          canLeaveFabric: role === "worker",
          canFactoryReset: true,
          nodes: [],
        },
      });
      return;
    }
    let nodes: RecoveryNode[] = [];
    if (native.role === "owner") {
      try {
        localControllerToken ??= await invoke<string>("controller_token");
        const [nodesResponse, offersResponse] = await Promise.all([
          fetch(`${controller}/v1/nodes`, { headers: controllerHeaders() }),
          fetch(`${controller}/v1/offers`, { headers: controllerHeaders() }),
        ]);
        if (nodesResponse.ok && offersResponse.ok) {
          const enrolled = await nodesResponse.json() as Array<{
            node_id: string;
            display_name: string;
            platform: string;
            device_kind: string;
          }>;
          const offers = await offersResponse.json() as ResourceOffer[];
          const liveIds = new Set(offers.map((offer) => offer.node_id));
          nodes = enrolled.map((node) => ({
            nodeId: node.node_id,
            displayName: node.display_name,
            platform: node.platform,
            deviceKind: node.device_kind,
            live: liveIds.has(node.node_id),
            local: node.display_name === "This Device",
          }));
        }
      } catch {
        // Native recovery remains available even if the owner controller is unhealthy.
      }
    }
    set({ recoveryStatus: { ...native, nodes } });
  },
  repairConnection: async () => {
    set({ lastAction: "Restarting Rampage and rebuilding the local connection path…" });
    await invoke("repair_connection");
  },
  leaveFabric: async (confirmation) => {
    const prior = localStorage.getItem("rampage.onboarded");
    localStorage.removeItem("rampage.onboarded");
    try {
      await invoke("leave_fabric", { confirmation });
    } catch (error) {
      if (prior !== null) localStorage.setItem("rampage.onboarded", prior);
      throw error;
    }
  },
  factoryReset: async (confirmation) => {
    const keys = ["rampage.onboarded", "rampage.compute-strategy", "rampage.target-model", "rampage.target-model-gib", "rampage.kv-cache-gib"];
    const prior = new Map(keys.map((key) => [key, localStorage.getItem(key)]));
    keys.forEach((key) => localStorage.removeItem(key));
    try {
      await invoke("factory_reset", { confirmation });
    } catch (error) {
      prior.forEach((value, key) => { if (value !== null) localStorage.setItem(key, value); });
      throw error;
    }
  },
  forgetNode: async (nodeId, confirmation) => {
    localControllerToken ??= await invoke<string>("controller_token");
    const response = await fetch(`${controller}/v1/nodes/${nodeId}/revoke`, {
      method: "POST",
      headers: controllerHeaders(true),
      body: JSON.stringify({ confirmation }),
    });
    if (!response.ok) throw new Error(await response.text());
    set({ lastAction: "Machine forgotten. Its offers, leases, sessions, and future access were revoked." });
    await Promise.all([get().refreshRecovery(), get().refresh()]);
  },
  toggleAutostart: async () => {
    const requested = !get().runAtLogin;
    try {
      const runAtLogin = await invoke<boolean>("set_autostart", { enabled: requested });
      set({
        runAtLogin,
        lastAction: runAtLogin
          ? "Rampage will start quietly in the system tray after Windows sign-in."
          : "Rampage will no longer start automatically.",
      });
    } catch {
      set({ lastAction: "Auto-start is available in the installed Rampage desktop app." });
    }
  },
}));

export function surfaceNativePairingRequest(request: PairingRequest | null) {
  if (!request) {
    void useRampage.getState().refreshPairing();
    return;
  }
  useRampage.setState((state) => {
    const requests = state.pairingWindow?.requests.filter(
      (candidate) => candidate.request_id !== request.request_id,
    ) ?? [];
    requests.push(request);
    return {
      pairingWindow: {
        schema: "rampage.pairing-window.v1",
        open: true,
        open_until_ms: Math.max(
          state.pairingWindow?.open_until_ms ?? 0,
          request.expires_at_ms,
        ),
        requests,
      },
      lastAction: `${request.device_name} found nearby. Approve it once to add it to this fabric.`,
    };
  });
}
