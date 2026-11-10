// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { probeCapabilities, workspaceSupported } from './workspaceSupport';

vi.mock('$app/environment', () => ({ browser: true }));

describe('server capability probe', () => {
	it('fetches capabilities once with the cookie and no cache', async () => {
		const body = { distribution: 'full', workspace: true, features: ['voice'], switchable: ['voice', 'plugins'] };
		const fetcher = vi.fn<typeof fetch>().mockResolvedValue(Response.json(body));
		expect(await probeCapabilities(fetcher, 'https://acme.invalid/api/v1')).toEqual(body);
		expect(fetcher).toHaveBeenCalledOnce();
		expect(fetcher).toHaveBeenCalledWith('https://acme.invalid/api/v1/capabilities', expect.objectContaining({ credentials: 'include', cache: 'no-store' }));
	});

	it('answers null when probing is unavailable', async () => {
		const fetcher = vi.fn<typeof fetch>()
			.mockResolvedValueOnce(new Response('', { status: 404 }))
			.mockResolvedValueOnce(new Response('invalid json'))
			.mockResolvedValueOnce(Response.json(null))
			.mockRejectedValueOnce(new TypeError('offline'));
		for (let attempt = 0; attempt < 4; attempt++) {
			expect(await probeCapabilities(fetcher, '/api/v1')).toBeNull();
		}
	});
});

describe('server workspace support', () => {
	it('is off only for an explicit headless host', () => {
		expect(workspaceSupported({ distribution: 'core', workspace: false })).toBe(false);
	});

	it.each([{ workspace: true }, { langfuse: { available: false } }, {}, null] as const)(
		'is on for full, legacy or unprobed servers: %j',
		(caps) => {
			expect(workspaceSupported(caps as Parameters<typeof workspaceSupported>[0])).toBe(true);
		}
	);

	it('stays on for a server with every switchable feature off', () => {
		expect(workspaceSupported({ workspace: true, features: [], switchable: ['plugins', 'voice'] })).toBe(true);
	});
});

describe('boot capabilities', () => {
	const realFetch = globalThis.fetch;
	afterEach(() => {
		globalThis.fetch = realFetch;
		vi.resetModules();
	});

	async function fresh(body: unknown) {
		vi.resetModules();
		const fetcher = vi.fn<typeof fetch>().mockImplementation(async () => Response.json(body));
		globalThis.fetch = fetcher;
		const boot = await import('./workspaceSupport');
		const features = await import('./features.svelte');
		return { fetcher, boot, features };
	}

	it('probes once however many callers ask', async () => {
		const { fetcher, boot } = await fresh({ workspace: true, features: [], switchable: ['plugins'] });
		await Promise.all([boot.bootCapabilities(), boot.bootCapabilities(), boot.requireFeature(null)]);
		await boot.bootCapabilities();
		expect(fetcher).toHaveBeenCalledOnce();
	});

	it('feeds the feature store', async () => {
		const { boot, features } = await fresh({ workspace: true, features: ['voice'], switchable: ['voice', 'plugins'] });
		await boot.bootCapabilities();
		expect(features.hasFeature('voice')).toBe(true);
		expect(features.hasFeature('plugins')).toBe(false);
	});

	it('turns a page of an off feature into a 404', async () => {
		const { boot } = await fresh({ workspace: true, features: [], switchable: ['plugins'] });
		await expect(boot.requireFeature('plugins')).rejects.toMatchObject({ status: 404 });
	});

	it('lets a page of an on or unknown feature load', async () => {
		const { boot } = await fresh({ workspace: true, features: ['plugins'], switchable: ['plugins'] });
		await expect(boot.requireFeature('plugins')).resolves.toBeUndefined();
		await expect(boot.requireFeature('voice')).resolves.toBeUndefined();
	});

	it('lets every page load when the probe fails', async () => {
		vi.resetModules();
		globalThis.fetch = vi.fn<typeof fetch>().mockRejectedValue(new TypeError('offline'));
		const boot = await import('./workspaceSupport');
		await expect(boot.requireFeature('plugins')).resolves.toBeUndefined();
	});
});
