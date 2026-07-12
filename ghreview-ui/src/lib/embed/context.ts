import type { Theme } from "../theme/theme";

// When embedded (CCT-610) the theme lives on the mount container, not <html>,
// so the host app's own theme is never clobbered. Review provides this; TopBar
// falls back to the global document theme when it is absent (standalone).
export interface EmbedThemeContext {
  get(): Theme;
  set(theme: Theme): void;
}

export const EMBED_THEME_KEY = Symbol("ghreview:embed-theme");
