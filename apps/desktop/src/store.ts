import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type { CapabilityState, ControllerHealth, FabricNode, LedgerEvent, ResourceOffer } from "./types";

const controller = import.meta.env.VITE_RAMPAGE_CONTROLLER ?? "http://127.0.0.1:47831";
const intelligence = import.meta.env.VITE_RAMPAGE_INTELLIGENCE ?? "http://127.0.0.1:47832";
let localControllerToken: string | null = null;

function controllerHeaders(json = false): HeadersInit {
  const headers: Record<string, string> = {};
  if (localControllerToken) headers["x-rampage-token"] = localControllerToken;
  if (json) headers["content-type"] = "application/json";
  return headers;
}

interface IntelligenceHealth {
  status: "ready";
  capability: CapabilityState;
  authority: "proposal_only";
}

const demoNodes: FabricNode[] = [
  { id: "home", name: "Command Rig", kind: "desktop", state: "ready", cpu: 31, memory: 46, gpu: 18, storage: 22, storageAvailableGb: 120, artifactEndpoint: false, latencyMs: 0, topologyConfidence: "controller_local", x: 0, y: 0, z: 0 },
  { id: "deck", name: "Steam Deck", kind: "steam_deck", state: "working", cpu: 64, memory: 53, gpu: 72, storage: 35, storageAvailableGb: 18, artifactEndpoint: true, latencyMs: 18.4, downlinkMbps: 386, uplinkMbps: 201, topologyConfidence: "measured", x: -3.2, y: -0.4, z: 1.8 },
  { id: "laptop", name: "Studio Laptop", kind: "laptop", state: "ready", cpu: 22, memory: 38, gpu: 12, storage: 14, storageAvailableGb: 42, artifactEndpoint: true, latencyMs: 7.1, downlinkMbps: 932, uplinkMbps: 908, topologyConfidence: "measured", x: 3.4, y: 0.3, z: 1.5 },
  { id: "phone", name: "Phone", kind: "phone", state: "sleeping", cpu: 0, memory: 0, gpu: 0, storage: 0, storageAvailableGb: 0, artifactEndpoint: false, topologyConfidence: "unmeasured", x: 2.5, y: -0.8, z: -2.5 },
  { id: "nas", name: "Archive", kind: "storage", state: "ready", cpu: 9, memory: 16, gpu: 0, storage: 48, storageAvailableGb: 540, artifactEndpoint: true, latencyMs: 2.2, downlinkMbps: 941, uplinkMbps: 936, topologyConfidence: "measured", x: -2.6, y: 0.6, z: -2.7 },
];

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
  fabricRole: "owner" | "worker";
  lastAction: string | null;
  setMode: (mode: "arena" | "grid") => void;
  setSelectedNode: (id: string) => void;
  setCommandOpen: (open: boolean) => void;
  setReducedMotion: (value: boolean) => void;
  finishOnboarding: () => void;
  refresh: () => Promise<void>;
  createInvite: () => Promise<void>;
  joinFabric: (invitation: string) => Promise<void>;
  runDemo: () => Promise<void>;
  runPoolProof: () => Promise<void>;
  storeFile: (file: File, nodeId: string) => Promise<void>;
  localStop: () => void;
}

function offersToNodes(offers: ResourceOffer[]): FabricNode[] {
  return offers.map((offer, index) => {
    const cpu = offer.resources.find((resource) => resource.class === "cpu_compute");
    const memory = offer.resources.find((resource) => resource.class === "ram_working_set");
    const gpu = offer.resources.find((resource) => resource.class === "gpu_compute");
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
      artifactEndpoint: Boolean(offer.mesh_endpoint?.signature),
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
  nodes: demoNodes,
  events: [],
  selectedNode: "home",
  commandOpen: false,
  reducedMotion: window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false,
  lastSync: null,
  meshMode: "local_only",
  meshEndpointId: null,
  inviteCode: null,
  inviteBundle: null,
  fabricRole: "owner",
  lastAction: null,
  setMode: (mode) => set({ mode }),
  setSelectedNode: (selectedNode) => set({ selectedNode }),
  setCommandOpen: (commandOpen) => set({ commandOpen }),
  setReducedMotion: (reducedMotion) => set({ reducedMotion }),
  finishOnboarding: () => {
    localStorage.setItem("rampage.onboarded", "true");
    set({ onboarding: false });
  },
  refresh: async () => {
    try {
      const fabricRole = await invoke<"owner" | "worker">("fabric_mode").catch(() => "owner" as const);
      if (fabricRole === "worker") {
        set({
          fabricRole,
          connected: true,
          capability: "local_reduced",
          nodes: [{
            id: "worker",
            name: "This Worker",
            kind: "desktop",
            state: "ready",
            cpu: 0,
            memory: 0,
            gpu: 0,
            storage: 0,
            storageAvailableGb: 0,
            artifactEndpoint: false,
            x: 0,
            y: 0,
            z: 0,
          }],
          selectedNode: "worker",
          lastAction: "Contributing through a signed, owner-controlled mesh session.",
          lastSync: new Date(),
        });
        return;
      }
      localControllerToken ??= await invoke<string>("controller_token").catch(() => null);
      const [healthResponse, offersResponse, eventsResponse, intelligenceResponse] = await Promise.all([
        fetch(`${controller}/health`),
        fetch(`${controller}/v1/offers`, { headers: controllerHeaders() }),
        fetch(`${controller}/v1/events?after=0&limit=120`, { headers: controllerHeaders() }),
        fetch(`${intelligence}/health`).catch(() => null),
      ]);
      if (!healthResponse.ok || !offersResponse.ok || !eventsResponse.ok) throw new Error("controller unavailable");
      const health = (await healthResponse.json()) as ControllerHealth;
      const offers = (await offersResponse.json()) as ResourceOffer[];
      const events = (await eventsResponse.json()) as LedgerEvent[];
      const intelligenceHealth = intelligenceResponse?.ok
        ? ((await intelligenceResponse.json()) as IntelligenceHealth)
        : null;
      set({
        connected: true,
        fabricRole,
        capability: health.kill_latch
          ? "read_only"
          : intelligenceHealth?.authority === "proposal_only"
            ? intelligenceHealth.capability
            : "deterministic_only",
        meshMode: health.mesh_mode,
        meshEndpointId: health.mesh_endpoint_id,
        nodes: offers.length ? offersToNodes(offers) : get().nodes,
        events,
        lastAction: (() => {
          const latest = events.at(-1);
          if (latest?.event_type === "artifact.replicated") return `Encrypted replica ${latest.subject_id.slice(0, 18)}… committed.`;
          if (latest?.event_type === "artifact.retrieved") return `Replica ${latest.subject_id.slice(0, 18)}… retrieved and verified.`;
          return get().lastAction;
        })(),
        lastSync: new Date(),
      });
    } catch {
      set({ connected: false, capability: "deterministic_only", lastSync: new Date() });
    }
  },
  createInvite: async () => {
    localControllerToken ??= await invoke<string>("controller_token");
    const response = await fetch(`${controller}/v1/enrollment/invites`, {
      method: "POST",
      headers: controllerHeaders(),
    });
    if (!response.ok) throw new Error("invite creation failed");
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
      nodes: state.nodes.map((node) => ({ ...node, state: "offline" })),
    }));
    void invoke("local_stop").catch(async () => {
      try {
        await fetch(`${controller}/v1/stop`, { method: "POST", headers: controllerHeaders() });
      } catch {
        // Browser preview has no Tauri IPC. The visible local state still fails closed.
      }
    });
  },
}));
