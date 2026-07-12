import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { hexToRgb, type Rgb } from "./contrast";
import { THEMES } from "./theme";

const TSUMIKIT_VARS = readFileSync(
  resolve(process.cwd(), "node_modules/@dorsk/tsumikit/dist/styles/variables.css"),
  "utf8",
);
const TOKENS = readFileSync(resolve(process.cwd(), "src/tokens.css"), "utf8");

function block(selector: string): Record<string, string> {
  const esc = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = TSUMIKIT_VARS.match(new RegExp(`${esc}\\s*\\{([^}]*)\\}`))?.[1] ?? "";
  const out: Record<string, string> = {};
  for (const m of body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out[m[1]] = m[2].trim();
  return out;
}

const ROOT = block(":root");

function palette(theme: string): Record<string, string> {
  const t = theme === "dark" ? {} : block(`[data-theme="${theme}"]`);
  const get = (name: string) => t[name] ?? ROOT[name];
  return {
    bg: get("--c-bg"),
    green: get("--c-green"),
    red: get("--c-red"),
  };
}

function mix(x: string, pct: number, y: string): Rgb {
  const a = hexToRgb(x);
  const b = hexToRgb(y);
  const p = pct / 100;
  const c = (k: keyof Rgb) => Math.round(a[k] * p + b[k] * (1 - p));
  return { r: c("r"), g: c("g"), b: c("b") };
}

function dist(a: Rgb, b: Rgb): number {
  return Math.abs(a.r - b.r) + Math.abs(a.g - b.g) + Math.abs(a.b - b.b);
}

describe("diff colors derive from and track every tsumikit theme", () => {
  for (const theme of THEMES) {
    it(`${theme}: add/del/context backgrounds are visibly distinct`, () => {
      const p = palette(theme);
      expect(p.bg, `${theme} --c-bg`).toMatch(/^#[0-9a-fA-F]{3,6}$/);
      const ctx = hexToRgb(p.bg);
      const add = mix(p.green, 14, p.bg);
      const del = mix(p.red, 14, p.bg);
      expect(dist(add, ctx)).toBeGreaterThan(8);
      expect(dist(del, ctx)).toBeGreaterThan(8);
      expect(dist(add, del)).toBeGreaterThan(8);
    });
  }

  it("add tint differs across light and dark themes", () => {
    const light = mix(palette("light").green, 14, palette("light").bg);
    const dark = mix(palette("dark").green, 14, palette("dark").bg);
    expect(dist(light, dark)).toBeGreaterThan(30);
  });
});

describe("tokens.css defines every --gh-* the renderers consume", () => {
  const required = [
    "--gh-bg",
    "--gh-bg-elev",
    "--gh-bg-inset",
    "--gh-border",
    "--gh-border-muted",
    "--gh-fg",
    "--gh-fg-muted",
    "--gh-fg-subtle",
    "--gh-accent",
    "--gh-accent-fg",
    "--gh-success",
    "--gh-warning",
    "--gh-danger",
    "--gh-merged",
    "--gh-draft",
    "--gh-diff-add-bg",
    "--gh-diff-add-fg",
    "--gh-diff-del-bg",
    "--gh-diff-del-fg",
    "--gh-diff-context-bg",
    "--gh-diff-context-fg",
    "--gh-diff-gutter-bg",
    "--gh-diff-gutter-fg",
    "--gh-diff-hunk-bg",
    "--gh-diff-hunk-fg",
    "--gh-diff-add-edge",
    "--gh-diff-del-edge",
    "--gh-diff-add-glyph",
    "--gh-diff-del-glyph",
    "--gh-syn-keyword",
    "--gh-syn-string",
    "--gh-syn-number",
    "--gh-syn-comment",
    "--gh-syn-function",
    "--gh-syn-variable",
    "--gh-syn-type",
    "--gh-syn-punctuation",
    "--gh-radius",
    "--gh-radius-sm",
    "--gh-font",
    "--gh-mono",
  ];
  for (const name of required) {
    it(`defines ${name}`, () => {
      expect(TOKENS).toContain(`${name}:`);
    });
  }
});
