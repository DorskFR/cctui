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

export interface OtelConfig {
	/** OTLP/HTTP collector base; spans post to `${endpoint}/v1/traces`. */
	endpoint: string;
	serviceName?: string;
	sampleRatio?: number;
}

export function otelConfig(): OtelConfig | null {
	if (!browser) return null;
	const otel = (window.CCTUI_CONFIG as { otel?: Partial<OtelConfig> } | undefined)?.otel;
	return otel?.endpoint ? { ...otel, endpoint: otel.endpoint } : null;
}
