import type { PrivacyScanJob } from "@bindings/PrivacyScanJob";
import type { ScanCategory } from "@bindings/ScanCategory";

export type RescrubScope = "all" | "30d" | "7d";

const DAYS: Record<RescrubScope, number | null> = { all: null, "30d": 30, "7d": 7 };

/** `since` for a scope, as the RFC3339 string the endpoint expects. */
export function rescrubScopeSince(
  scope: RescrubScope,
  now: Date = new Date(),
): string | null {
  const days = DAYS[scope];
  if (days === null) return null;
  return new Date(now.getTime() - days * 86_400_000).toISOString();
}

export function rescrubCategoryRows(job: PrivacyScanJob | null): ScanCategory[] {
  return job ? job.categories : [];
}

export function rescrubIsRunning(job: PrivacyScanJob | null): boolean {
  return job?.status === "running";
}

/** Determinate while the row estimate is known and still ahead of the cursor;
 *  the count is a snapshot, so a scan that overruns it falls back to
 *  indeterminate rather than rendering a bar past 100%. */
export function rescrubProgress(
  job: PrivacyScanJob | null,
): { value: number; max: number } | null {
  if (!job?.rows_total || job.rows_total < job.rows_scanned) return null;
  return { value: job.rows_scanned, max: job.rows_total };
}

/** Categories whose matches look like identifiers rather than credentials —
 *  the `\w+_token` trap, worth a warning before an irreversible apply. */
export function rescrubIdentifierWarnings(job: PrivacyScanJob | null): string[] {
  return (job?.categories ?? []).filter((c) => c.identifier_warning).map((c) => c.category);
}

export function rescrubSampleLines(category: ScanCategory): string[] {
  return category.samples.map((s) => `${s.text}   ${s.context}`);
}
