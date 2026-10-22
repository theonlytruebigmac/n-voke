import { describe, expect, it } from "vitest";
import { ECO_WORDS, STAGE_WORDS, UFA_WORDS, describeStage, ecoWords, eventWords, movedBanner, stageView, ufaStepTitle, ufaWords } from "./words";

describe("STAGE_WORDS", () => {
  const entries = Object.entries(STAGE_WORDS);

  it("gives every stage a word and a sentence", () => {
    expect(entries.length).toBe(16);
    for (const [stage, w] of entries) {
      expect(w.short.trim(), stage).not.toBe("");
      expect(w.long.trim(), stage).not.toBe("");
    }
  });

  it("uses a different word for each stage, so a table never reads ambiguously", () => {
    const words = entries.map(([, w]) => w.short);
    expect(new Set(words).size).toBe(words.length);
  });

  it("only pulses for stages where something is happening now", () => {
    const pulsing = entries.filter(([, w]) => w.pulse).map(([s]) => s).sort();
    expect(pulsing).toEqual(["agentRemoved", "launched", "registered", "staged"]);
  });

  it("colours problems amber or red and finished work green", () => {
    for (const stage of ["unreachable", "launchFailed", "lost"]) expect(STAGE_WORDS[stage as keyof typeof STAGE_WORDS].tone).toBe("err");
    for (const stage of ["notPickedUp", "stuckStaged", "wrongPlace"]) expect(STAGE_WORDS[stage as keyof typeof STAGE_WORDS].tone).toBe("warn");
    for (const stage of ["healthy", "cleanedUp", "retired"]) expect(STAGE_WORDS[stage as keyof typeof STAGE_WORDS].tone).toBe("ok");
  });
});

describe("describeStage", () => {
  it("fills the blanks and never leaves a brace in a sentence", () => {
    for (const stage of Object.keys(STAGE_WORDS) as (keyof typeof STAGE_WORDS)[]) {
      expect(describeStage(stage, { since: "34 min", destination: "Acme", reason: "No token." })).not.toMatch(/[{}]/);
      expect(describeStage(stage, { since: "" })).not.toMatch(/[{}]/);
    }
  });

  it("says how long and why where the words ask for it", () => {
    expect(describeStage("notPickedUp", { since: "34 min" })).toContain("Launched 34 min ago and still not running");
    expect(describeStage("launchFailed", { since: "", reason: "The token expired." })).toContain("refused: The token expired.");
    expect(describeStage("healthy", { since: "", destination: "Acme HQ" })).toBe("Checking in to Acme HQ.");
  });
});

describe("a healthy device and Ecoverse", () => {
  it("reads Healthy only once Ecoverse has data, and Checking in until then", () => {
    expect(stageView("healthy", false).short).toBe("Healthy");
    expect(stageView("healthy", true)).toMatchObject({ short: "Checking in", tone: "accent" });
    expect(stageView("lost", true).short).toBe("Lost");
  });

  it("says it isn't fully working while waiting, and what that means", () => {
    expect(describeStage("healthy", { since: "", destination: "Acme HQ", waitingForEcoverse: true })).toContain("Ecoverse isn't receiving its data yet");
    expect(ECO_WORDS.waitingForData.long).toContain("scripts can't run");
    for (const state of ["waitingForData", "notSeen"] as const) expect(ECO_WORDS[state].long).toContain("hourly check");
  });

  it("has words for every state, and for one not read yet", () => {
    for (const state of ["reporting", "waitingForData", "notSeen", "notLinked"] as const) expect(ecoWords(state).short).not.toBe("");
    expect(ecoWords(null).short).toBe("Ecoverse not read yet");
  });

  it("reads a stage change as the stage and why, and other events by their kind", () => {
    expect(eventWords("stage", "AgentRemoved: the old agent has stopped")).toEqual({ title: STAGE_WORDS.agentRemoved.short, body: "the old agent has stopped" });
    expect(eventWords("stage", "Planned")).toEqual({ title: STAGE_WORDS.planned.short, body: "" });
    expect(eventWords("poll", "seen")).toEqual({ title: "Checked", body: "seen" });
    expect(eventWords("custom", "x").title).toBe("Custom");
  });

  it("only says Ecoverse is receiving a device's data when Ecoverse was checked", () => {
    expect(movedBanner(true).rest).toContain("Ecoverse is receiving its data");
    const unchecked = movedBanner(false);
    expect(`${unchecked.lead} ${unchecked.rest}`).not.toMatch(/receiving its data|and is working|fully working/);
    expect(unchecked.rest).toContain("not checked");
    expect(ECO_WORDS.notLinked.short).toBe("Ecoverse not checked");
    expect(ECO_WORDS.notLinked.long).toContain("not looked at");
  });
});

describe("the Ecoverse agent on the device", () => {
  const ufa = (state: Parameters<typeof ufaWords>[0]["state"], extra: { repairedAt?: string | null; redeployFiles?: number | null; removalFailures?: number | null; removalError?: number | null } = {}) => ({ state, repairedAt: null, redeployFiles: null, removalFailures: null, removalError: null, ...extra });

  it("has words for every state the installer can report", () => {
    for (const [state, words] of Object.entries(UFA_WORDS)) {
      expect(words.short, state).not.toBe("");
      expect(words.long, state).not.toBe("");
    }
  });

  it("says a blocked agent is watched first and what the installer does, and a rejected one that a restart doesn't help", () => {
    expect(ufaWords(ufa("blocked")).long).toContain("often clears by itself");
    expect(ufaWords(ufa("blocked")).long).toContain("port 443");
    expect(ufaWords(ufa("rejected")).long).toContain("restart doesn't help");
    expect(ufaWords(ufa("denied")).tone).toBe("err");
    expect(ufaWords(ufa("synced")).tone).toBe("ok");
  });

  it("says an agent with no device record can't be helped by a restart or the repair, and where it is fixed", () => {
    const words = ufaWords(ufa("noDeviceRecord"));
    expect(words.short).toBe("No device record in Ecoverse");
    expect(words.tone).toBe("err");
    expect(words.long).toContain("doesn't help");
    expect(words.long).toContain("Ecoverse side");
    expect(words.long).toContain("collect-logs.ps1");
  });

  it("reads a missing agent after the repair as being put back, with what its folder shows", () => {
    expect(ufaWords(ufa("notInstalled")).short).toBe("Not installed yet");
    const repaired = { repairedAt: "2026-10-21T16:10:00Z" };
    expect(ufaWords(ufa("notInstalled", repaired)).short).toBe("Being redeployed");
    expect(ufaWords(ufa("notInstalled", { ...repaired, redeployFiles: 4 })).long).toContain("4 files written");
    expect(ufaWords(ufa("notInstalled", { ...repaired, redeployFiles: 1 })).long).toContain("1 file written");
    expect(ufaWords(ufa("notProvisioned", { ...repaired, redeployFiles: 0 })).long).toContain("Nothing new in its folder");
    expect(ufaWords(ufa("blocked", repaired)).short).toBe(UFA_WORDS.blocked.short);
  });

  it("names the installer's steps, and shows an unknown one as it is", () => {
    expect(ufaStepTitle("ufa-sync-restart")).toBe("Restarted the Ecoverse agent");
    expect(ufaStepTitle("ufa-repair")).toContain("Repaired");
    expect(ufaStepTitle("ufa-repair-restart")).toContain("again");
    expect(ufaStepTitle("ufa-registration-broken")).toContain("program missing");
    expect(ufaStepTitle("something-new")).toBe("something-new");
  });

  it("says a registered agent whose program is gone can't be fixed by the installer, and what Windows Installer logged", () => {
    const plain = ufaWords(ufa("broken"));
    expect(plain.short).toBe("Registered but broken");
    expect(plain.tone).toBe("err");
    expect(plain.long).toContain("never installs a new one");
    expect(plain.long).toContain("The installer can't fix this");
    expect(plain.long).not.toContain("Windows Installer logged");
    const logged = ufaWords(ufa("broken", { removalFailures: 5, removalError: 1721 }));
    expect(logged.long).toContain("Windows Installer logged 5 failed removals, the last with MSI error 1721.");
    expect(ufaWords(ufa("broken", { removalFailures: 1 })).long).toContain("1 failed removal.");
    expect(ufaWords(ufa("notInstalled", { removalFailures: 5, removalError: 1721 })).short).toBe("Not installed yet");
  });
});
