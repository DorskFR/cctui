import { type GithubPull, prStateOf } from "../api/types";

export type PrRelation = "all" | "review" | "authored";
export type PrStateFilter = "all" | "open" | "draft" | "merged" | "closed";

export interface PrEntry {
  owner: string;
  repo: string;
  pull: GithubPull;
}

export interface PrFilterCriteria {
  text: string;
  relation: PrRelation;
  author: string;
  repo: string;
  state: PrStateFilter;
  label: string;
}

export const emptyCriteria: PrFilterCriteria = {
  text: "",
  relation: "all",
  author: "",
  repo: "",
  state: "all",
  label: "",
};

export function repoKey(e: PrEntry): string {
  return `${e.owner}/${e.repo}`;
}

export function labelNames(pull: GithubPull): string[] {
  return (pull.labels ?? []).map((l) => l.name);
}

function matchesText(e: PrEntry, needle: string): boolean {
  const q = needle.trim().toLowerCase();
  if (!q) return true;
  const hay = [
    e.pull.title,
    e.pull.user?.login ?? "",
    repoKey(e),
    `#${e.pull.number}`,
    ...labelNames(e.pull),
  ]
    .join(" ")
    .toLowerCase();
  return hay.includes(q);
}

function matchesRelation(e: PrEntry, relation: PrRelation, account: string): boolean {
  if (relation === "authored") return e.pull.user?.login === account;
  if (relation === "review") {
    return (e.pull.requested_reviewers ?? []).some((u) => u.login === account);
  }
  return true;
}

export function filterEntries(
  entries: PrEntry[],
  criteria: PrFilterCriteria,
  account: string,
): PrEntry[] {
  return entries.filter((e) => {
    if (criteria.repo && repoKey(e) !== criteria.repo) return false;
    if (criteria.author && e.pull.user?.login !== criteria.author) return false;
    if (criteria.state !== "all" && prStateOf(e.pull) !== criteria.state) return false;
    if (criteria.label && !labelNames(e.pull).includes(criteria.label)) return false;
    if (!matchesRelation(e, criteria.relation, account)) return false;
    if (!matchesText(e, criteria.text)) return false;
    return true;
  });
}

export function collectRepos(entries: PrEntry[]): string[] {
  return [...new Set(entries.map(repoKey))].sort();
}

export function collectAuthors(entries: PrEntry[]): string[] {
  const set = new Set<string>();
  for (const e of entries) {
    const login = e.pull.user?.login;
    if (login) set.add(login);
  }
  return [...set].sort((a, b) => a.localeCompare(b));
}

export function collectLabels(entries: PrEntry[]): string[] {
  const set = new Set<string>();
  for (const e of entries) for (const n of labelNames(e.pull)) set.add(n);
  return [...set].sort((a, b) => a.localeCompare(b));
}
