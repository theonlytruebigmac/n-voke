import type { ProjectSummary } from "../../../lib/migrate/bindings";
import { projectProgress, readyToFinish } from "../../../lib/migrate/progress";
import { Button } from "../../ui";

/**
 * Where the project stands, on every step: how many devices have moved and are working, how many wait for Ecoverse,
 * and how many need the person (who can jump to Track from here). A project with nothing in it shows nothing.
 */
export function ProjectStatus({ project, onReview }: { project: ProjectSummary; onReview: () => void }) {
  const { total, done, fraction } = projectProgress(project);
  if (total === 0) return null;
  const waiting = project.ecoverseWaiting;
  const done100 = readyToFinish(project);
  return (
    <div className="flex flex-wrap items-center gap-x-5 gap-y-1.5 border-b border-border px-6 py-2 text-xs">
      <div className="flex items-center gap-2.5">
        <div className="h-1.5 w-32 overflow-hidden rounded-full bg-surface-3" role="progressbar" aria-valuenow={Math.round(fraction * 100)} aria-valuemin={0} aria-valuemax={100} aria-label="Devices moved and working">
          <div className="h-full rounded-full bg-ok transition-[width]" style={{ width: `${Math.round(fraction * 100)}%` }} />
        </div>
        <span className="tabular-nums text-muted">
          {done} of {total} moved and working
        </span>
      </div>
      {waiting > 0 && <span className="text-warn">{waiting} waiting for Ecoverse</span>}
      {project.attention > 0 ? (
        <Button size="sm" variant="ghost" onClick={onReview}>
          <span className="text-warn">{project.attention} need you</span>
          <span className="text-muted">· Review</span>
        </Button>
      ) : done100 ? (
        <span className="text-ok">Everything has moved and is working</span>
      ) : null}
    </div>
  );
}
