import { describe, expect, it } from "vitest";
import type { GithubPull } from "../api/types";
import {
  buildPrSchema,
  collectAuthors,
  collectLabels,
  collectRepos,
  filterPrs,
  groupByRepo,
  type PrEntry,
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

const schema = buildPrSchema(
  collectRepos(entries),
  collectAuthors(entries),
  collectLabels(entries),
);

function run(query: string, relation: "all" | "review" | "authored" = "all"): PrEntry[] {
  return filterPrs(entries, query, schema, relation, "me");
}

describe("filterPrs", () => {
  it("returns everything by default (All)", () => {
    expect(run("")).toHaveLength(3);
  });

  it("free text matches title, author, repo, number and label", () => {
    expect(run("login")).toHaveLength(1);
    expect(run("alice")).toHaveLength(1);
    expect(run("cli")).toHaveLength(1);
    expect(run("#2")).toHaveLength(1);
    expect(run("feature")).toHaveLength(1);
  });

  it("relation review / authored use the account", () => {
    expect(run("", "review")).toHaveLength(1);
    expect(run("", "authored")).toHaveLength(1);
  });

  it("state field maps merged/draft/open/closed", () => {
    expect(run("state:merged")).toHaveLength(1);
    expect(run("state:draft")).toHaveLength(1);
    expect(run("state:open")).toHaveLength(1);
  });

  it("author, repo and label fields", () => {
    expect(run("author:bob")).toHaveLength(1);
    expect(run("repo:acme/api")).toHaveLength(1);
    expect(run("label:bug")).toHaveLength(1);
  });

  it("clauses compose", () => {
    expect(run("repo:acme/web state:open label:bug fix")).toHaveLength(1);
    expect(run("author:alice state:merged")).toHaveLength(0);
  });
});

describe("collectors", () => {
  it("collect repos, authors, labels sorted and deduped", () => {
    expect(collectRepos(entries)).toEqual(["acme/api", "acme/web", "other/cli"]);
    expect(collectAuthors(entries)).toEqual(["alice", "bob", "me"]);
    expect(collectLabels(entries)).toEqual(["bug", "feature"]);
  });
});

describe("groupByRepo", () => {
  it("groups entries under sorted repo headers", () => {
    const groups = groupByRepo(entries);
    expect(groups.map((g) => g.repo)).toEqual(["acme/api", "acme/web", "other/cli"]);
    expect(groups[0].entries).toHaveLength(1);
  });
});
