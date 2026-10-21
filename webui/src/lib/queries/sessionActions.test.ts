import { describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import type { DispatchRequest } from '@bindings/DispatchRequest';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SessionListResponse } from '@bindings/SessionListResponse';

const qc = new QueryClient();
vi.mock('@tanstack/svelte-query', async (orig) => ({
	...(await orig<typeof import('@tanstack/svelte-query')>()),
	useQueryClient: () => qc
}));
vi.mock('./endpoints', () => ({ endpoints: { dispatch: vi.fn(async () => ({})) } }));

const { useSessionActions } = await import('./sessionActions');
const { qk } = await import('./keys');

describe('dispatch optimistic card', () => {
	it('prepends the placeholder and keeps the cached parent stubs', async () => {
		const parents = [{ id: 'orch', name: 'orch-x', working_dir: '/w', status: 'active' as const }];
		const old = { id: 'old' } as SessionListItem;
		qc.setQueryData<SessionListResponse>(qk.sessions(false), { sessions: [old], parents });
		const body = { session_id: 'new-1', profile: 'p', prompt: 'go' } as unknown as DispatchRequest;

		await useSessionActions().dispatch(body);

		const after = qc.getQueryData<SessionListResponse>(qk.sessions(false));
		expect(after?.sessions.map((s) => s.id)).toEqual(['new-1', 'old']);
		expect(after?.parents).toEqual(parents);
	});

	it('starts an empty cache with no parents', async () => {
		qc.removeQueries({ queryKey: qk.sessions(false) });
		await useSessionActions().dispatch({ session_id: 'new-2' } as unknown as DispatchRequest);
		const after = qc.getQueryData<SessionListResponse>(qk.sessions(false));
		expect(after?.sessions.map((s) => s.id)).toEqual(['new-2']);
		expect(after?.parents).toEqual([]);
	});
});
