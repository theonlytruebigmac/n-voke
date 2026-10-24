import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";

/**
 * Where a step's own buttons go in the page header, beside the title, so they sit on the title's line rather than in
 * a row of their own. The Workspace owns the header and provides the spot; the step keeps the buttons' state.
 */
export const HeaderActionsSlot = createContext<HTMLElement | null>(null);

/** Puts its children in the page header's action area; renders nothing until the header is there. */
export function HeaderActions({ children }: { children: ReactNode }) {
  const slot = useContext(HeaderActionsSlot);
  return slot ? createPortal(children, slot) : null;
}
