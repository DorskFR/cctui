import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { contrastRatio } from "./contrast";
import { THEMES } from "./theme";

const CSS = readFileSync(resolve(process.cwd(), "src/app.css"), "utf8");

function blockFor(theme: string): Record<string, string> {
  const re = new RegExp(`\\[data-theme="${theme}"\\][^{]*\\{([^}]*)\\}`);
  const body = CSS.match(re)?.[1];
  if (!body) throw new Error(`no block for theme ${theme}`);
  const vars: Record<string, string> = {};
  for (const m of body.matchAll(/(--[\w-]+)\s*:\s*(#[0-9a-fA-F]{3,8})\s*;/g)) {
    vars[m[1]] = m[2];
  }
  return vars;
}

const AA = 4.5;

const TEXT_PAIRS: Array<[string, string]> = [
  ["--gh-fg", "--gh-bg"],
  ["--gh-fg", "--gh-bg-elev"],
  ["--gh-fg", "--gh-bg-inset"],
  ["--gh-fg-muted", "--gh-bg"],
  ["--gh-fg-muted", "--gh-bg-elev"],
  ["--gh-fg-subtle", "--gh-bg"],
  ["--gh-accent-fg", "--gh-bg"],
  ["--gh-accent-fg", "--gh-bg-elev"],
];

const DIFF_PAIRS: Array<[string, string]> = [
  ["--gh-diff-add-fg", "--gh-diff-add-bg"],
  ["--gh-diff-del-fg", "--gh-diff-del-bg"],
  ["--gh-diff-context-fg", "--gh-diff-context-bg"],
  ["--gh-diff-gutter-fg", "--gh-diff-gutter-bg"],
  ["--gh-diff-hunk-fg", "--gh-diff-hunk-bg"],
];

describe("WCAG AA contrast across all themes", () => {
  for (const theme of THEMES) {
    const vars = blockFor(theme);
    for (const [fg, bg] of [...TEXT_PAIRS, ...DIFF_PAIRS]) {
      it(`${theme}: ${fg} on ${bg} ≥ ${AA}:1`, () => {
        const fgHex = vars[fg];
        const bgHex = vars[bg];
        expect(fgHex, `${fg} missing in ${theme}`).toBeDefined();
        expect(bgHex, `${bg} missing in ${theme}`).toBeDefined();
        expect(contrastRatio(fgHex, bgHex)).toBeGreaterThanOrEqual(AA);
      });
    }
  }
});
