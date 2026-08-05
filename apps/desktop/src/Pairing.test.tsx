import { invoke } from "@tauri-apps/api/core";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { Onboarding } from "./components/Onboarding";
import { PairingPanel } from "./components/PairingPanel";
import { type PairingWindow, useRampage } from "./store";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const pendingWindow: PairingWindow = {
  schema: "rampage.pairing-window.v1",
  open: true,
  open_until_ms: Date.now() + 60_000,
  requests: [{
    request_id: "ab".repeat(16),
    device_name: "Studio Laptop",
    device_kind: "desktop",
    verification_code: "4721",
    expires_at_ms: Date.now() + 60_000,
    state: "awaiting_approval",
  }],
};

afterEach(cleanup);

beforeEach(() => {
  vi.clearAllMocks();
  useRampage.setState({
    onboarding: true,
    pairingWindow: null,
    workerPairing: { state: "idle" },
    lastAction: null,
  });
});

test("makes nearby pairing the zero-copy default for a new laptop", () => {
  render(<Onboarding />);
  fireEvent.click(screen.getByRole("button", { name: "Join my fabric" }));

  expect(screen.getByRole("heading", { name: "Join your fabric" })).toBeVisible();
  expect(screen.getByText(/nothing needs to be copied, typed, or configured/i)).toBeVisible();
  expect(screen.getByRole("button", { name: /find my fabric/i })).toBeEnabled();
  expect(screen.getByText(/advanced: use a complete invite/i)).toBeVisible();
  expect(screen.getByLabelText("Signed Rampage invite")).not.toBeVisible();
});

test("asks for one approval on the main PC without showing or typing a code", () => {
  useRampage.setState({
    workerPairing: {
      state: "waiting_approval",
      request_id: "ab".repeat(16),
      owner_name: "MAIN-PC",
      verification_code: "4721",
      expires_at_ms: Date.now() + 60_000,
    },
  });
  render(<Onboarding />);
  fireEvent.click(screen.getByRole("button", { name: "Join my fabric" }));

  expect(screen.getByText(/main pc found/i)).toBeVisible();
  expect(screen.getByText((_, element) =>
    element?.tagName === "SPAN" && /approve this laptop on main-pc/i.test(element.textContent ?? ""),
  )).toBeVisible();
  expect(screen.queryByText("4721")).not.toBeInTheDocument();
});

test("uses one device approval on the automatically detected owner request", async () => {
  useRampage.setState({ pairingWindow: pendingWindow });
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "pairing_window") return pendingWindow;
    if (command === "pairing_status") return { state: "idle" };
    return pendingWindow.requests[0];
  });
  render(<PairingPanel />);

  expect(screen.queryByText("4721")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: /approve this machine/i }));

  await waitFor(() => expect(invoke).toHaveBeenCalledWith("approve_pairing", {
    requestId: pendingWindow.requests[0].request_id,
  }));
});

test("reports authenticated enrollment completion on the owner PC", () => {
  useRampage.setState({
    pairingWindow: {
      ...pendingWindow,
      requests: [{ ...pendingWindow.requests[0], state: "completed" }],
    },
  });
  render(<PairingPanel />);

  expect(screen.getByText(/connected securely/i)).toBeVisible();
  expect(screen.queryByRole("button", { name: /approve this machine/i })).not.toBeInTheDocument();
});
