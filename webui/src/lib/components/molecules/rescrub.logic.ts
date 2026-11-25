import type { LocalTranscriptsScan } from "@bindings/LocalTranscriptsScan";
import type { MachineTranscriptScan } from "@bindings/MachineTranscriptScan";
import type { NotScannedReason } from "@bindings/NotScannedReason";
import type { PrivacyScanJob } from "@bindings/PrivacyScanJob";
import type { ScanCategory } from "@bindings/ScanCategory";
import type { TranscriptAuditReport } from "@bindings/TranscriptAuditReport";

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

export type RescrubNotice = "failed" | "done" | null;

/** A finished job only notifies the panel that watched it run: the server keeps
 *  the last job, so a fresh mount would otherwise replay its toast. */
export function rescrubSettleNotice(
  done: PrivacyScanJob,
  watchedId: string | null,
): RescrubNotice {
  if (done.id !== watchedId) return null;
  if (done.status === "failed") return "failed";
  if (!done.dry_run && done.status === "completed") return "done";
  return null;
}

export function rescrubLocal(job: PrivacyScanJob | null): LocalTranscriptsScan | null {
  return job?.local_transcripts ?? null;
}

/** Stored messages or local transcripts: either has something to mask. */
export function rescrubHasFindings(job: PrivacyScanJob): boolean {
  return job.substitutions > 0 || (job.local_transcripts?.substitutions ?? 0) > 0;
}

export type RescrubMachineNote =
  | { kind: "open_unknown" }
  | { kind: "left" | "unfit" | "read_only" | "live" | "busy"; count: number };

export type RescrubMachineLine =
  | { kind: "pending" }
  | {
      kind: "scanned";
      files: number;
      values: number;
      changed: number;
      dryRun: boolean;
      notes: RescrubMachineNote[];
    }
  | { kind: "not_scanned"; reason: NotScannedReason; detail: string | null };

/** What stays on disk after the pass, so `values` never claims more than was
 *  masked. */
function machineNotes(r: TranscriptAuditReport): RescrubMachineNote[] {
  const notes: RescrubMachineNote[] = r.open_files_unknown ? [{ kind: "open_unknown" }] : [];
  const counts = [
    ["left", r.values_left],
    ["unfit", r.lines_unfit],
    ["read_only", r.files_read_only],
    ["live", r.files_live],
    ["busy", r.files_busy],
  ] as const;
  for (const [kind, count] of counts) {
    if (count > 0) notes.push({ kind, count });
  }
  return notes;
}

/** A machine without a report is never shown as clean: it is pending or
 *  carries the reason it was not scanned. */
export function rescrubMachineLine(machine: MachineTranscriptScan): RescrubMachineLine {
  const r = machine.report;
  if (machine.state === "scanned" && r) {
    return {
      kind: "scanned",
      files: r.files_scanned,
      values: r.substitutions,
      changed: r.files_changed,
      dryRun: r.dry_run,
      notes: machineNotes(r),
    };
  }
  if (machine.state === "pending") return { kind: "pending" };
  return { kind: "not_scanned", reason: machine.reason ?? "failed", detail: machine.detail };
}
