import { browser } from '$app/environment';
import { error } from '@sveltejs/kit';
import type { CapabilitiesResponse } from '@bindings/CapabilitiesResponse';
import type { Feature } from '@bindings/Feature';
import { apiBase } from './config';
import { hasFeature, setCapabilities } from './features.svelte';

export type ProbedCapabilities = Partial<CapabilitiesResponse>;

/** `null` when the probe could not answer: older servers and offline starts
 *  keep the existing authentication/reconnection flow. */
export async function probeCapabilities(
	fetcher: typeof fetch,
	base: string
): Promise<ProbedCapabilities | null> {
	try {
		const response = await fetcher(`${base}/capabilities`, {
			credentials: 'include',
			cache: 'no-store',
			signal: AbortSignal.timeout(3_000)
		});
		if (!response.ok) return null;
		const caps = (await response.json()) as ProbedCapabilities | null;
		return caps && typeof caps === 'object' ? caps : null;
	} catch {
		return null;
	}
}

/** Only an explicit `workspace: false` (the retired headless host) disables the app. */
export function workspaceSupported(caps: ProbedCapabilities | null): boolean {
	return caps?.workspace !== false;
}

let boot: Promise<ProbedCapabilities | null> | null = null;

/** The one capabilities probe of the page: the first caller sends it, every
 *  later caller (the layout, a page load) shares its answer. */
export function bootCapabilities(
	fetcher: typeof fetch = fetch,
	base: string = apiBase()
): Promise<ProbedCapabilities | null> {
	boot ??= probeCapabilities(fetcher, base).then((caps) => {
		setCapabilities(caps);
		return caps;
	});
	return boot;
}

/** For a page `load`: a page whose feature is off is a 404. Runs before the
 *  layout mounts, so it waits for the probe itself. */
export async function requireFeature(name: Feature | null): Promise<void> {
	if (!browser || !name) return;
	await bootCapabilities();
	if (!hasFeature(name)) error(404, 'Not Found');
}
