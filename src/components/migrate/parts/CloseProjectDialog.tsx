import { unfinishedParts } from "../../../lib/migrate/closing";
import { useFinish, useSetClosed } from "../../../lib/migrate/queries";
import { Button, ErrorNote, Modal } from "../../ui";

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** Closing a project by hand: what it leaves unfinished, and that it can be reopened. */
export function CloseProjectDialog({ projectId, name, onClose, onClosed }: { projectId: string; name: string; onClose: () => void; onClosed?: () => void }) {
  const finish = useFinish(projectId);
  const close = useSetClosed(projectId);
  const v = finish.data;
  const left = v ? unfinishedParts(v) : [];
  return (
    <Modal
      title={`Close ${name}?`}
      description="Tracking stops and the project moves to Closed. Nothing is sent to any device or server, and you can reopen it at any time."
      onClose={onClose}
      dismissible={!close.isPending}
      footer={
        <>
          <Button disabled={close.isPending} onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" busy={close.isPending} disabled={!v} onClick={() => close.mutate(true, { onSuccess: () => (onClosed ?? onClose)() })}>
            Close project
          </Button>
        </>
      }
    >
      <div className="space-y-2 text-sm">
        {finish.isError ? (
          <ErrorNote>{String(finish.error)}</ErrorNote>
        ) : !v ? (
          <p className="text-muted">Loading…</p>
        ) : v.unfinished > 0 ? (
          <>
            <p>
              {plural(v.unfinished, "device")} {v.unfinished === 1 ? "isn't" : "aren't"} finished and {v.unfinished === 1 ? "stays" : "stay"} as {v.unfinished === 1 ? "it is" : "they are"}:
            </p>
            <ul className="list-disc space-y-0.5 pl-5 text-muted">
              {left.map((l) => (
                <li key={l}>{l}</li>
              ))}
            </ul>
            <p className="text-xs text-faint">Reopen the project to carry on with them. Its report can be saved either way.</p>
          </>
        ) : (
          <p className="text-muted">Every device is finished.</p>
        )}
        {close.isError && <ErrorNote>{String(close.error)}</ErrorNote>}
      </div>
    </Modal>
  );
}
