import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeResponse } from '$lib/bindings/MeResponse';

const fetched = vi.hoisted(() => vi.fn(async () => new Response('{}')));
const goto = vi.hoisted(() => vi.fn(async () => undefined));
const toasted = vi.hoisted(() => ({ ok: vi.fn(), info: vi.fn(), error: vi.fn() }));

vi.mock('$app/environment', () => ({ browser: false }));
vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$lib/api', () => ({ apiFetch: fetched }));
vi.mock('$lib/config', () => ({ apiBase: () => '/api/v1' }));
vi.mock('$lib/toast.svelte', () => ({ toasts: toasted }));

import { hostContext, hostUser } from './hostContext';
import { pluginSpawn } from './spawnRequest.svelte';

const me = (over: Partial<MeResponse> = {}): MeResponse => ({
	role: 'user',
	user_id: 'u1',
	user_name: 'dorsk',
	machine_id: null,
	scopes: [],
	token_preview: 'cctui_u_ab…34',
	...over
});

beforeEach(() => {
	fetched.mockClear();
	goto.mockClear();
	for (const fn of Object.values(toasted)) fn.mockClear();
	pluginSpawn.close();
});

describe('hostUser', () => {
	it('is unknown until /me answers', () => {
		expect(hostUser(undefined)).toBeUndefined();
		expect(hostUser(me({ user_id: null }))).toBeUndefined();
	});
	it('reports the admin role as a flag and falls back to the id for a nameless user', () => {
		expect(hostUser(me({ role: 'admin' }))).toEqual({ id: 'u1', name: 'dorsk', isAdmin: true });
		expect(hostUser(me({ user_name: null }))).toEqual({ id: 'u1', name: 'u1', isAdmin: false });
	});
});

describe('host context v1.1', () => {
	const ctx = () => hostContext({ pluginId: 'ghreview', user: hostUser(me()) });

	it('reports the contract major and minor', () => {
		expect(ctx().cctuiApi).toBe(1);
		expect(ctx().cctuiApiMinor).toBeGreaterThanOrEqual(1);
	});

	it('sends apiFetch at the host API with the user session', async () => {
		await ctx().apiFetch?.('/sessions', { method: 'GET' });
		expect(fetched).toHaveBeenCalledWith('/api/v1/sessions', { method: 'GET' });
		await ctx().apiFetch?.('me');
		expect(fetched).toHaveBeenLastCalledWith('/api/v1/me', undefined);
	});

	it('sends pluginFetch through the host proxy for its own id', async () => {
		await ctx().pluginFetch?.('/pulls?state=open');
		expect(fetched).toHaveBeenCalledWith('/api/v1/plugins/ghreview/backend/pulls?state=open', undefined);
	});

	it('navigates the host SPA', () => {
		ctx().navigate?.('/sessions/abc');
		expect(goto).toHaveBeenCalledWith('/sessions/abc');
		ctx().navigate?.('sessions/abc');
		expect(goto).toHaveBeenLastCalledWith('/sessions/abc');
	});

	it('openSpawn pre-fills the host form without launching anything', () => {
		ctx().openSpawn?.({ prompt: 'review PR 12', working_dir: '/w/repo', machine_id: 'm1' });
		expect(pluginSpawn.prefill).toEqual({ prompt: 'review PR 12', working_dir: '/w/repo', machine_id: 'm1' });
	});

	it('omits spawn fields the plugin left out, so the form keeps its own defaults', () => {
		ctx().openSpawn?.({ prompt: 'hello' });
		expect(pluginSpawn.prefill).toEqual({ prompt: 'hello' });
	});

	it('maps the toast tone onto the host toaster', () => {
		ctx().toast?.('saved', 'ok');
		ctx().toast?.('heads up');
		ctx().toast?.('broke', 'error');
		expect(toasted.ok).toHaveBeenCalledWith('saved');
		expect(toasted.info).toHaveBeenCalledWith('heads up');
		expect(toasted.error).toHaveBeenCalledWith('broke');
	});
});
