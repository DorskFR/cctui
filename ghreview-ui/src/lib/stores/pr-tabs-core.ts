export type PrContentTab = "description" | "conversation" | "commits" | "checks" | "diff";

export const PR_CONTENT_TABS: readonly PrContentTab[] = [
  "description",
  "conversation",
  "commits",
  "checks",
  "diff",
] as const;

export const PR_CONTENT_TAB_LABELS: Record<PrContentTab, string> = {
  description: "Description",
  conversation: "Conversation",
  commits: "Commits",
  checks: "Checks",
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
  return isPrContentTab(raw) ? raw : defaultPrTab();
}
