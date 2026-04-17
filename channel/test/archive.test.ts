import { describe, it, expect } from "bun:test";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { computeFileSha256, walkProjectDirs } from "../src/archive";

describe("archive", () => {
  it("walks projects dir and returns jsonl ProjectFile entries", () => {
    const root = mkdtempSync(join(tmpdir(), "cctui-arch-"));
    const p1 = join(root, "-home-user-foo");
    mkdirSync(p1, { recursive: true });
    writeFileSync(join(p1, "abc-123.jsonl"), '{"x":1}\n');
    writeFileSync(join(p1, "README.txt"), "ignore me");
    const p2 = join(root, "-home-user-bar");
    mkdirSync(p2);
    writeFileSync(join(p2, "def-456.jsonl"), '{"y":2}\n');

    const files = walkProjectDirs(root);
    const rels = files.map((f) => `${f.projectDir}/${f.sessionId}`).sort();
    expect(rels).toEqual([
      "-home-user-bar/def-456",
      "-home-user-foo/abc-123",
    ]);
  });

  it("computes stable sha256", async () => {
    const root = mkdtempSync(join(tmpdir(), "cctui-arch-"));
    const path = join(root, "f.jsonl");
    writeFileSync(path, "hello\n");
    // sha256("hello\n") = 5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03
    expect(await computeFileSha256(path)).toBe(
      "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03",
    );
  });

  it("returns [] for missing projects root", () => {
    expect(walkProjectDirs("/definitely/does/not/exist/cctui")).toEqual([]);
  });
});
