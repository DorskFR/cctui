export type PrContentTab = "description" | "comments" | "commits" | "diff";

export const PR_CONTENT_TABS: readonly PrContentTab[] = [
  "description",
  "commits",
  "comments",
  "diff",
] as const;

export const PR_CONTENT_TAB_LABELS: Record<PrContentTab, string> = {
  description: "Description",
  commits: "Commits",
  comments: "Comments",
  diff: "Diff",
};

export function defaultPrTab(): PrContentTab {
  return "diff";
}

export function isPrContentTab(value: unknown): value is PrContentTab {
  return typeof value === "string" && (PR_CONTENT_TABS as readonly string[]).includes(value);
}

export function prTabStorageKey(owner: string, repo: string, number: number): string {
  return `ghreview:prtab:${owner}/${repo}/${number}`;
}

export function deserializePrTab(raw: string | null): PrContentTab {
  if (raw === "conversation" || raw === "activity") return "comments";
  if (raw === "checks") return "diff";
  return isPrContentTab(raw) ? raw : defaultPrTab();
}
