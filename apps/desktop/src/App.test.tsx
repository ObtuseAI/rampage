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
