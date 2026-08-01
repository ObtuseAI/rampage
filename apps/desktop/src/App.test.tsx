import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import { useRampage } from "./store";

vi.mock("./components/Arena", () => ({ Arena: () => <div>Spatial fabric</div> }));

afterEach(cleanup);

beforeEach(() => {
  localStorage.setItem("rampage.onboarded", "true");
  localStorage.setItem("rampage.compute-strategy", "maximum_model_size");
  useRampage.setState({ onboarding: false, mode: "arena", commandOpen: false, computeStrategy: "maximum_model_size", modelPlan: null, modelPlanPending: false, gatewayModels: [], runAtLogin: false });
  globalThis.fetch = vi.fn().mockRejectedValue(new Error("offline"));
});

test("provides accessible grid parity", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Grid" }));
  expect(screen.getByRole("button", { name: /command rig/i })).toBeInTheDocument();
  expect(screen.getByLabelText("Fabric nodes")).toBeInTheDocument();
  expect(screen.getByLabelText("Encrypt file locally")).toBeInTheDocument();
});

test("local stop does not depend on controller", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "STOP" }));
  expect(useRampage.getState().capability).toBe("read_only");
});

test("separates maximum model size from measured speed boost", () => {
  render(<App />);
  expect(screen.getByRole("button", { name: /maximum model/i })).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(screen.getByRole("button", { name: /speed boost/i }));
  expect(useRampage.getState().computeStrategy).toBe("speed_boost");
  expect(screen.getByText(/use tensor peers only when measured links predict faster tokens/i)).toBeInTheDocument();
  expect(localStorage.getItem("rampage.compute-strategy")).toBe("speed_boost");
});

test("exposes installed desktop lifecycle control", () => {
  render(<App />);
  expect(screen.getByRole("button", { name: "Start Rampage with Windows" })).toBeInTheDocument();
});

test("shows when the OpenAI-compatible whole-model gateway is ready", () => {
  useRampage.setState({ gatewayModels: ["gemma3:4b"] });
  render(<App />);
  expect(screen.getByText(/1 consistent installed model · OpenAI API ready/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Copy API setup" })).toBeEnabled();
});
