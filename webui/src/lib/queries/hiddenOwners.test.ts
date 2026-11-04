import { QueryClient } from "@tanstack/svelte-query";
import { describe, expect, it, vi } from "vitest";
import type { HiddenOwners } from "@bindings/HiddenOwners";
import { path } from "@bindings/routes";
import { hiddenOwnerActions } from "./hiddenOwners";
import { qk } from "./keys";

function setup(initial: HiddenOwners = { owners: [] }) {
  const qc = new QueryClient();
  qc.setQueryData(qk.hiddenOwners, initial);
  qc.setQueryData(qk.sessions(false), { sessions: [] });
  qc.setQueryData(qk.sessions(false, true), { sessions: [] });
  const api = {
    hideOwner: vi.fn(async () => undefined),
    unhideOwner: vi.fn(async () => undefined),
  };
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  return { qc, api, invalidate, actions: hiddenOwnerActions(qc, api) };
}

const invalidatedKeys = (spy: { mock: { calls: unknown[][] } }) =>
  spy.mock.calls.map((c) => (c[0] as { queryKey: unknown }).queryKey);

describe("hiddenOwnerActions", () => {
  it("hide calls the API, adds the owner and refreshes the session lists", async () => {
    const { qc, api, invalidate, actions } = setup();
    await actions.hide("user", "u-bob", "bob");
    expect(api.hideOwner).toHaveBeenCalledWith("user", "u-bob");
    expect(qc.getQueryData<HiddenOwners>(qk.hiddenOwners)?.owners).toEqual([
      { kind: "user", target_id: "u-bob", label: "bob" },
    ]);
    expect(invalidatedKeys(invalidate)).toEqual([qk.hiddenOwners, qk.sessionsAll]);
    expect(qc.getQueryState(qk.sessions(false))?.isInvalidated).toBe(true);
    expect(qc.getQueryState(qk.sessions(false, true))?.isInvalidated).toBe(true);
  });

  it("hide is idempotent in the cache", async () => {
    const { qc, actions } = setup({ owners: [{ kind: "machine", target_id: "m1", label: "box" }] });
    await actions.hide("machine", "m1", "box");
    expect(qc.getQueryData<HiddenOwners>(qk.hiddenOwners)?.owners).toHaveLength(1);
  });

  it("unhide calls the API and drops only that owner", async () => {
    const { qc, api, invalidate, actions } = setup({
      owners: [
        { kind: "machine", target_id: "m1", label: "box" },
        { kind: "user", target_id: "m1", label: "same id, other kind" },
      ],
    });
    await actions.unhide("machine", "m1");
    expect(api.unhideOwner).toHaveBeenCalledWith("machine", "m1");
    expect(qc.getQueryData<HiddenOwners>(qk.hiddenOwners)?.owners).toEqual([
      { kind: "user", target_id: "m1", label: "same id, other kind" },
    ]);
    expect(invalidatedKeys(invalidate)).toEqual([qk.hiddenOwners, qk.sessionsAll]);
  });

  it("leaves the cache alone when the server refuses", async () => {
    const { qc, api, invalidate, actions } = setup();
    api.hideOwner.mockRejectedValueOnce(new Error("400 cannot hide yourself"));
    await expect(actions.hide("user", "u-me", "me")).rejects.toThrow("cannot hide yourself");
    expect(qc.getQueryData<HiddenOwners>(qk.hiddenOwners)?.owners).toEqual([]);
    expect(invalidate).not.toHaveBeenCalled();
  });
});

describe("hidden-owner query keys", () => {
  it("keeps the plain session keys unchanged and nests the show-hidden ones under them", () => {
    expect(qk.sessions(false)).toEqual(["sessions", { archived: false }]);
    expect(qk.sessions(false, true)).toEqual(["sessions", { archived: false, showHidden: true }]);
    expect(qk.sessionSearch("q", false)).toEqual(["sessions", "search", { q: "q", archived: false }]);
    expect(qk.sessionSearch("q", false, true)).toEqual([
      "sessions",
      "search",
      { q: "q", archived: false, showHidden: true },
    ]);
  });

  it("builds the hidden-owner routes from the generated table", () => {
    expect(path("get_me_hidden_owners")).toBe("/me/hidden-owners");
    expect(path("put_me_hidden_owners_by_kind_by_target", { kind: "user", target_id: "u1" })).toBe(
      "/me/hidden-owners/user/u1",
    );
    expect(path("delete_me_hidden_owners_by_kind_by_target", { kind: "machine", target_id: "m1" })).toBe(
      "/me/hidden-owners/machine/m1",
    );
  });
});
