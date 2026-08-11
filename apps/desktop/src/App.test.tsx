import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import { type PairingRequest, useRampage } from "./store";

vi.mock("./components/Arena", () => ({ Arena: () => <div>Spatial fabric</div> }));
const nativeInvoke = vi.hoisted(() => vi.fn(async (command: string) => {
  if (command === "control_window") return undefined;
  throw new Error("native backend unavailable in browser test");
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: nativeInvoke }));
const nativeEvent = vi.hoisted(() => ({
  pairingHandler: null as null | ((event: { payload: PairingRequest | null }) => void),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_event: string, handler: (event: { payload: PairingRequest | null }) => void) => {
    nativeEvent.pairingHandler = handler;
    return () => { nativeEvent.pairingHandler = null; };
  }),
}));
const baselineNodes = useRampage.getState().nodes.map((node) => ({ ...node }));

afterEach(cleanup);

beforeEach(() => {
  nativeInvoke.mockClear();
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {},
  });
  localStorage.setItem("rampage.onboarded", "true");
  localStorage.setItem("rampage.compute-strategy", "maximum_model_size");
  useRampage.setState({
    onboarding: false,
    mode: "arena",
    commandOpen: false,
    computeStrategy: "maximum_model_size",
    modelPlan: null,
    modelPlanPending: false,
    gatewayModels: [],
    runAtLogin: false,
    localAiRuntime: {
      state: "detecting",
      modelId: "qwen3:4b",
      runtimeVersion: null,
      modelDigest: null,
      message: "Checking the automatic local AI runtime.",
    },
    fabricBenchmark: null,
    dividendHistory: [],
    breakEvenPlan: null,
    networkAutopilot: null,
    fabricBenchmarkPending: false,
    fabricRole: "owner",
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
    killLatch: false,
    connected: true,
    pairingWindow: null,
    nodes: baselineNodes,
    selectedNode: baselineNodes[0]?.id ?? "",
  });
  globalThis.fetch = vi.fn().mockRejectedValue(new Error("offline"));
});

test("surfaces a native nearby request immediately without waiting for polling", async () => {
  render(<App />);
  await waitFor(() => expect(nativeEvent.pairingHandler).not.toBeNull());
  act(() => nativeEvent.pairingHandler?.({
    payload: {
      request_id: "ab".repeat(16),
      device_name: "Studio Laptop",
      device_kind: "desktop",
      verification_code: "4721",
      expires_at_ms: Date.now() + 60_000,
      state: "awaiting_approval",
    },
  }));

  expect(screen.getByText("NEW MACHINE FOUND")).toBeVisible();
  expect(screen.getByRole("button", { name: /approve this machine/i })).toBeEnabled();
  expect(screen.queryByText("4721")).not.toBeInTheDocument();
});

test("provides accessible grid parity", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Grid" }));
  expect(screen.getByRole("button", { name: /command rig/i })).toBeInTheDocument();
  expect(screen.getByLabelText("Fabric nodes")).toBeInTheDocument();
  expect(screen.getByLabelText("Encrypt file locally")).toBeInTheDocument();
});

test("every primary destination opens a functional product surface", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Work" }));
  expect(screen.getByRole("heading", { name: /put the whole fabric to work/i })).toBeVisible();
  expect(screen.getByRole("button", { name: /run sustained benchmark/i })).toBeEnabled();

  fireEvent.click(screen.getByRole("button", { name: "Evolution" }));
  expect(screen.getByRole("heading", { name: /fabric that finds its own limits/i })).toBeVisible();
  expect(screen.getByRole("button", { name: /scan now/i })).toBeEnabled();

  fireEvent.click(screen.getByRole("button", { name: "Evidence" }));
  expect(screen.getByRole("heading", { name: /every useful action leaves a receipt/i })).toBeVisible();
  expect(screen.getByRole("button", { name: /refresh evidence/i })).toBeEnabled();

  fireEvent.click(screen.getByRole("button", { name: "Fabric" }));
  expect(screen.getByRole("heading", { name: /your machines, acting as one/i })).toBeVisible();
});

test("routes every borderless window control through the native backend", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Minimize Rampage" }));
  fireEvent.click(screen.getByRole("button", { name: "Maximize Rampage" }));
  fireEvent.click(screen.getByRole("button", { name: "Close Rampage" }));
  expect(nativeInvoke).toHaveBeenCalledWith("control_window", { action: "minimize" });
  expect(nativeInvoke).toHaveBeenCalledWith("control_window", { action: "maximize" });
  expect(nativeInvoke).toHaveBeenCalledWith("control_window", { action: "close" });
});

test("local stop does not depend on controller", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "STOP" }));
  expect(useRampage.getState().capability).toBe("read_only");
});

test("separates the biggest AI outcome from the fastest AI outcome", () => {
  render(<App />);
  expect(screen.getByRole("button", { name: /biggest ai/i })).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(screen.getByRole("button", { name: /fastest ai/i }));
  expect(useRampage.getState().computeStrategy).toBe("speed_boost");
  expect(screen.getByText(/use tensor peers only when measured links predict faster tokens/i)).toBeInTheDocument();
  expect(localStorage.getItem("rampage.compute-strategy")).toBe("speed_boost");
});

test("exposes installed desktop lifecycle control", () => {
  render(<App />);
  expect(screen.getByRole("button", { name: "Start Rampage with Windows" })).toBeInTheDocument();
});

test("shows when the universal whole-model gateway is ready", () => {
  useRampage.setState({ gatewayModels: ["gemma3:4b"] });
  render(<App />);
  expect(screen.getByText(/1 consistent installed model · OpenAI · Anthropic · OpenRouter ready/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Copy API setup" })).toBeEnabled();
});

test("shows automatic local AI qualification and the sustained fabric proof", () => {
  useRampage.setState({
    connected: true,
    localAiRuntime: {
      state: "ready",
      modelId: "qwen3:4b",
      runtimeVersion: "0.32.5",
      modelDigest: `sha256:${"a".repeat(64)}`,
      message: "qwen3:4b is installed and qualified for signed whole-model work.",
    },
  });
  render(<App />);
  expect(screen.getByText(/local ai autopilot · ready/i)).toBeInTheDocument();
  expect(screen.getByText(/qwen3:4b is installed and qualified/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Prove the fabric" })).toBeEnabled();
});

test("turns signed benchmark receipts into a bounded compute dividend", () => {
  useRampage.setState({
    fabricBenchmark: {
      schema: "rampage.fabric-benchmark-result.v1",
      set_id: "set-1",
      status: "succeeded",
      nodes: [
        { job_id: "job-1", node_id: "main", name: "Main", receipt_id: "receipt-1", lanes: 4, total_hashes: 2_000_000, elapsed_ms: 50, hashes_per_second: 60_000_000, result_digest: `sha256:${"a".repeat(64)}` },
        { job_id: "job-2", node_id: "laptop", name: "Laptop", receipt_id: "receipt-2", lanes: 4, total_hashes: 2_000_000, elapsed_ms: 75, hashes_per_second: 40_000_000, result_digest: `sha256:${"b".repeat(64)}` },
      ],
      fabric_hashes_per_second: 100_000_000,
      fastest_node_hashes_per_second: 60_000_000,
      effective_scale_over_fastest_node: 1.666666,
      verified_extra_capacity_percent: 66.6666,
      estimated_time_saved_percent: 40,
      time_returned_hours_per_100: 40,
      proof_basis: "concurrent_signed_sustained_cpu_receipts",
      applicability: "matching_fully_divisible_cpu_work_only",
      all_results_signed: true,
    },
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Work" }));
  expect(screen.getByRole("heading", { name: "40.0 hours returned per 100" })).toBeVisible();
  expect(screen.getByText(/not a claim that every workload or the PC itself becomes this much faster/i)).toBeVisible();
  expect(screen.getByText(/\+66.7% verified capacity/i)).toBeVisible();
});

test("shows fail-closed break-even and network autopilot decisions", () => {
  useRampage.setState({
    breakEvenPlan: {
      schema: "rampage.break-even-plan.v1",
      decision: "stay_on_fastest_node",
      workload_class: "build_test",
      baseline_node_id: "main",
      selected_node_ids: ["main"],
      p90_baseline_ms: 60_000,
      p90_fabric_ms: 62_000,
      estimated_gain_percent: -3.3,
      required_gain_percent: 12,
      evidence_set_id: "set-1",
      evidence_age_seconds: 30,
      topology_confidence: "fresh_signed_compute_and_link_evidence",
      reason: "Distribution projects -3.3% gain, below the 12.0% safety threshold; stay on the fastest node.",
      claim_boundary: "projection_for_matching_divisible_cpu_work_not_a_general_speed_guarantee",
    },
    networkAutopilot: {
      schema: "rampage.network-autopilot-status.v1",
      generated_at: new Date().toISOString(),
      mode: "automatic_evidence_gated",
      policy: "authority_first_then_measured_interactive_and_bulk_admission",
      nodes: [{
        node_id: "laptop",
        preferred_path: "owner_relay_bootstrap",
        evidence: "owner-operated relay retained while direct-path evidence is unavailable",
        direct_candidates: 1,
        owner_relays: 1,
        rtt_millis_p50: null,
        uplink_mbps: null,
        downlink_mbps: null,
        link_expires_at: null,
        traffic: [{ traffic_class: "authority_control", admitted: true, reason: "authenticated" }],
      }],
    },
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Work" }));
  expect(screen.getByRole("heading", { name: "Fastest node wins" })).toBeVisible();
  expect(screen.getByText(/slower or unmeasured distributed plans are refused automatically/i)).toBeVisible();
  expect(screen.getByText(/0 direct · 1 relay/i)).toBeVisible();
  expect(screen.getByText(/laptop · owner relay bootstrap/i)).toBeVisible();
});

test("exposes remote control only for an explicitly capable paired worker", () => {
  const laptop = useRampage.getState().nodes.find((node) => node.id === "laptop")!;
  useRampage.setState({
    fabricRole: "owner",
    nodes: [{ ...laptop, remoteAssist: true }],
    selectedNode: laptop.id,
  });
  render(<App />);
  expect(screen.getByRole("button", { name: /view desktop/i })).toBeEnabled();
  expect(screen.getByRole("button", { name: /control desktop/i })).toBeEnabled();
});

test("worker surface makes Remote Assist opt-in and active control visible", () => {
  useRampage.setState({
    fabricRole: "worker",
    remoteAssistStatus: {
      supported: true,
      enabled: true,
      active: true,
      sessionId: "session",
      mode: "control",
      expiresAt: new Date(Date.now() + 30_000).toISOString(),
    },
  });
  render(<App />);
  expect(screen.getByRole("checkbox", { name: /allow owner remote control/i })).toBeChecked();
  expect(screen.getAllByText(/remote control active/i).length).toBeGreaterThan(0);
  expect(screen.getByText(/lock screen and admin prompts stay blocked/i)).toBeInTheDocument();
});
