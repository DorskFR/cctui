import { compilePredicate, parse, type Schema } from "@dorsk/tsumikit";
import { type GithubPull, prStateOf } from "../api/types";

export type PrRelation = "all" | "review" | "authored";

export interface PrEntry {
  owner: string;
  repo: string;
  pull: GithubPull;
}

export function repoKey(e: PrEntry): string {
  return `${e.owner}/${e.repo}`;
}

export function labelNames(pull: GithubPull): string[] {
  return (pull.labels ?? []).map((l) => l.name);
}

export function prRow(e: PrEntry): Record<string, unknown> {
  return {
    title: e.pull.title,
    author: e.pull.user?.login ?? "",
    repo: repoKey(e),
    state: prStateOf(e.pull),
    label: labelNames(e.pull).join(" "),
    num: `#${e.pull.number}`,
  };
}

export function buildPrSchema(repos: string[], authors: string[], labels: string[]): Schema {
  const opts = (vs: string[]) => vs.map((v) => ({ value: v, label: v }));
  return {
    fields: [
      {
        name: "title",
        label: "Title",
        type: "string",
        operators: ["contains", "not_contains"],
      },
      {
        name: "repo",
        label: "Repository",
        type: "enum",
        operators: ["eq", "ne", "in"],
        options: opts(repos),
      },
      {
        name: "author",
        label: "Author",
        type: "enum",
        aliases: ["by"],
        operators: ["eq", "ne", "in"],
        options: opts(authors),
      },
      {
        name: "state",
        label: "State",
        type: "enum",
        operators: ["eq", "ne", "in"],
        options: opts(["open", "draft", "merged", "closed"]),
      },
      {
        name: "label",
        label: "Label",
        type: "string",
        operators: ["contains", "not_contains"],
        options: opts(labels),
      },
    ],
  };
}

function matchesRelation(e: PrEntry, relation: PrRelation, account: string): boolean {
  if (relation === "authored") return e.pull.user?.login === account;
  if (relation === "review") {
    return (e.pull.requested_reviewers ?? []).some((u) => u.login === account);
  }
  return true;
}

export function filterPrs(
  entries: PrEntry[],
  query: string,
  schema: Schema,
  relation: PrRelation,
  account: string,
): PrEntry[] {
  const predicate = compilePredicate(parse(query, schema));
  return entries.filter((e) => matchesRelation(e, relation, account) && predicate(prRow(e)));
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

export function groupByRepo(entries: PrEntry[]): { repo: string; entries: PrEntry[] }[] {
  const groups = new Map<string, PrEntry[]>();
  for (const e of entries) {
    const key = repoKey(e);
    const list = groups.get(key);
    if (list) list.push(e);
    else groups.set(key, [e]);
  }
  return [...groups.entries()]
    .sort((a, b) => a[0].localeCompare(b[0]))
    .map(([repo, list]) => ({
      repo,
      entries: list.sort((a, b) => b.pull.number - a.pull.number),
    }));
}
