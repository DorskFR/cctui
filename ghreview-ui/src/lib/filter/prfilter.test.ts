import { describe, expect, it } from "vitest";
import type { GithubPull } from "../api/types";
import {
  collectAuthors,
  collectLabels,
  collectRepos,
  emptyCriteria,
  filterEntries,
  type PrEntry,
  type PrFilterCriteria,
} from "./prfilter";

function pull(p: Partial<GithubPull>): GithubPull {
  return { number: 1, title: "t", state: "open", ...p } as GithubPull;
}

const entries: PrEntry[] = [
  {
    owner: "acme",
    repo: "web",
    pull: pull({
      number: 1,
      title: "Fix login bug",
      state: "open",
      user: { login: "alice" },
      requested_reviewers: [{ login: "me" }],
      labels: [{ name: "bug" }],
    }),
  },
  {
    owner: "acme",
    repo: "api",
    pull: pull({
      number: 2,
      title: "Add search",
      state: "closed",
      merged: true,
      user: { login: "me" },
      labels: [{ name: "feature" }],
    }),
  },
  {
    owner: "other",
    repo: "cli",
    pull: pull({
      number: 3,
      title: "Draft docs",
      state: "open",
      draft: true,
      user: { login: "bob" },
    }),
  },
];

function crit(over: Partial<PrFilterCriteria>): PrFilterCriteria {
  return { ...emptyCriteria, ...over };
}

describe("filterEntries", () => {
  it("returns everything by default (All)", () => {
    expect(filterEntries(entries, emptyCriteria, "me")).toHaveLength(3);
  });

  it("text matches title, author, repo, number and label", () => {
    expect(filterEntries(entries, crit({ text: "login" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ text: "alice" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ text: "cli" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ text: "#2" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ text: "feature" }), "me")).toHaveLength(1);
  });

  it("relation review / authored use the account", () => {
    expect(filterEntries(entries, crit({ relation: "review" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ relation: "authored" }), "me")).toHaveLength(1);
  });

  it("state filter maps merged/draft/open/closed", () => {
    expect(filterEntries(entries, crit({ state: "merged" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ state: "draft" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ state: "open" }), "me")).toHaveLength(1);
  });

  it("author, repo and label filters", () => {
    expect(filterEntries(entries, crit({ author: "bob" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ repo: "acme/api" }), "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ label: "bug" }), "me")).toHaveLength(1);
  });

  it("criteria compose", () => {
    const c = crit({ repo: "acme/web", state: "open", label: "bug", text: "fix" });
    expect(filterEntries(entries, c, "me")).toHaveLength(1);
    expect(filterEntries(entries, crit({ author: "alice", state: "merged" }), "me")).toHaveLength(0);
  });
});

describe("collectors", () => {
  it("collect repos, authors, labels sorted and deduped", () => {
    expect(collectRepos(entries)).toEqual(["acme/api", "acme/web", "other/cli"]);
    expect(collectAuthors(entries)).toEqual(["alice", "bob", "me"]);
    expect(collectLabels(entries)).toEqual(["bug", "feature"]);
  });
});
