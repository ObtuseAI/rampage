import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { RecoveryCenter } from "./components/RecoveryCenter";
import { type RecoveryStatus, useRampage } from "./store";

const workerStatus: RecoveryStatus = {
  schema: "rampage.recovery-status.v1",
  version: "0.3.1",
  role: "worker",
  state: "enrolled_worker",
  healthy: true,
  issues: [],
  canLeaveFabric: true,
  canFactoryReset: true,
  nodes: [],
};

afterEach(cleanup);

beforeEach(() => {
  useRampage.setState({
    recoveryOpen: true,
    recoveryStatus: workerStatus,
    repairConnection: vi.fn().mockResolvedValue(undefined),
    leaveFabric: vi.fn().mockResolvedValue(undefined),
    factoryReset: vi.fn().mockResolvedValue(undefined),
    forgetNode: vi.fn().mockResolvedValue(undefined),
    refreshRecovery: vi.fn().mockResolvedValue(undefined),
  });
});

test("lets a worker repair or return to pairing without copying an identity", async () => {
  render(<RecoveryCenter />);

  fireEvent.click(screen.getByRole("button", { name: /fix rampage/i }));
  expect(useRampage.getState().repairConnection).toHaveBeenCalledOnce();

  fireEvent.click(screen.getByRole("button", { name: /pair again/i }));
  expect(screen.getByRole("heading", { name: /start pairing over/i })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /leave fabric/i }));

  await waitFor(() => expect(useRampage.getState().leaveFabric).toHaveBeenCalledWith("LEAVE FABRIC"));
});

test("keeps enrolled-machine revocation in advanced recovery with a clear second confirmation", async () => {
  const nodeId = "0198f1aa-9f18-7dc3-81a3-d78f22efb662";
  useRampage.setState({
    recoveryStatus: {
      ...workerStatus,
      role: "owner",
      state: "owner_fabric",
      canLeaveFabric: false,
      nodes: [{
        nodeId,
        displayName: "Studio Laptop",
        platform: "windows-x86_64",
        deviceKind: "desktop",
        live: false,
        local: false,
      }],
    },
  });
  render(<RecoveryCenter />);

  fireEvent.click(screen.getByText(/advanced recovery and enrolled devices/i));
  fireEvent.click(screen.getByRole("button", { name: /forget/i }));
  expect(screen.getByRole("heading", { name: /forget studio laptop/i })).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /forget machine/i }));

  await waitFor(() => expect(useRampage.getState().forgetNode).toHaveBeenCalledWith(nodeId, `FORGET ${nodeId}`));
});

test("requires the full factory-reset phrase before destructive reset is enabled", () => {
  render(<RecoveryCenter />);

  fireEvent.click(screen.getByText(/advanced recovery and enrolled devices/i));
  fireEvent.click(screen.getByRole("button", { name: /factory reset/i }));
  const reset = screen.getByRole("button", { name: /reset rampage/i });
  expect(reset).toBeDisabled();
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "RESET RAMPAGE" } });
  expect(reset).toBeEnabled();
});
