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
let known: ProbedCapabilities | null = null;
const listeners = new Set<(caps: ProbedCapabilities) => void>();

function probe(fetcher: typeof fetch, base: string): Promise<ProbedCapabilities | null> {
	return probeCapabilities(fetcher, base).then((caps) => {
		if (!caps) return known;
		known = caps;
		setCapabilities(caps);
		for (const fn of listeners) fn(caps);
		return caps;
	});
}

/** The capabilities probe of the page: the first caller sends it, every
 *  later caller (the layout, a page load) shares its answer. */
export function bootCapabilities(
	fetcher: typeof fetch = fetch,
	base: string = apiBase()
): Promise<ProbedCapabilities | null> {
	boot ??= probe(fetcher, base);
	return boot;
}

/** Probe again, for when the session has just become authenticated: the boot
 *  probe may have been refused for want of a login. Later `bootCapabilities`
 *  callers share this answer. A failed probe keeps the last answer. */
export function refreshCapabilities(
	fetcher: typeof fetch = fetch,
	base: string = apiBase()
): Promise<ProbedCapabilities | null> {
	boot = probe(fetcher, base);
	return boot;
}

/** Called with every answer a probe adopts; returns the unsubscribe. */
export function onCapabilities(fn: (caps: ProbedCapabilities) => void): () => void {
	listeners.add(fn);
	return () => listeners.delete(fn);
}

/** For a page `load`: a page whose feature is off is a 404. Runs before the
 *  layout mounts, so it waits for the probe itself. */
export async function requireFeature(name: Feature | null): Promise<void> {
	if (!browser || !name) return;
	await bootCapabilities();
	if (!hasFeature(name)) error(404, 'Not Found');
}
