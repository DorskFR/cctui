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
