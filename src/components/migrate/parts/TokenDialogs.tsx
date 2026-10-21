import type { TokenReport, TokenRow } from "../../../lib/migrate/bindings";
import { Button, ErrorNote, Modal } from "../../ui";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** Asks before creating registration tokens on the destination, naming every unit that gets one. */
export function GenerateTokensDialog({ missing, busy, error, onConfirm, onClose }: { missing: TokenRow[]; busy: boolean; error: string | null; onConfirm: () => void; onClose: () => void }) {
  return (
    <Modal
      title={`Create ${plural(missing.length, "registration token")}?`}
      description="The destination server gets a token for each customer or site below that devices are going to. Units that already have a usable token are left alone."
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={onConfirm} busy={busy} disabled={missing.length === 0}>
            Create
          </Button>
        </>
      }
    >
      <ul className="max-h-64 space-y-1 overflow-y-auto text-sm">
        {missing.map((t) => (
          <li key={t.destId}>{t.destName}</li>
        ))}
      </ul>
      {error && (
        <div className="mt-3">
          <ErrorNote>{error}</ErrorNote>
        </div>
      )}
    </Modal>
  );
}

/** What creating the tokens did: made, already fine, expired (N-central has to renew), held back, failed. */
export function TokenResultDialog({ report, onClose }: { report: TokenReport; onClose: () => void }) {
  const lines: { key: string; text: string; names?: string[]; tone?: string }[] = [];
  if (report.generated.length) lines.push({ key: "generated", text: `Created ${plural(report.generated.length, "token")}`, names: report.generated });
  if (report.alreadyFine) lines.push({ key: "fine", text: `${plural(report.alreadyFine, "unit")} already had a usable token` });
  if (report.expired.length) lines.push({ key: "expired", text: "Expired: N-central has to renew these", names: report.expired, tone: "text-warn" });
  if (report.heldBack) lines.push({ key: "held", text: `${plural(report.heldBack, "token")} not created: the app is in dry-run mode`, tone: "text-warn" });
  if (report.failed.length) lines.push({ key: "failed", text: `${plural(report.failed.length, "token")} failed`, names: report.failed.map(([name, why]) => `${name}: ${why}`), tone: "text-err" });
  return (
    <Modal title="Registration tokens" onClose={onClose} footer={<Button onClick={onClose}>Done</Button>}>
      <div className="space-y-3 text-sm">
        {lines.length === 0 && <p className="text-muted">Nothing needed a token.</p>}
        {lines.map((l) => (
          <div key={l.key}>
            <p className={l.tone ?? ""}>{l.text}</p>
            {l.names && (
              <ul className="mt-1 max-h-40 space-y-0.5 overflow-y-auto pl-4 text-muted">
                {l.names.map((n) => (
                  <li key={n} className="list-disc break-words">
                    {n}
                  </li>
                ))}
              </ul>
            )}
          </div>
        ))}
      </div>
    </Modal>
  );
}
