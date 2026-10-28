import { describe, expect, it, vi } from 'vitest';
import { probeWorkspaceSupport } from './workspaceSupport';

describe('server workspace support', () => {
	it('detects core before any workspace authentication or query', async () => {
		const fetcher = vi.fn<typeof fetch>().mockResolvedValue(Response.json({ distribution: 'core', workspace: false }));
		expect(await probeWorkspaceSupport(fetcher, 'https://acme.invalid/api/v1')).toBe(false);
		expect(fetcher).toHaveBeenCalledOnce();
		expect(fetcher).toHaveBeenCalledWith('https://acme.invalid/api/v1/capabilities', expect.objectContaining({ credentials: 'include', cache: 'no-store' }));
	});

	it.each([{ workspace: true }, { langfuse: { available: false } }, null])('preserves full or legacy capability responses: %j', async (body) => {
		const fetcher = vi.fn<typeof fetch>().mockResolvedValue(Response.json(body));
		expect(await probeWorkspaceSupport(fetcher, '/api/v1')).toBe(true);
	});

	it('keeps legacy and offline startup when probing is unavailable', async () => {
		const fetcher = vi.fn<typeof fetch>()
			.mockResolvedValueOnce(new Response('', { status: 404 }))
			.mockResolvedValueOnce(new Response('invalid json'))
			.mockRejectedValueOnce(new TypeError('offline'));
		for (let attempt = 0; attempt < 3; attempt++) {
			expect(await probeWorkspaceSupport(fetcher, '/api/v1')).toBe(true);
		}
	});
});
