import type { CapabilitiesResponse } from '@bindings/CapabilitiesResponse';

/** Only an explicit server restriction disables the workspace. Older servers
 * and offline probes keep the existing authentication/reconnection flow. */
export async function probeWorkspaceSupport(fetcher: typeof fetch, base: string): Promise<boolean> {
	try {
		const response = await fetcher(`${base}/capabilities`, {
			credentials: 'include',
			cache: 'no-store',
			signal: AbortSignal.timeout(3_000)
		});
		if (!response.ok) return true;
		const caps = (await response.json()) as Partial<CapabilitiesResponse> | null;
		return caps?.workspace !== false;
	} catch {
		return true;
	}
}
