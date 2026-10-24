import { useState } from "react";
import { assetIdFromTaskId, getAssetName, getTask } from "../lib/api";
import { jobStore, newTask } from "../lib/jobs";
import { applySnapshot } from "../lib/taskState";
import { Button, Field, Modal, inputClass } from "./ui";

/** Adds an existing task (e.g. one started from the PowerShell script) as a job. */
export function WatchTaskDialog({ onClose, onAdded }: { onClose: () => void; onAdded: (jobId: string) => void }) {
  const [taskId, setTaskId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const watch = async () => {
    const id = taskId.trim().replace(/^'|'$/g, "");
    if (!id) return;
    setBusy(true);
    setError(null);
    try {
      const { task } = await getTask(id);
      if (!task) throw new Error("No task found with that ID.");

      const assetId = assetIdFromTaskId(id);
      const deviceName = (assetId && (await getAssetName(assetId).catch(() => null))) || "Unknown device";
      const jobTask = applySnapshot(newTask(id, assetId, deviceName), task);
      jobStore.add({
        id,
        name: task.name ?? "Untitled task",
        scriptId: task.script?.id ?? null,
        scriptName: task.script?.name ?? "Unknown script",
        orgId: null,
        orgName: null,
        targetLabel: deviceName,
        createdAt: task.createdAt ?? new Date().toISOString(),
        timeoutSeconds: 0,
        tasks: [jobTask],
      });
      onAdded(id);
      onClose();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title="Watch a task"
      description="Follow a run started somewhere else, like the PowerShell script."
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} disabled={!taskId.trim()} onClick={() => void watch()}>
            Watch
          </Button>
        </>
      }
    >
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          void watch();
        }}
      >
        <Field label="Task ID" error={error}>
          <input
            className={`${inputClass} font-mono ${error ? "border-err" : ""}`}
            value={taskId}
            onChange={(e) => {
              setTaskId(e.target.value);
              setError(null);
            }}
            placeholder="MDFhMTRhZjMt…"
            aria-invalid={!!error}
            autoFocus
            spellCheck={false}
          />
        </Field>
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
