import { describe, expect, it } from "vitest";
import type { PrepareView, ScriptState } from "./bindings";
import { prepareChecklist } from "./checklist";

const rest = (state: ScriptState) => ({ serverLabel: "S", serverHost: "h", itemId: null, uploadedVersion: null, state });
const eco = (state: ScriptState, notUsed: string | null = null) => ({ notUsed, state, org: null });
const view = (e: ReturnType<typeof eco>, r: ScriptState, destination: PrepareView["destination"] = null) => ({ ecoverse: e, rest: rest(r), destination });

describe("prepareChecklist", () => {
  it("needs the Ecoverse script when Ecoverse is used and calls the repository script optional", () => {
    const c = prepareChecklist(view(eco("notThere"), "notThere"));
    expect(c.ready).toBe(false);
    expect(c.items.map((i) => [i.anchor, i.status])).toEqual([["prepare-source-ecoverse", "needed"], ["prepare-source-rest", "optional"]]);
    const done = prepareChecklist(view(eco("current"), "notThere"));
    expect(done.ready).toBe(true);
    expect(done.items[0].status).toBe("ready");
  });

  it("needs the repository script when Ecoverse isn't used, and drops the Ecoverse line", () => {
    const c = prepareChecklist(view(eco("notThere", "REST only"), "outdated"));
    expect(c.items).toHaveLength(1);
    expect(c.items[0]).toMatchObject({ status: "needed", text: "Out of date: publish it again" });
    expect(c.ready).toBe(false);
    expect(prepareChecklist(view(eco("notThere", "REST only"), "current")).ready).toBe(true);
  });

  it("lists the destination's scripts as optional, since they are made when first needed", () => {
    const c = prepareChecklist(view(eco("current"), "current", { ecoverse: eco("notThere"), rest: rest("notThere") }));
    expect(c.items.slice(2).map((i) => i.status)).toEqual(["optional", "optional"]);
    expect(c.ready).toBe(true);
  });
});
