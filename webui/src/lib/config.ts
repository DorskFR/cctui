import { browser } from '$app/environment';

/** Server origin (no trailing slash). Overridable at runtime via static/config.js. */
export function apiOrigin(): string {
	if (browser && window.CCTUI_CONFIG?.apiBase) {
		return window.CCTUI_CONFIG.apiBase.replace(/\/$/, '');
	}
	// Default to the origin the SPA is served from (same-origin deploy).
	return browser ? window.location.origin : '';
}

export function apiBase(): string {
	return `${apiOrigin()}/api/v1`;
}

/** ws:// or wss:// base for the TUI websocket. */
export function wsBase(): string {
	return `${apiOrigin().replace(/^http/, 'ws')}/api/v1`;
}

// gh-review backend origin (CCT-610), or null when unconfigured — the connector
// is optional, so an unset value hides the Review center instead of erroring.
export function ghreviewUrl(): string | null {
	if (browser && window.CCTUI_CONFIG?.ghreviewUrl) {
		return window.CCTUI_CONFIG.ghreviewUrl.replace(/\/$/, '');
	}
	return null;
}
