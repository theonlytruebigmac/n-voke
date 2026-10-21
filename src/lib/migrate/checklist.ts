import type { PrepareView, ScriptState } from "./bindings";
import { scriptsReady } from "./steps";

export type ItemStatus = "ready" | "needed" | "optional";

export interface ChecklistItem {
  /** The card on the page that handles it. */
  anchor: string;
  label: string;
  status: ItemStatus;
  text: string;
}

const STATE_TEXT: Record<ScriptState, string> = { current: "Up to date", outdated: "Out of date: publish it again", notThere: "Not there yet" };

/**
 * What Prepare asks for, in one list, so the person sees what is left without reading every card. The source needs the
 * script where it launches from (Ecoverse, or the repository when Ecoverse isn't used); the rest is optional or done
 * for them when it is first needed.
 */
export function prepareChecklist(view: Pick<PrepareView, "ecoverse" | "rest" | "destination">): { items: ChecklistItem[]; ready: boolean } {
  const items: ChecklistItem[] = [];
  const ecoUsed = !view.ecoverse.notUsed;
  if (ecoUsed) {
    const s = view.ecoverse.state;
    items.push({ anchor: "prepare-source-ecoverse", label: "Script in Ecoverse", status: s === "current" ? "ready" : "needed", text: STATE_TEXT[s] });
  }
  const restState = view.rest.state;
  items.push({
    anchor: "prepare-source-rest",
    label: "Script in the source's repository",
    status: restState === "current" ? "ready" : ecoUsed ? "optional" : "needed",
    text: restState === "current" ? STATE_TEXT.current : ecoUsed ? `${STATE_TEXT[restState]} (the fallback for devices Ecoverse can't reach)` : STATE_TEXT[restState],
  });
  if (view.destination?.ecoverse && !view.destination.ecoverse.notUsed) {
    const s = view.destination.ecoverse.state;
    items.push({ anchor: "prepare-destination-ecoverse", label: "Script in the destination's Ecoverse", status: s === "current" ? "ready" : "optional", text: s === "current" ? STATE_TEXT.current : "Published for you when a probe or cleanup first needs it" });
  }
  if (view.destination) {
    const s = view.destination.rest.state;
    items.push({ anchor: "prepare-destination-rest", label: "Script in the destination's repository", status: s === "current" ? "ready" : "optional", text: s === "current" ? STATE_TEXT.current : "Only needed to reach a moved device Ecoverse can't" });
  }
  return { items, ready: scriptsReady(view) };
}
