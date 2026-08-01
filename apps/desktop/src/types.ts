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
  model_runtimes?: Array<{
    adapter: string;
    backend: "local_ollama" | "exo_mlx" | "vllm_ray";
    available_model_bytes: number;
    status: "shipped_local" | "candidate" | "qualified";
    supported_parallelism: string[];
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
  latencyMs?: number;
  downlinkMbps?: number;
  uplinkMbps?: number;
  topologyConfidence?: "controller_local" | "measured" | "unmeasured";
  x: number;
  y: number;
  z: number;
}
