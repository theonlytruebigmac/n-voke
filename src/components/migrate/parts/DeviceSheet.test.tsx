// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { TooltipProvider } from "@radix-ui/react-tooltip";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DeviceDetail, DeviceUfa } from "../../../lib/migrate/bindings";

const state = vi.hoisted(() => ({ detail: null as unknown }));

vi.mock("../../../lib/migrate/queries", () => ({
  useDevice: () => ({ data: state.detail, isError: false, error: null }),
  useDeviceActions: () => ({ command: { mutate: vi.fn(), isPending: false, isError: false, error: null } }),
}));

import { DeviceSheet } from "./DeviceSheet";

const ufa = (over: Partial<DeviceUfa> = {}): DeviceUfa => ({
  state: "blocked",
  probedAt: new Date(Date.now() - 12 * 60_000).toISOString(),
  restarts: 0,
  repairedAt: null,
  removalFailures: null,
  removalError: null,
  faultSeenAt: null,
  redeployFiles: null,
  warning: null,
  steps: [],
  ...over,
});

const detail = (deviceUfa: DeviceUfa | null): DeviceDetail => ({
  index: 0,
  name: "SUP-2190",
  stage: "healthy",
  since: new Date(Date.now() - 3600_000).toISOString(),
  wave: 1,
  path: "ecoverse",
  customer: "Demo - Acme Corporation",
  destination: "Demo - Acme HQ",
  sourceDeviceId: "1097945961",
  destDeviceId: "2007811670",
  assetId: "asset-1",
  attempts: 1,
  lastError: null,
  launchUnsure: false,
  ecoverse: "waitingForData",
  ecoverseNote: null,
  waitingForEcoverse: true,
  ecoverseOverdue: false,
  deviceUfa,
  candidates: [],
  actions: ["probe"],
  events: [],
});

const show = (d: DeviceDetail) => {
  state.detail = d;
  render(
    <TooltipProvider>
      <DeviceSheet projectId="p" index={0} onClose={() => {}} />
    </TooltipProvider>,
  );
};

afterEach(cleanup);

describe("DeviceSheet: the Ecoverse agent on the device", () => {
  it("says a device that has waited more than two hours is overdue, and not before", () => {
    show({ ...detail(null), ecoverseOverdue: true });
    expect(screen.getByText(/waited more than 2 hours for Ecoverse's data/)).toBeTruthy();
    cleanup();
    show(detail(null));
    expect(screen.queryByText(/waited more than 2 hours/)).toBeNull();
  });

  it("shows nothing about the agent until the device has been probed", () => {
    show(detail(null));
    expect(screen.queryByText("Ecoverse agent on the device")).toBeNull();
    expect(screen.getByRole("button", { name: "Probe" })).toBeTruthy();
  });

  it("says a blocked agent can't reach the login service, how old the Probe is, and to probe again", () => {
    show(detail(ufa()));
    expect(screen.getByText("Ecoverse agent on the device")).toBeTruthy();
    expect(screen.getByText("Can't reach the Ecoverse login service")).toBeTruthy();
    expect(screen.getByText(/often clears by itself/)).toBeTruthy();
    expect(screen.getByText(/As of the last Probe, 12 min ago\. Probe again to refresh\./)).toBeTruthy();
  });

  it("shows the restarts and the steps the installer took, oldest first, and its own warning", () => {
    show(
      detail(
        ufa({
          restarts: 2,
          warning: "MspAgent had not synced its data 361 min after the install (state: blocked)",
          steps: [
            { at: "2026-10-21T15:25:00Z", event: "ufa-sync-restart", detail: "restarted MspAgent (restart 1 of 1)" },
            { at: "2026-10-21T16:10:00Z", event: "ufa-repair", detail: "removed the UFA core" },
          ],
        }),
      ),
    );
    expect(screen.getByText(/Restarted 2 times\./)).toBeTruthy();
    expect(screen.getByText(/MspAgent had not synced its data 361 min/)).toBeTruthy();
    const titles = screen.getAllByText(/^(Restarted the Ecoverse agent|Repaired the Ecoverse agent)/).map((e) => e.textContent ?? "");
    expect(titles[0]).toBe("Restarted the Ecoverse agent");
    expect(titles[1]).toMatch(/^Repaired the Ecoverse agent/);
    expect(screen.getByText("removed the UFA core")).toBeTruthy();
  });

  it("reads a missing agent after the repair as being redeployed, with what its folder shows", () => {
    show(detail(ufa({ state: "notInstalled", repairedAt: "2026-10-21T16:10:00Z", redeployFiles: 4 })));
    expect(screen.getByText("Being redeployed")).toBeTruthy();
    expect(screen.getByText(/4 files written to its folder since the last restart/)).toBeTruthy();
  });

  it("tells the operator a registered agent whose program is gone is broken, with the removal failures and the step that found it", () => {
    show(
      detail(
        ufa({
          state: "broken",
          removalFailures: 5,
          removalError: 1721,
          steps: [{ at: "2026-10-21T21:58:00Z", event: "ufa-registration-broken", detail: "MSP Core Agent 3.47.0 is registered but msp-agent-core.exe is missing" }],
        }),
      ),
    );
    expect(screen.getByText("Registered but broken")).toBeTruthy();
    expect(screen.getByText(/never installs a new one/)).toBeTruthy();
    expect(screen.getByText(/Windows Installer logged 5 failed removals, the last with MSI error 1721\./)).toBeTruthy();
    expect(screen.getByText("Found the Ecoverse agent registered with its program missing")).toBeTruthy();
  });

  it("says singular for one restart, and shows a synced agent as sending data", () => {
    show(detail(ufa({ state: "synced", restarts: 1 })));
    expect(screen.getByText("Sending data")).toBeTruthy();
    expect(screen.getByText(/Restarted 1 time\./)).toBeTruthy();
  });
});
