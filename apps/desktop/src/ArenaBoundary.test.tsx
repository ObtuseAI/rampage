import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { Arena } from "./components/Arena";
import { ArenaBoundary, ArenaLoading } from "./components/ArenaBoundary";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

test("offers the accessible grid when WebGL rendering fails", () => {
  const openGrid = vi.fn();
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  const BrokenArena = () => {
    throw new Error("WebGL unavailable");
  };

  render(<ArenaBoundary openGrid={openGrid}><BrokenArena /></ArenaBoundary>);
  expect(screen.getByText(/3d arena could not start/i)).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /open ops grid/i }));
  expect(openGrid).toHaveBeenCalledOnce();
});

test("never leaves a user trapped behind an endless initializing message", () => {
  vi.useFakeTimers();
  const openGrid = vi.fn();
  render(<ArenaLoading openGrid={openGrid} />);
  expect(screen.getByText(/initializing spatial fabric/i)).toBeVisible();

  act(() => vi.advanceTimersByTime(4_100));
  expect(screen.getByText(/taking longer than expected/i)).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /open ops grid/i }));
  expect(openGrid).toHaveBeenCalledOnce();
});

test("announces a real WebGL capability failure and offers the grid", () => {
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  const openGrid = vi.fn();
  render(<Arena openGrid={openGrid} />);

  expect(screen.getByText("3D acceleration is unavailable.")).toBeVisible();
  fireEvent.click(screen.getByRole("button", { name: /open ops grid/i }));
  expect(openGrid).toHaveBeenCalledOnce();
});
