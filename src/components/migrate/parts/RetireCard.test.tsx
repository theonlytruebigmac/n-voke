// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { TooltipProvider } from "@radix-ui/react-tooltip";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RetireView } from "../../../lib/migrate/bindings";

const state = vi.hoisted(() => ({ view: null as unknown, mutate: vi.fn(), reset: vi.fn() }));

vi.mock("../../../lib/migrate/queries", () => ({
  useRetire: () => ({ data: state.view, isError: false, error: null }),
  useRetireActions: () => ({ mutate: state.mutate, reset: state.reset, isPending: false, isError: false, error: null }),
}));

import { RetireCard } from "./RetireCard";

const view = (over: Partial<RetireView> = {}): RetireView => ({
  turnedOn: true,
  sourceHost: "old.example.com",
  reviewDays: 7,
  ready: [],
  inReview: 3,
  waiting: [
    { name: "PC-1", reviewEnds: "2026-10-28T12:00:00Z" },
    { name: "PC-2", reviewEnds: "2026-10-29T12:00:00Z" },
    { name: "PC-3", reviewEnds: "2026-10-30T12:00:00Z" },
  ],
  nextReviewEnds: "2026-10-28T12:00:00Z",
  notCleanedUp: 0,
  ...over,
});

const open = () =>
  render(
    <TooltipProvider>
      <RetireCard projectId="p" onOptions={() => {}} />
    </TooltipProvider>,
  );
const normal = () => screen.getByRole("button", { name: /^Retire old records/ });
const early = () => screen.getByRole("button", { name: /Retire before the review period ends/ });

beforeEach(() => {
  state.mutate.mockReset();
  state.reset.mockReset();
});
afterEach(cleanup);

describe("RetireCard: before the review period ends", () => {
  it("offers a way past the review period when nothing is ready but devices are still in it", () => {
    state.view = view();
    open();
    expect((normal() as HTMLButtonElement).disabled).toBe(true);
    expect((early() as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByText(/3 still in the review period/)).toBeTruthy();
  });

  it("offers nothing extra when no device is inside its review period", () => {
    state.view = view({ ready: ["PC-9"], inReview: 0, waiting: [], nextReviewEnds: null });
    open();
    expect((normal() as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByRole("button", { name: /Retire before the review period ends/ })).toBeNull();
  });

  it("warns, lists the devices with their review end dates, and keeps Delete off until the tick", () => {
    state.view = view();
    open();
    fireEvent.click(early());

    expect(screen.getByText(/This skips the review period/)).toBeTruthy();
    expect(screen.getByText(/3 devices are still inside it/)).toBeTruthy();
    expect(screen.getByText(/can't be brought back/)).toBeTruthy();
    for (const name of ["PC-1", "PC-2", "PC-3"]) expect(screen.getByText(new RegExp(`^${name}`))).toBeTruthy();
    expect(screen.getAllByText(/review ends/).length).toBe(3);

    const del = screen.getByRole("button", { name: "Delete the records" }) as HTMLButtonElement;
    expect(del.disabled).toBe(true);
    fireEvent.click(del);
    expect(state.mutate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByLabelText(/I understand this skips the review period/));
    expect(del.disabled).toBe(false);
    fireEvent.click(del);
    expect(state.mutate).toHaveBeenCalledTimes(1);
    expect(state.mutate.mock.calls[0][0]).toEqual({ dryRun: false, beforeReviewEnds: true });
  });

  it("needs no tick for a dry run, which changes nothing, and still says it covers the early ones", () => {
    state.view = view();
    open();
    fireEvent.click(early());
    fireEvent.click(screen.getByLabelText(/Dry run/));
    const show = screen.getByRole("button", { name: "Show what would go" }) as HTMLButtonElement;
    expect(show.disabled).toBe(false);
    fireEvent.click(show);
    expect(state.mutate.mock.calls[0][0]).toEqual({ dryRun: true, beforeReviewEnds: true });
  });

  it("asks for the tick again each time the dialog is opened", () => {
    state.view = view();
    open();
    fireEvent.click(early());
    fireEvent.click(screen.getByLabelText(/I understand this skips the review period/));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(early());
    expect((screen.getByLabelText(/I understand this skips the review period/) as HTMLInputElement).checked).toBe(false);
    expect((screen.getByRole("button", { name: "Delete the records" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("RetireCard: the normal retire", () => {
  it("has no warning and no tick, and only covers the devices past their review period", () => {
    state.view = view({ ready: ["PC-9"], inReview: 1, waiting: [{ name: "PC-1", reviewEnds: "2026-10-28T12:00:00Z" }] });
    open();
    fireEvent.click(normal());
    expect(screen.queryByText(/This skips the review period/)).toBeNull();
    expect(screen.queryByLabelText(/I understand this skips the review period/)).toBeNull();
    expect(screen.getByText("PC-9")).toBeTruthy();
    expect(screen.queryByText(/^PC-1/)).toBeNull();
    const del = screen.getByRole("button", { name: "Delete the records" }) as HTMLButtonElement;
    expect(del.disabled).toBe(false);
    fireEvent.click(del);
    expect(state.mutate.mock.calls[0][0]).toEqual({ dryRun: false, beforeReviewEnds: false });
  });
});
