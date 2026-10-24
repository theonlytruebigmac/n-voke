import { useState, type ReactNode } from "react";
import { inputClass } from "../../ui";

export interface NameItem {
  key: string;
  text: ReactNode;
  /** Plain text to search on, when `text` isn't a string. */
  search?: string;
  className?: string;
}

const SHOWN = 50;

/**
 * A list of device names inside a confirmation or result. A project can hold thousands, so only the first 50 are
 * drawn; past that a search box narrows the list, and the rest are counted.
 */
export function NameList({ items, className = "" }: { items: NameItem[]; className?: string }) {
  const [term, setTerm] = useState("");
  const needle = term.trim().toLowerCase();
  const matching = needle ? items.filter((i) => (i.search ?? (typeof i.text === "string" ? i.text : i.key)).toLowerCase().includes(needle)) : items;
  const shown = matching.slice(0, SHOWN);
  return (
    <div className={`space-y-2 ${className}`}>
      {items.length > SHOWN && <input className={inputClass} placeholder={`Search ${items.length.toLocaleString()} devices`} value={term} onChange={(e) => setTerm(e.target.value)} aria-label="Search the list" />}
      <ul className="max-h-48 list-disc space-y-0.5 overflow-y-auto pl-5">
        {shown.map((i) => (
          <li key={i.key} className={`break-words ${i.className ?? ""}`}>
            {i.text}
          </li>
        ))}
      </ul>
      {matching.length > shown.length && <p className="text-xs text-faint">and {(matching.length - shown.length).toLocaleString()} more</p>}
      {needle && matching.length === 0 && <p className="text-xs text-faint">None match.</p>}
    </div>
  );
}
