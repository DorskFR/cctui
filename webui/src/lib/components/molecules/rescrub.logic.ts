import type { RescrubReport } from "@bindings/RescrubReport";

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

/** `by_category` as table rows, biggest first, so the preview leads with
 *  whatever leaked most. */
export function rescrubCategoryRows(
  report: RescrubReport | null,
): { category: string; count: number }[] {
  if (!report) return [];
  return Object.entries(report.by_category)
    .map(([category, count]) => ({ category, count }))
    .sort((a, b) => b.count - a.count || a.category.localeCompare(b.category));
}
