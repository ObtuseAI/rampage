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
  artifactEndpoint: boolean;
  latencyMs?: number;
  downlinkMbps?: number;
  uplinkMbps?: number;
  topologyConfidence?: "controller_local" | "measured" | "unmeasured";
  x: number;
  y: number;
  z: number;
}
