import type { MachineResourcesRow } from "@bindings/MachineResourcesRow";
import type { MachineResources } from "@bindings/MachineResources";
import { statTone, worstTone as worstOf, type StatTone } from "$lib/stats/tone";

export type ResourceTone = StatTone;

export function resourceTone(pct: number | null | undefined): ResourceTone {
  return statTone("resource", pct);
}

/** The tone the machine's single dot takes on a narrow screen: its worst bar,
 *  `unknown` only when nothing is known at all. */
export function worstTone(
  r: MachineResources | null | undefined,
): ResourceTone {
  if (!r) return "unknown";
  return worstOf([
    resourceTone(r.cpu_pct),
    resourceTone(r.mem_pct),
    resourceTone(r.disk_pct),
  ]);
}

/** Rounded 0..100 for a bar width, null when unknown. */
export function pctOf(v: number | null | undefined): number | null {
  if (v === null || v === undefined || !Number.isFinite(v)) return null;
  return Math.max(0, Math.min(100, Math.round(v)));
}

/** What the strip calls the machine: its operator display name, else its
 *  enrolled hostname. */
export function machineLabel(
  row: Pick<MachineResourcesRow, "name" | "display_name">,
): string {
  const d = row.display_name?.trim();
  return d && d.length > 0 ? d : row.name;
}

/** `1.5 GB`, `512 MB` — for the tooltip only, never the strip. */
export function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "?";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${i === 0 ? v : v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`;
}

/** The rows the header shows: the ticked machines, in the order they were
 *  ticked, skipping ids that no longer exist (revoked, deleted). */
export function pinnedRows(
  rows: MachineResourcesRow[] | undefined,
  pinned: string[],
): MachineResourcesRow[] {
  if (!rows || pinned.length === 0) return [];
  const byId = new Map(rows.map((r) => [r.machine_id, r]));
  return pinned
    .map((id) => byId.get(id))
    .filter((r): r is MachineResourcesRow => !!r);
}

/** Apply a live ws snapshot onto the cached list, leaving other rows alone. */
export function patchRow(
  rows: MachineResourcesRow[] | undefined,
  machineId: string,
  resources: MachineResources,
  at: string,
): MachineResourcesRow[] | undefined {
  if (!rows) return rows;
  return rows.map((r) =>
    r.machine_id === machineId
      ? { ...r, resources, updated_at: at, liveness: "online" }
      : r,
  );
}

/** A snapshot older than this is shown hatched: the daemon went quiet. */
export const STALE_AFTER_MS = 3 * 60_000;

export function isStale(
  updatedAt: string | null | undefined,
  now: number,
): boolean {
  if (!updatedAt) return true;
  const t = Date.parse(updatedAt);
  return !Number.isFinite(t) || now - t > STALE_AFTER_MS;
}
