// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { health } = vi.hoisted(() => ({
	health: { reportFailure: vi.fn(), reportOk: vi.fn(), waitUntilUp: vi.fn() }
}));
vi.mock('./backendHealth.svelte', async (orig) => ({
	...(await orig<typeof import('./backendHealth.svelte')>()),
	backendHealth: health
}));

import { api, apiFetch, request } from './api';
import { auth } from './auth.svelte';

const realFetch = globalThis.fetch;

function respond(...statuses: number[]) {
	globalThis.fetch = vi.fn(async () => {
		const status = statuses.shift() ?? 200;
		return new Response(JSON.stringify({}), { status });
	}) as typeof fetch;
}

beforeEach(() => {
	window.CCTUI_CONFIG = { apiBase: 'https://api.test' };
	vi.spyOn(auth, 'markLoggedOut').mockImplementation(() => {});
});
afterEach(() => {
	globalThis.fetch = realFetch;
	delete window.CCTUI_CONFIG;
	vi.restoreAllMocks();
	vi.clearAllMocks();
});

describe('api client feeds backend health', () => {
	it.each([502, 503, 504])('a %i reports a failure', async (status) => {
		respond(status);
		await expect(api.get('/x')).rejects.toThrow();
		expect(health.reportFailure).toHaveBeenCalledTimes(1);
	});

	it('a network error reports a failure and still rejects', async () => {
		globalThis.fetch = vi.fn(async () => {
			throw new TypeError('Failed to fetch');
		}) as typeof fetch;
		await expect(api.get('/x')).rejects.toThrow(TypeError);
		expect(health.reportFailure).toHaveBeenCalledTimes(1);
	});

	it.each([400, 401, 403, 404, 500])('a %i is not an outage', async (status) => {
		respond(status);
		await expect(api.get('/x')).rejects.toThrow();
		expect(health.reportFailure).not.toHaveBeenCalled();
	});

	it('an abort is not an outage', async () => {
		globalThis.fetch = vi.fn(async () => {
			throw new DOMException('aborted', 'AbortError');
		}) as typeof fetch;
		await expect(api.get('/x')).rejects.toThrow();
		expect(health.reportFailure).not.toHaveBeenCalled();
	});

	it('a success reports ok', async () => {
		respond(200);
		await api.get('/x');
		expect(health.reportOk).toHaveBeenCalledTimes(1);
	});

	it('a foreign backend (token) never feeds cctui health', async () => {
		respond(503);
		await expect(request('https://other.test', '/x', { token: 't' })).rejects.toThrow();
		expect(health.reportFailure).not.toHaveBeenCalled();
	});

	it('raw apiFetch reports outages too', async () => {
		respond(502);
		await apiFetch('https://api.test/api/v1/me');
		expect(health.reportFailure).toHaveBeenCalledTimes(1);
	});
});

describe('auth.init during an outage', () => {
	it('waits for the backend instead of showing the login', async () => {
		respond(503, 200);
		health.waitUntilUp.mockResolvedValue(undefined);
		auth.checking = true;
		auth.isAuthed = false;
		await auth.init();
		expect(health.waitUntilUp).toHaveBeenCalledTimes(1);
		expect(auth.isAuthed).toBe(true);
		expect(auth.checking).toBe(false);
	});

	it('a 401 is a plain logged-out answer', async () => {
		respond(401);
		auth.checking = true;
		await auth.init();
		expect(health.waitUntilUp).not.toHaveBeenCalled();
		expect(auth.isAuthed).toBe(false);
		expect(auth.checking).toBe(false);
	});
});
