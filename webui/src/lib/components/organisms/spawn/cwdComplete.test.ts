import { beforeEach, describe, expect, it, vi } from "vitest";

let dirs: string[] = [];
let calls: Array<[string, string]> = [];
let fail = false;

vi.mock("$lib/queries", () => ({
  endpoints: {
    machineDirs: async (id: string, parent: string) => {
      calls.push([id, parent]);
      if (fail) throw new Error("boom");
      return { dirs };
    },
  },
}));

const { cwdSuggestions } = await import("./cwdComplete");

beforeEach(() => {
  dirs = [];
  calls = [];
  fail = false;
});

describe("cwdSuggestions", () => {
  it("offers the recent dirs while the field is empty, without listing", async () => {
    expect(await cwdSuggestions("m1", "", ["/a", "/b", "/a"])).toEqual([
      "/a",
      "/b",
    ]);
    expect(calls).toEqual([]);
  });

  it("lists the parent once a slash is typed and filters by the leaf prefix", async () => {
    dirs = ["Documents", "Downloads", "Music"];
    expect(await cwdSuggestions("m1", "/home/dorsk/Do", [])).toEqual([
      "/home/dorsk/Documents",
      "/home/dorsk/Downloads",
    ]);
    expect(calls).toEqual([["m1", "/home/dorsk"]]);
  });

  it("lists the root without doubling its slash", async () => {
    dirs = ["srv"];
    expect(await cwdSuggestions("m1", "/s", [])).toEqual(["/srv"]);
    expect(calls).toEqual([["m1", "/"]]);
  });

  it("hides dotfiles unless the leaf prefix asks for them", async () => {
    dirs = [".config", "code"];
    expect(await cwdSuggestions("m1", "/home/", [])).toEqual(["/home/code"]);
    expect(await cwdSuggestions("m1", "/home/.", [])).toEqual([
      "/home/.config",
    ]);
  });

  it("returns nothing without a machine, and never requests one", async () => {
    expect(await cwdSuggestions("", "/home/d", ["/a"])).toEqual([]);
    expect(calls).toEqual([]);
  });

  it("swallows a listing error so typing keeps working", async () => {
    fail = true;
    expect(await cwdSuggestions("m1", "/home/d", [])).toEqual([]);
  });
});
