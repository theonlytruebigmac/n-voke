import { describe, expect, it } from "vitest";
import { scriptsReady, stepStatus } from "./steps";

const ready = { ecoverse: { notUsed: null, state: "current" as const, org: null }, rest: { serverLabel: "", serverHost: "", itemId: null, uploadedVersion: null, state: "notThere" as const } };

describe("stepStatus", () => {
  it("marks nothing for a project that has nothing in it, or isn't loaded", () => {
    expect(stepStatus(undefined, undefined).marks).toEqual({});
    expect(stepStatus({ devices: 0, stageCounts: {} }, undefined).marks).toEqual({});
  });

  it("checks the setup steps once devices are in the project, and Prepare once the script is in place", () => {
    const planned = stepStatus({ devices: 6, stageCounts: { planned: 6 } }, ready);
    expect(planned.marks).toEqual({ connect: "done", map: "done", plan: "done", prepare: "done" });
    expect(stepStatus({ devices: 6, stageCounts: { planned: 6 } }, undefined).marks.prepare).toBeUndefined();
  });

  it("does not check Launch while any wave hasn't gone, and says how far it has got", () => {
    const some = stepStatus({ devices: 6, stageCounts: { planned: 2, staged: 4 } }, ready);
    expect(some.marks.launch).toBe("partial");
    expect(some.notes.launch).toBe("4 of 6 devices launched; 2 still waiting");
    expect(stepStatus({ devices: 6, stageCounts: { planned: 6 } }, ready).marks.launch).toBeUndefined();
  });

  it("checks Launch only when every device has been launched, and a retried device takes the check back", () => {
    expect(stepStatus({ devices: 6, stageCounts: { staged: 6 } }, ready).marks.launch).toBe("done");
    expect(stepStatus({ devices: 6, stageCounts: { staged: 5, planned: 1 } }, ready).marks.launch).toBe("partial");
  });

  it("leaves out devices taken out of the project when judging whether everything has launched", () => {
    expect(stepStatus({ devices: 5, stageCounts: { staged: 4, excluded: 1 } }, ready).marks.launch).toBe("done");
  });

  it("checks Track only when everything has moved and is working, and Finish only when the project is closed", () => {
    expect(stepStatus({ devices: 3, stageCounts: { healthy: 2, staged: 1 } }, ready).marks.track).toBeUndefined();
    expect(stepStatus({ devices: 3, stageCounts: { healthy: 3 }, ecoverseWaiting: 1 }, ready).marks.track).toBeUndefined();
    expect(stepStatus({ devices: 3, stageCounts: { healthy: 3 } }, ready).marks.track).toBe("done");
    expect(stepStatus({ devices: 3, stageCounts: { healthy: 3 } }, ready).marks.finish).toBeUndefined();
    expect(stepStatus({ devices: 3, stageCounts: { cleanedUp: 3 }, closed: true }, ready).marks.finish).toBe("done");
  });
});

describe("scriptsReady", () => {
  const rest = (state: "current" | "notThere") => ({ serverLabel: "", serverHost: "", itemId: null, uploadedVersion: null, state });
  it("wants the Ecoverse copy when Ecoverse is used and the repository copy when it isn't", () => {
    expect(scriptsReady({ ecoverse: { notUsed: null, state: "current", org: null }, rest: rest("notThere") })).toBe(true);
    expect(scriptsReady({ ecoverse: { notUsed: null, state: "outdated", org: null }, rest: rest("current") })).toBe(false);
    expect(scriptsReady({ ecoverse: { notUsed: "REST only", state: "notThere", org: null }, rest: rest("current") })).toBe(true);
    expect(scriptsReady({ ecoverse: { notUsed: "REST only", state: "notThere", org: null }, rest: rest("notThere") })).toBe(false);
  });
});
