export interface ThemeOption {
  id: Theme;
  label: string;
  mode: "light" | "dark";
}

export const THEME_OPTIONS = [
  { id: "light", label: "Light", mode: "light" },
  { id: "highcontrast", label: "High Contrast Light", mode: "light" },
  { id: "gruvboxlight", label: "Gruvbox Light", mode: "light" },
  { id: "solarizedlight", label: "Solarized Light", mode: "light" },
  { id: "everforestlight", label: "Everforest Light", mode: "light" },
  { id: "rosepinedawn", label: "Rosé Pine Dawn", mode: "light" },
  { id: "latte", label: "Catppuccin Latte", mode: "light" },
  { id: "nordlight", label: "Nord Light", mode: "light" },
  { id: "tokyoday", label: "Tokyo Night Day", mode: "light" },
  { id: "kanagawalotus", label: "Kanagawa Lotus", mode: "light" },
  { id: "sepia", label: "Sepia", mode: "light" },
  { id: "dark", label: "Dark", mode: "dark" },
  { id: "colorblind", label: "Color-blind safe", mode: "dark" },
  { id: "mocha", label: "Catppuccin Mocha", mode: "dark" },
  { id: "dracula", label: "Dracula", mode: "dark" },
  { id: "nord", label: "Nord", mode: "dark" },
  { id: "tokyonight", label: "Tokyo Night", mode: "dark" },
  { id: "gruvbox", label: "Gruvbox", mode: "dark" },
  { id: "solarized", label: "Solarized Dark", mode: "dark" },
  { id: "rosepine", label: "Rosé Pine", mode: "dark" },
  { id: "onedark", label: "One Dark", mode: "dark" },
  { id: "everforest", label: "Everforest", mode: "dark" },
  { id: "monokai", label: "Monokai", mode: "dark" },
  { id: "amoled", label: "AMOLED (high contrast)", mode: "dark" },
] as const;

export type Theme = (typeof THEME_OPTIONS)[number]["id"];

export const THEMES = THEME_OPTIONS.map((t) => t.id) as readonly Theme[];

export const THEME_LABELS: Record<Theme, string> = Object.fromEntries(
  THEME_OPTIONS.map((t) => [t.id, t.label]),
) as Record<Theme, string>;

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
 * this accessor is the seam the canvas renderer uses to read the identical
 * values (canvas cannot reference CSS variables). Every color the renderer
 * draws MUST come from here — no hardcoded colors in either renderer.
 *
 * Tokens are now derived from tsumikit semantic vars (var()/color-mix), so a
 * raw getPropertyValue would hand back an unresolved expression. We resolve
 * each token against `root` (the element carrying the active theme — the embed
 * root or <html>) through a throwaway probe whose computed `color` the browser
 * fully resolves to rgb.
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
  const probe = document.createElement("span");
  probe.style.cssText =
    "position:absolute;left:-9999px;top:0;width:0;height:0;visibility:hidden;pointer-events:none";
  root.appendChild(probe);
  const cs = getComputedStyle(probe);
  const v = (name: string): string => {
    probe.style.color = `var(${name})`;
    return cs.color.trim();
  };
  const tokens: ThemeTokens = {
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
  probe.remove();
  return tokens;
}
