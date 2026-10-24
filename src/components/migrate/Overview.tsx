import { ArrowRight, ChevronRight } from "lucide-react";
import type { GroupCount, LaunchView, NeedKind, ProjectSummary, TrackGroup, WaveRow } from "../../lib/migrate/bindings";
import { projectProgress, readyToFinish } from "../../lib/migrate/progress";
import { useLaunch, useTrack, useTracker } from "../../lib/migrate/queries";
import type { StepStatus } from "../../lib/migrate/steps";
import { GROUP_LABELS } from "../../lib/migrate/track";
import { STAGE_WORDS } from "../../lib/migrate/words";
import { Button, Card, StepTitle, Timeline, type StepState, type TimelineStep } from "../ui";
import { SETUP_STEPS, SetupChips, type StepId } from "./parts/StepRail";
import { usePublishTracker } from "./TrackerStatus";

/** Only the counts and the needs-you groups are wanted from Track here, so one row is enough. */
const COUNTS_ONLY = { group: null, wave: null, customer: null, text: "", offset: 0, limit: 1 } as const;

/** Colour of each Track group in the stacked bar and its legend. */
const GROUP_COLOR: Record<TrackGroup, string> = {
  planned: "bg-border-strong",
  launched: "bg-accent",
  staged: "bg-accent",
  moving: "bg-accent",
  onNew: "bg-accent",
  waiting: "bg-accent/45",
  healthy: "bg-ok",
  done: "bg-ok/60",
  needsYou: "bg-warn",
};

const n = (x: number) => x.toLocaleString();
const plural = (x: number, word: string) => `${n(x)} ${word}${x === 1 ? "" : "s"}`;

/**
 * A project's landing view (the "project overview" mockup): its four phases on a timeline, the one next thing to do
 * opened under the current phase, and on the right what needs the person and where the devices are. Each step is
 * reachable from here; the step pages lead back with the breadcrumb.
 */
export function Overview({ projectId, project, status, onGo }: { projectId: string; project: ProjectSummary; status: StepStatus; onGo: (step: StepId) => void }) {
  const launch = useLaunch(projectId);
  const track = useTrack(projectId, COUNTS_ONLY);
  const { marks, notes } = status;

  const setupDone = SETUP_STEPS.every((s) => marks[s.id] === "done");
  const firstOpen = SETUP_STEPS.find((s) => marks[s.id] !== "done");
  const active = project.devices - (project.stageCounts.excluded ?? 0);
  const planned = project.stageCounts.planned ?? 0;
  const launched = Math.max(0, active - planned);
  const finished = readyToFinish(project);
  // A project used to open on Track once it had launched, and Track is what keeps the tracker running. The overview
  // is the landing view now, so it keeps the tracker going too; otherwise its counts would go stale.
  const tracking = launched > 0 && !project.closed;
  const tracker = useTracker(projectId, tracking);
  usePublishTracker(project.name, tracking ? tracker : null);

  const setupState: StepState = setupDone ? "done" : "current";
  const launchState: StepState = marks.launch === "done" ? "done" : setupDone ? "current" : "upcoming";
  const trackState: StepState = marks.track === "done" ? "done" : project.attention > 0 ? "attention" : launched > 0 ? "current" : "upcoming";
  const finishState: StepState = project.closed ? "done" : finished ? "current" : "upcoming";

  const steps: TimelineStep[] = [
    {
      key: "setup",
      state: setupState,
      content: (
        <div className="space-y-3">
          <StepTitle hint={setupDone ? "Done" : `${SETUP_STEPS.filter((s) => marks[s.id] === "done").length} of ${SETUP_STEPS.length} done`}>Set up</StepTitle>
          <SetupChips marks={marks} notes={notes} onSelect={onGo} />
          {!setupDone && firstOpen && (
            <NextCard
              title={`Continue with ${firstOpen.label}`}
              text={SETUP_NEXT[firstOpen.id] ?? ""}
              action={
                <Button variant="primary" onClick={() => onGo(firstOpen.id)}>
                  Open {firstOpen.label}
                </Button>
              }
            />
          )}
        </div>
      ),
    },
    {
      key: "launch",
      state: launchState,
      content: <LaunchPhase state={launchState} view={launch.data} onGo={onGo} note={notes.launch} />,
    },
    {
      key: "track",
      state: trackState,
      content: (
        <PhaseLine
          title="Track"
          hint={
            launched === 0 ? (
              "Each wave lands here as it starts"
            ) : (
              <>
                {plural(launched, "device")} launched
                {project.attention > 0 && <span className="text-warn"> · {n(project.attention)} need you</span>}
              </>
            )
          }
          onOpen={launched > 0 ? () => onGo("track") : undefined}
        />
      ),
    },
    {
      key: "finish",
      state: finishState,
      content: (
        <PhaseLine
          title="Finish"
          hint={project.closed ? "Closed" : finished ? "Everything has moved. Clean up and save the report." : "Clean up once every device is working"}
          onOpen={launched > 0 ? () => onGo("finish") : undefined}
        />
      ),
    },
  ];

  return (
    <div className="flex flex-wrap items-start gap-6">
      <div className="min-w-0 flex-[3_1_480px]">
        <Timeline label="Phases" steps={steps} />
      </div>
      <div className="flex min-w-0 flex-[2_1_300px] flex-col gap-4">
        {track.data && track.data.needsYouTotal > 0 && <NeedsYouCard total={track.data.needsYouTotal} kinds={track.data.needsYou} onReview={() => onGo("track")} />}
        <WhereDevicesAre project={project} counts={track.data?.counts} />
      </div>
    </div>
  );
}

/** What each unfinished set-up step asks for, in one line. */
const SETUP_NEXT: Partial<Record<StepId, string>> = {
  connect: "Check that both servers and Ecoverse are reachable and signed in.",
  map: "Choose where each customer and site goes on the destination.",
  prepare: "Put the endpoint script where devices can get it, and set the options.",
  plan: "Choose which devices this project moves.",
};

function NextCard({ title, text, action }: { title: string; text: string; action: React.ReactNode }) {
  return (
    <Card tone="accent" className="flex flex-wrap items-center gap-x-5 gap-y-3 px-4 py-3.5">
      <div className="min-w-0 flex-[1_1_260px]">
        <div className="text-[11px] font-semibold tracking-wider text-accent uppercase">Next</div>
        <div className="text-base font-semibold">{title}</div>
        <div className="text-muted">{text}</div>
      </div>
      {action}
    </Card>
  );
}

function PhaseLine({ title, hint, onOpen }: { title: string; hint: React.ReactNode; onOpen?: () => void }) {
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
      <div className="min-w-0 flex-1">
        <StepTitle hint={hint}>{title}</StepTitle>
      </div>
      {onOpen && <OpenButton step={title} onClick={onOpen} />}
    </div>
  );
}

/** The way into a phase's own page from the overview: a real button, so it reads as one. */
function OpenButton({ step, onClick }: { step: string; onClick: () => void }) {
  return (
    <Button size="sm" variant="tinted" onClick={onClick} aria-label={`Open ${step}`}>
      Open
      <ChevronRight />
    </Button>
  );
}

function LaunchPhase({ state, view, onGo, note }: { state: StepState; view: LaunchView | undefined; onGo: (step: StepId) => void; note?: string }) {
  const waves = view?.waves ?? [];
  const out = waves.filter((w) => w.launchedAt).length;
  const ready = waves.find((w) => w.waiting > 0);
  const hint =
    state === "upcoming"
      ? "Comes after set up"
      : view
        ? [waves.length ? `${plural(out, "wave")} out` : "No waves yet", view.unassigned > 0 ? `${n(view.unassigned)} devices not in a wave` : null].filter(Boolean).join(" · ")
        : note;
  const latest = [...waves].sort((a, b) => b.number - a.number).slice(0, 3);

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <div className="min-w-0 flex-1">
          <StepTitle hint={hint}>Launch</StepTitle>
        </div>
        {state !== "upcoming" && waves.length > 0 && <OpenButton step="Launch" onClick={() => onGo("launch")} />}
      </div>
      {state === "current" && view && (ready || view.unassigned > 0) && (
        <NextCard
          title={ready ? `Launch ${ready.label}` : waves.length ? "Make the next wave" : "Make the first wave"}
          text={
            ready
              ? `${plural(ready.waiting, "device")} ready. You'll confirm before old agents are removed.`
              : `${plural(view.unassigned, "planned device")} aren't in a wave yet. Pick them by number or one by one.`
          }
          action={
            <Button variant="primary" onClick={() => onGo("launch")}>
              {ready ? "Launch…" : "Choose devices"}
              <ArrowRight />
            </Button>
          }
        />
      )}
      {state !== "upcoming" && latest.length > 0 && (
        <div className="divide-y divide-border rounded-xl border border-border">
          {latest.map((w) => (
            <WaveLine key={w.number} wave={w} />
          ))}
          {waves.length > latest.length && (
            <button type="button" onClick={() => onGo("launch")} className="block w-full px-3.5 py-2.5 text-left text-sm font-medium text-accent hover:text-accent-hover">
              Show all {waves.length} waves
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function WaveLine({ wave }: { wave: WaveRow }) {
  const total = Math.max(1, wave.devices);
  const parts = [
    { n: wave.working, color: "bg-ok" },
    { n: wave.moving, color: "bg-accent" },
    { n: wave.needsYou, color: "bg-warn" },
    { n: wave.waiting, color: "bg-border-strong" },
  ];
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-3.5 py-2.5">
      <span className="w-44 truncate font-medium" title={wave.label}>
        {wave.label}
      </span>
      <span className="flex h-1.5 min-w-24 flex-1 gap-px overflow-hidden rounded-full bg-surface-3" aria-hidden>
        {parts.map((p, i) => p.n > 0 && <span key={i} className={p.color} style={{ width: `${(p.n / total) * 100}%` }} />)}
      </span>
      <span className="text-right whitespace-nowrap text-muted tabular-nums">
        {n(wave.working)} / {n(wave.devices)} working
      </span>
    </div>
  );
}

function NeedsYouCard({ total, kinds, onReview }: { total: number; kinds: NeedKind[]; onReview: () => void }) {
  return (
    <Card tone="warn" className="space-y-1 px-4 py-3.5">
      <div className="flex items-baseline gap-2 pb-1">
        <h2 className="flex-1 font-semibold text-warn">{plural(total, "device")} need you</h2>
        <span className="text-xs text-faint">grouped by reason</span>
      </div>
      <ul className="divide-y divide-warn/15">
        {kinds.map((k) => (
          <li key={`${k.stage}-${k.needsMatch}`} className="flex items-center gap-3 py-2">
            <span className="w-12 text-base font-bold tabular-nums">{n(k.count)}</span>
            <span className="min-w-0 flex-1 truncate">{k.needsMatch ? "Several fit on the new server" : STAGE_WORDS[k.stage].short}</span>
            <Button size="sm" onClick={onReview}>
              Review
            </Button>
          </li>
        ))}
      </ul>
    </Card>
  );
}

function WhereDevicesAre({ project, counts }: { project: ProjectSummary; counts: GroupCount[] | undefined }) {
  const { total, done } = projectProgress(project);
  if (total === 0) {
    return (
      <Card className="px-5 py-4 text-muted">
        No devices in the project yet. They are added in Plan.
      </Card>
    );
  }
  const shown = (counts ?? []).filter((c) => c.count > 0);
  const sum = shown.reduce((s, c) => s + c.count, 0) || 1;
  return (
    <Card className="space-y-3.5 px-5 py-4">
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="text-3xl font-bold tabular-nums">{n(done)}</span>
        <span className="text-muted">
          of {n(total)} {project.ecoverseChecked ? "moved and working" : "moved and healthy"}
        </span>
      </div>
      {shown.length > 0 && (
        <>
          <div className="flex h-2 gap-px overflow-hidden rounded-full bg-surface-3" aria-hidden>
            {shown.map((c) => (
              <span key={c.group} className={GROUP_COLOR[c.group]} style={{ width: `${(c.count / sum) * 100}%`, minWidth: 3 }} />
            ))}
          </div>
          <ul className="space-y-2">
            {shown.map((c) => (
              <li key={c.group} className="flex items-center gap-2.5">
                <span className={`size-2.5 rounded-sm ${GROUP_COLOR[c.group]}`} />
                <span className="flex-1">{GROUP_LABELS[c.group]}</span>
                <span className={`font-semibold tabular-nums ${c.group === "needsYou" ? "text-warn" : ""}`}>{n(c.count)}</span>
              </li>
            ))}
          </ul>
        </>
      )}
      {project.ecoverseWaiting > 0 && <p className="text-xs text-faint">{plural(project.ecoverseWaiting, "device")} waiting for Ecoverse's data. About an hour and a half is normal.</p>}
    </Card>
  );
}
