export const THEMES = ["dark", "light", "colorblind-dark", "colorblind-light"] as const;
export type Theme = (typeof THEMES)[number];

export const THEME_LABELS: Record<Theme, string> = {
  dark: "Dark",
  light: "Light",
  "colorblind-dark": "Colorblind dark",
  "colorblind-light": "Colorblind light",
};

const STORAGE_KEY = "ghreview:theme";
const DEFAULT_THEME: Theme = "dark";

function isTheme(v: string | null): v is Theme {
  return v !== null && (THEMES as readonly string[]).includes(v);
}

export function getStoredTheme(): Theme | null {
  const v = localStorage.getItem(STORAGE_KEY);
  return isTheme(v) ? v : null;
}

function systemTheme(): Theme {
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export function resolveTheme(): Theme {
  return getStoredTheme() ?? systemTheme() ?? DEFAULT_THEME;
}

export function applyTheme(theme: Theme): void {
  document.documentElement.setAttribute("data-theme", theme);
}

export function storeTheme(theme: Theme): void {
  localStorage.setItem(STORAGE_KEY, theme);
}

export function setTheme(theme: Theme): void {
  storeTheme(theme);
  applyTheme(theme);
}

export function currentTheme(): Theme {
  const v = document.documentElement.getAttribute("data-theme");
  return isTheme(v) ? v : DEFAULT_THEME;
}

export function initTheme(): Theme {
  const theme = resolveTheme();
  applyTheme(theme);
  return theme;
}

/**
 * Diff-relevant design tokens resolved to concrete CSS color strings for the
 * active theme. The DOM renderer styles itself with the CSS variables directly;
 * this accessor is the seam CCT-608's canvas renderer uses to read the identical
 * values (canvas cannot reference CSS variables). Every color the renderer draws
 * MUST come from here — no hardcoded colors in either renderer.
 */
export interface ThemeTokens {
  bg: string;
  fg: string;
  fgMuted: string;
  accent: string;
  border: string;
  addBg: string;
  addFg: string;
  delBg: string;
  delFg: string;
  contextBg: string;
  contextFg: string;
  gutterBg: string;
  gutterFg: string;
  hunkBg: string;
  hunkFg: string;
  addEdge: string;
  delEdge: string;
  addGlyph: string;
  delGlyph: string;
  syntax: {
    keyword: string;
    string: string;
    number: string;
    comment: string;
    function: string;
    variable: string;
    type: string;
    punctuation: string;
  };
}

export function themeTokens(root: Element = document.documentElement): ThemeTokens {
  const s = getComputedStyle(root);
  const v = (name: string) => s.getPropertyValue(name).trim();
  return {
    bg: v("--gh-bg"),
    fg: v("--gh-fg"),
    fgMuted: v("--gh-fg-muted"),
    accent: v("--gh-accent"),
    border: v("--gh-border"),
    addBg: v("--gh-diff-add-bg"),
    addFg: v("--gh-diff-add-fg"),
    delBg: v("--gh-diff-del-bg"),
    delFg: v("--gh-diff-del-fg"),
    contextBg: v("--gh-diff-context-bg"),
    contextFg: v("--gh-diff-context-fg"),
    gutterBg: v("--gh-diff-gutter-bg"),
    gutterFg: v("--gh-diff-gutter-fg"),
    hunkBg: v("--gh-diff-hunk-bg"),
    hunkFg: v("--gh-diff-hunk-fg"),
    addEdge: v("--gh-diff-add-edge"),
    delEdge: v("--gh-diff-del-edge"),
    addGlyph: v("--gh-diff-add-glyph"),
    delGlyph: v("--gh-diff-del-glyph"),
    syntax: {
      keyword: v("--gh-syn-keyword"),
      string: v("--gh-syn-string"),
      number: v("--gh-syn-number"),
      comment: v("--gh-syn-comment"),
      function: v("--gh-syn-function"),
      variable: v("--gh-syn-variable"),
      type: v("--gh-syn-type"),
      punctuation: v("--gh-syn-punctuation"),
    },
  };
}
