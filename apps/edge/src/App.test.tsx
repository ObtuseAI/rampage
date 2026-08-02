/** @vitest-environment jsdom */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const safeIdleView = {
  native: {
    platform: "android",
    deviceKind: "phone",
    foreground: true,
    donationRequested: false,
    batteryPercent: 78,
    onExternalPower: true,
    lowPowerMode: false,
    thermalHeadroomPercent: 82,
    screenKeptAwake: false
  },
  message: "Ready for an owner-authorized foreground session."
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(safeIdleView);
});

afterEach(cleanup);

test("shows the foreground-only safety boundary", async () => {
  render(<App />);

  expect(await screen.findByText("THE HARD BOUNDARY")).toBeTruthy();
  expect(screen.getByText(/never becomes remote RAM/i)).toBeTruthy();
  expect(screen.getByRole("button", { name: /start foreground donation/i })).toBeTruthy();
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("edge_status"));
});

test("submits only an explicitly pasted invitation and trimmed device name", async () => {
  render(<App />);
  await screen.findByText("THE HARD BOUNDARY");

  fireEvent.change(screen.getByLabelText(/device name/i), {
    target: { value: "  My Tablet  " }
  });
  fireEvent.change(screen.getByLabelText(/signed invitation/i), {
    target: { value: "  signed-invitation  " }
  });
  fireEvent.click(screen.getByRole("button", { name: /start foreground donation/i }));

  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("edge_start", {
      invitation: "signed-invitation",
      displayName: "My Tablet"
    })
  );
});
