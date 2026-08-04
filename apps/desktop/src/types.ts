export type CapabilityState =
  | "full"
  | "local_reduced"
  | "deterministic_only"
  | "read_only"
  | "blocked";

export interface ResourceQuantity {
  class: string;
  capacity: number;
  available: number;
  unit: string;
  labels: Record<string, string>;
}

export interface ResourceOffer {
  offer_id: string;
  node_id: string;
  expires_at: string;
  resources: ResourceQuantity[];
  availability: {
    on_ac_power: boolean;
    battery_percent: number | null;
    thermal_headroom_percent: number;
    foreground_allowed: boolean;
    owner_idle: boolean;
  };
  adapters: string[];
  workload_capabilities?: WorkloadCapability[];
  model_runtimes?: Array<{
    adapter: string;
    backend: "local_ollama" | "exo_mlx" | "vllm_ray";
    available_model_bytes: number;
    status: "shipped_local" | "candidate" | "qualified";
    supported_parallelism: string[];
    installed_models?: Array<{
      model_id: string;
      artifact_digest: string;
      artifact_size_bytes: number;
    }>;
  }>;
  link_benchmark?: {
    rtt_micros_p50: number;
    uplink_bps: number;
    downlink_bps: number;
    transfer_bytes: number;
    samples: number;
    transport: "authenticated_quic";
    expires_at: string;
  };
  mesh_endpoint?: {
    endpoint_id: string;
    direct_addresses: string[];
    signature: string;
  };
}

export interface WorkloadCapability {
  schema: "rampage.workload-capability.v1";
  adapter: string;
  domain: "ai_inference" | "ai_evaluation" | "gaming" | "creative_production" | "software_build" | "scientific_computing" | "data_processing" | "storage" | "edge_utility";
  operations: string[];
  execution_patterns: string[];
  resource_classes: string[];
  isolation: "allowlisted_in_process" | "dedicated_process" | "container" | "wasm_sandbox" | "external_service" | "vendor_worker";
  runtime_digest: string;
  checkpointable: boolean;
  preemptible: boolean;
  network_allowlist_required: boolean;
  status: "shipped" | "qualified" | "candidate";
  qualification_digest?: string;
}

export interface FabricDiagnosticReport {
  schema: "rampage.fabric-diagnostic-report.v1";
  status: "healthy" | "attention" | "degraded" | "stopped";
  health_score: number;
  evidence_digest: string;
  autonomy: {
    mode: "deterministic_thresholded_governor";
    per_change_approval_required: false;
    authority_expansion: "automatically_denied_outside_owner_envelope";
  };
  findings: Array<{
    severity: "info" | "warning" | "critical";
    code: string;
    scope: string;
    evidence: string;
  }>;
}

export type ComputeStrategy =
  | "maximum_model_size"
  | "speed_boost"
  | "maximum_throughput"
  | "efficiency"
  | "autonomous_balanced";

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

export interface LedgerEvent {
  sequence: number;
  recorded_at: string;
  event_type: string;
  subject_id: string;
  payload: unknown;
  event_hash: string;
}

export interface ControllerHealth {
  status: "ready" | "stopped";
  kill_latch: boolean;
  mesh_mode: "local_only" | "private_relay";
  mesh_endpoint_id: string;
  mesh_sockets: string[];
}

export interface FabricNode {
  id: string;
  name: string;
  kind: string;
  state: "ready" | "working" | "sleeping" | "offline";
  cpu: number;
  memory: number;
  gpu: number;
  storage: number;
  storageAvailableGb: number;
  modelMemoryAvailableGb?: number;
  modelRuntimeCount?: number;
  artifactEndpoint: boolean;
  remoteAssist: boolean;
  latencyMs?: number;
  downlinkMbps?: number;
  uplinkMbps?: number;
  topologyConfidence?: "controller_local" | "measured" | "unmeasured";
  x: number;
  y: number;
  z: number;
}

export interface RemoteAssistStatus {
  supported: boolean;
  enabled: boolean;
  active: boolean;
  sessionId: string | null;
  mode: "view" | "control" | null;
  expiresAt: string | null;
}

export interface RemoteDesktopSession {
  schema: "rampage.remote-desktop-lease.v1";
  lease_id: string;
  session_id: string;
  node_id: string;
  mode: "view" | "control";
  max_width: number;
  max_height: number;
  max_fps: number;
  expires_at: string;
}

export interface RemoteDesktopFramePayload {
  schema: "rampage.remote-desktop-frame-payload.v1";
  session_id: string;
  frame: {
    sequence: number;
    captured_at: string;
    width: number;
    height: number;
    media_type: "image/jpeg";
  };
  data_base64: string;
}

export type RemoteInputEvent =
  | { kind: "mouse_move"; x: number; y: number }
  | { kind: "mouse_button"; button: "left" | "right" | "middle"; pressed: boolean }
  | { kind: "mouse_wheel"; delta: number }
  | { kind: "key"; virtual_key: number; pressed: boolean };
