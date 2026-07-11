import { describe, expect, it } from "vitest";
import type { GithubFile } from "../api/types";
import { buildNavIndex } from "./navindex";
import { buildDiffModel } from "./parse";
import { computeWindow } from "./virtual";

function bigFile(index: number, lines: number): GithubFile {
  const rows: string[] = [`@@ -1,${lines} +1,${lines} @@`];
  for (let i = 0; i < lines; i++) {
    rows.push(i % 3 === 0 ? `-old ${i}` : i % 3 === 1 ? `+new ${i}` : ` ctx ${i}`);
  }
  return {
    filename: `src/file-${index}.ts`,
    status: "modified",
    additions: 0,
    deletions: 0,
    changes: 0,
    patch: rows.join("\n"),
  };
}

describe("open-path performance", () => {
  it("parses + indexes a 50-file / ~10k-line working set well under the 100ms budget", () => {
    const files = Array.from({ length: 50 }, (_, i) => bigFile(i, 200));

    const start = performance.now();
    const model = buildDiffModel(files);
    const nav = buildNavIndex(model);
    const elapsed = performance.now() - start;

    expect(model.rows.length).toBeGreaterThan(10_000);
    expect(nav.files).toHaveLength(50);
    expect(elapsed).toBeLessThan(100);
  });

  it("windows a 10k-row diff to a small visible slice in O(1)", () => {
    const win = computeWindow(50_000, 800, 20, 10_000, 40);
    expect(win.end - win.start).toBeLessThan(200);
    expect(win.totalHeight).toBe(200_000);
  });
});
