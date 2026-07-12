import { describe, expect, it } from "vitest";
import type { ThemeTokens } from "../../theme/theme";
import type { GithubFile } from "../../api/types";
import { buildDiffModel } from "../parse";
import { type Ctx2D, paint, type PaintParams } from "./paint";
import { ROW_HEIGHT } from "./layout";

function stubCtx() {
  const ops = { fillRect: 0, fillText: 0, setTransform: 0, total: 0 };
  const ctx = {
    fillStyle: "",
    font: "",
    textBaseline: "middle" as CanvasTextBaseline,
    globalAlpha: 1,
    fillRect: () => {
      ops.fillRect++;
      ops.total++;
    },
    clearRect: () => {
      ops.total++;
    },
    fillText: () => {
      ops.fillText++;
      ops.total++;
    },
    save: () => {
      ops.total++;
    },
    restore: () => {
      ops.total++;
    },
    setTransform: () => {
      ops.setTransform++;
      ops.total++;
    },
    beginPath: () => {
      ops.total++;
    },
    rect: () => {
      ops.total++;
    },
    clip: () => {
      ops.total++;
    },
  };
  return { ctx: ctx as unknown as Ctx2D, ops };
}

const TOKENS: ThemeTokens = {
  bg: "#000",
  fg: "#fff",
  fgMuted: "#aaa",
  accent: "#09f",
  border: "#333",
  addBg: "#031",
  addFg: "#9f9",
  delBg: "#310",
  delFg: "#f99",
  contextBg: "#000",
  contextFg: "#ccc",
  gutterBg: "#111",
  gutterFg: "#888",
  hunkBg: "#012",
  hunkFg: "#8bf",
  addEdge: "#0f0",
  delEdge: "#f00",
  addGlyph: "#0f0",
  delGlyph: "#f00",
  syntax: {
    keyword: "#f0f",
    string: "#0ff",
    number: "#ff0",
    comment: "#666",
    function: "#0f0",
    variable: "#fff",
    type: "#0ff",
    punctuation: "#ccc",
  },
};

function bigModel(totalLines: number) {
  const rows: string[] = [`@@ -1,${totalLines} +1,${totalLines} @@`];
  for (let i = 0; i < totalLines; i++) {
    rows.push(i % 3 === 0 ? `-old ${i}` : i % 3 === 1 ? `+new ${i}` : ` ctx ${i}`);
  }
  const file: GithubFile = {
    filename: "src/big.ts",
    status: "modified",
    additions: 0,
    deletions: 0,
    changes: 0,
    patch: rows.join("\n"),
  };
  return buildDiffModel([file]);
}

function params(model: ReturnType<typeof bigModel>): PaintParams {
  return {
    model,
    tokens: TOKENS,
    scrollTop: 4000,
    viewportWidth: 900,
    viewportHeight: 800,
    dpr: 2,
    rowHeight: ROW_HEIGHT,
    focusRow: 210,
    selection: { anchor: 205, head: 208 },
    fontFamily: "monospace",
    fontSize: 12,
  };
}

describe("canvas paint draw-op budget", () => {
  it("draws only the virtualized window on a 10k-line model", () => {
    const model = bigModel(10_000);
    expect(model.rows.length).toBeGreaterThan(10_000);

    const { ctx, ops } = stubCtx();
    const start = performance.now();
    paint(ctx, params(model));
    const elapsed = performance.now() - start;

    const visibleRows = Math.ceil(800 / ROW_HEIGHT) + 2 * 20;
    expect(ops.fillRect).toBeLessThan(visibleRows * 6 + 10);
    expect(ops.fillText).toBeLessThan(visibleRows * 5 + 10);
    expect(elapsed).toBeLessThan(8);
  });

  it("issues the same op count regardless of total diff size (virtualization proof)", () => {
    const small = stubCtx();
    const large = stubCtx();
    paint(small.ctx, params(bigModel(500)));
    paint(large.ctx, params(bigModel(10_000)));
    expect(small.ops.total).toBe(large.ops.total);
  });

  it("applies the dpr transform exactly once per frame", () => {
    const { ctx, ops } = stubCtx();
    paint(ctx, params(bigModel(2000)));
    expect(ops.setTransform).toBe(1);
  });
});
