import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import { useRampage } from "./store";

vi.mock("./components/Arena", () => ({ Arena: () => <div>Spatial fabric</div> }));

afterEach(cleanup);

beforeEach(() => {
  localStorage.setItem("rampage.onboarded", "true");
  useRampage.setState({ onboarding: false, mode: "arena", commandOpen: false });
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
