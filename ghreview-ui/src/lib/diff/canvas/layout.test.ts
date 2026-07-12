import { describe, expect, it } from "vitest";
import type { GithubFile } from "../../api/types";
import { buildNavIndex } from "../navindex";
import { buildDiffModel } from "../parse";
import { computeWindow } from "../virtual";
import {
  anchorScreenY,
  clampScroll,
  hitTest,
  regionAtX,
  ROW_HEIGHT,
  rowAtY,
  rowTop,
  scrollToRow,
} from "./layout";

function file(lines: number): GithubFile {
  const rows: string[] = [`@@ -1,${lines} +1,${lines} @@`];
  for (let i = 0; i < lines; i++) {
    rows.push(i % 3 === 0 ? `-old ${i}` : i % 3 === 1 ? `+new ${i}` : ` ctx ${i}`);
  }
  return {
    filename: "src/a.ts",
    status: "modified",
    additions: 0,
    deletions: 0,
    changes: 0,
    patch: rows.join("\n"),
  };
}

describe("row geometry", () => {
  it("rowTop is a linear multiple of the shared row height", () => {
    expect(rowTop(0)).toBe(0);
    expect(rowTop(5)).toBe(5 * ROW_HEIGHT);
  });

  it("rowAtY inverts rowTop and rejects out-of-range", () => {
    expect(rowAtY(0, 10)).toBe(0);
    expect(rowAtY(ROW_HEIGHT * 3 + 4, 10)).toBe(3);
    expect(rowAtY(-1, 10)).toBe(-1);
    expect(rowAtY(ROW_HEIGHT * 10, 10)).toBe(-1);
  });
});

describe("row-layout parity with the DOM renderer", () => {
  it("canvas rowTop equals the DOM scroll math (rowIndex * ROW_HEIGHT) for nav targets", () => {
    const model = buildDiffModel([file(200)]);
    const nav = buildNavIndex(model);
    for (const t of [...nav.files, ...nav.hunks]) {
      expect(rowTop(t.rowIndex)).toBe(t.rowIndex * ROW_HEIGHT);
    }
  });

  it("shares the windowing math so canvas paints the same visible slice as DOM", () => {
    const model = buildDiffModel([file(500)]);
    const win = computeWindow(1000, 600, ROW_HEIGHT, model.rows.length, 20);
    expect(win.start).toBeLessThanOrEqual(rowAtY(1000, model.rows.length));
    expect(win.end).toBeGreaterThan(rowAtY(1000 + 600, model.rows.length));
  });
});

describe("hit-testing (pixel → row/hunk/file)", () => {
  const model = buildDiffModel([file(50)]);

  it("maps x to gutter/marker/code columns", () => {
    expect(regionAtX(10)).toBe("oldGutter");
    expect(regionAtX(60)).toBe("newGutter");
    expect(regionAtX(100)).toBe("marker");
    expect(regionAtX(200)).toBe("code");
  });

  it("resolves a viewport pixel to a row plus its file/hunk indices", () => {
    const scrollTop = 4 * ROW_HEIGHT;
    const hit = hitTest(model, 200, ROW_HEIGHT * 2 + 3, scrollTop);
    expect(hit).not.toBeNull();
    expect(hit?.rowIndex).toBe(6);
    expect(hit?.region).toBe("code");
    expect(hit?.fileIndex).toBe(model.rows[6].fileIndex);
    expect(hit?.hunkIndex).toBe(model.rows[6].hunkIndex);
  });

  it("returns null past the end of the model", () => {
    expect(hitTest(model, 10, 10, 1_000_000)).toBeNull();
  });
});

describe("overlay anchor positioning", () => {
  it("subtracts scroll so an anchor tracks its row through scrolling", () => {
    expect(anchorScreenY(10, 0)).toBe(10 * ROW_HEIGHT);
    expect(anchorScreenY(10, 5 * ROW_HEIGHT)).toBe(5 * ROW_HEIGHT);
  });

  it("scales with row height so anchors survive zoom", () => {
    expect(anchorScreenY(10, 0, 20)).toBe(200);
    expect(anchorScreenY(10, 0, 30)).toBe(300);
  });

  it("is independent of total diff size", () => {
    expect(anchorScreenY(100, 40, 20)).toBe(anchorScreenY(100, 40, 20));
  });
});

describe("scroll clamping and reveal", () => {
  const model = buildDiffModel([file(200)]);

  it("clamps to [0, maxScroll]", () => {
    expect(clampScroll(-50, model, 600)).toBe(0);
    const max = model.rows.length * ROW_HEIGHT - 600;
    expect(clampScroll(1_000_000, model, 600)).toBe(max);
  });

  it("scrolls a below-fold row into view one third down", () => {
    const target = 100;
    const next = scrollToRow(target, 0, 600, model);
    expect(next).toBeGreaterThan(0);
    expect(next).toBeLessThanOrEqual(rowTop(target));
  });

  it("leaves scroll untouched when the row is already visible", () => {
    expect(scrollToRow(5, 0, 600, model)).toBe(0);
  });
});
