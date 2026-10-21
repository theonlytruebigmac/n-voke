import type { CleanupDevice } from "./bindings";

export interface WaveGroup {
  wave: number | null;
  label: string;
  ids: string[];
}

/** The ready devices grouped by wave, in wave order, with those in no wave last. */
export function waveGroups(devices: CleanupDevice[]): WaveGroup[] {
  const groups = new Map<number | null, WaveGroup>();
  for (const d of devices) {
    const group = groups.get(d.wave) ?? { wave: d.wave, label: d.wave === null ? "No wave" : (d.waveLabel ?? `Wave ${d.wave}`), ids: [] };
    group.ids.push(d.deviceId);
    groups.set(d.wave, group);
  }
  return [...groups.values()].sort((a, b) => (a.wave ?? Infinity) - (b.wave ?? Infinity));
}

/** What to send: null when every ready device is ticked (the core then takes whatever is ready), else the ticked IDs. */
export const cleanupScope = (devices: CleanupDevice[], picked: ReadonlySet<string>): string[] | null =>
  devices.every((d) => picked.has(d.deviceId)) ? null : devices.filter((d) => picked.has(d.deviceId)).map((d) => d.deviceId);
