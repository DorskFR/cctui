import { describe, expect, it } from "vitest";
import { qk } from "./keys";
import { shouldPersistQuery } from "./persist";

const ok = (queryKey: readonly unknown[], status = "success") => ({
  queryKey,
  state: { status },
});

describe("shouldPersistQuery", () => {
  it("persists the session lists, conversations and labels", () => {
    expect(shouldPersistQuery(ok(qk.sessions(false)))).toBe(true);
    expect(shouldPersistQuery(ok(qk.sessions(true)))).toBe(true);
    expect(shouldPersistQuery(ok(qk.conversation("s1")))).toBe(true);
    expect(shouldPersistQuery(ok(["labels"]))).toBe(true);
  });

  it("skips search and archive search pages", () => {
    expect(shouldPersistQuery(ok(qk.sessionSearch("foo", false)))).toBe(false);
    expect(shouldPersistQuery(ok(qk.sessionSearch("", true)))).toBe(false);
  });

  it("skips unlisted roots and unsuccessful queries", () => {
    expect(shouldPersistQuery(ok(qk.sessionContext("s1")))).toBe(false);
    expect(shouldPersistQuery(ok(qk.sessions(false), "error"))).toBe(false);
    expect(shouldPersistQuery(ok(qk.sessions(false), "pending"))).toBe(false);
  });
});
