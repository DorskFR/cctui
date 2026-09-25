// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError, api, apiBlob, apiFetch, request } from './api';
import { auth } from './auth.svelte';

const realFetch = globalThis.fetch;
let calls: { url: string; init: RequestInit }[] = [];

function respond(status = 200, body: unknown = {}) {
	globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
		calls.push({ url: String(input), init });
		return new Response(status === 204 ? null : JSON.stringify(body), {
			status,
			headers: { 'content-type': 'application/json' }
		});
	}) as typeof fetch;
}

const headers = (i = 0) => new Headers(calls[i].init.headers);

beforeEach(() => {
	calls = [];
	window.CCTUI_CONFIG = { apiBase: 'https://api.test' };
});

afterEach(() => {
	globalThis.fetch = realFetch;
	delete window.CCTUI_CONFIG;
	vi.restoreAllMocks();
});

describe('cctui api calls', () => {
	it('GET sends the cookie and no body headers', async () => {
		respond(200, { ok: true });
		await api.get('/things-get', { a: 1, b: undefined });
		expect(calls[0].url).toBe('https://api.test/api/v1/things-get?a=1');
		expect(calls[0].init.credentials).toBe('include');
		expect(calls[0].init.method).toBe('GET');
		expect(headers().get('content-type')).toBeNull();
		expect(headers().get('authorization')).toBeNull();
	});

	it.each([
		['post', 'POST'],
		['patch', 'PATCH'],
		['put', 'PUT']
	] as const)('%s sends the cookie and a JSON body', async (verb, method) => {
		respond(200, {});
		await api[verb](`/things-${verb}`, { x: 1 });
		expect(calls[0].init.method).toBe(method);
		expect(calls[0].init.credentials).toBe('include');
		expect(headers().get('content-type')).toBe('application/json');
		expect(calls[0].init.body).toBe('{"x":1}');
	});

	it('del sends the cookie', async () => {
		respond(204);
		await api.del('/things-del');
		expect(calls[0].init.method).toBe('DELETE');
		expect(calls[0].init.credentials).toBe('include');
	});

	it('postForm lets the browser set the multipart content type', async () => {
		respond(200, {});
		const form = new FormData();
		form.append('f', 'v');
		await api.postForm('/upload', form);
		expect(calls[0].init.method).toBe('POST');
		expect(calls[0].init.credentials).toBe('include');
		expect(calls[0].init.body).toBe(form);
		expect(new Headers(calls[0].init.headers).get('content-type')).toBeNull();
	});

	it('a 401 ends the cctui session', async () => {
		const out = vi.spyOn(auth, 'markLoggedOut').mockImplementation(() => {});
		respond(401);
		await expect(api.get('/things-401')).rejects.toBeInstanceOf(ApiError);
		expect(out).toHaveBeenCalledOnce();
	});
});

describe('request against another backend with a bearer', () => {
	it('sends the bearer and never the cctui cookie', async () => {
		respond(200, { items: [] });
		await request('https://gh.example', '/v1/accounts', { token: 'tok' });
		expect(calls[0].url).toBe('https://gh.example/v1/accounts');
		expect(calls[0].init.credentials).toBe('omit');
		expect(headers().get('authorization')).toBe('Bearer tok');
	});

	it('a 401 does not end the cctui session', async () => {
		const out = vi.spyOn(auth, 'markLoggedOut').mockImplementation(() => {});
		respond(401);
		await expect(request('https://gh.example', '/v1/x', { token: 'tok' })).rejects.toMatchObject({
			status: 401
		});
		expect(out).not.toHaveBeenCalled();
	});

	it('surfaces a nested error message', async () => {
		respond(409, { error: { code: 'conflict', message: 'login owned by another user' } });
		await expect(
			request('https://gh.example', '/v1/accounts', { method: 'POST', token: 'tok', body: {} })
		).rejects.toThrow('login owned by another user');
	});
});

describe('raw fetches', () => {
	it('apiBlob sends the cookie and returns the response untouched', async () => {
		respond(404, { error: 'gone' });
		const res = await apiBlob('/api/v1/sessions/s/attachments/a');
		expect(res.status).toBe(404);
		expect(calls[0].url).toBe('/api/v1/sessions/s/attachments/a');
		expect(calls[0].init.credentials).toBe('include');
	});

	it('apiFetch keeps the caller init and forces the cookie', async () => {
		respond(200);
		await apiFetch('https://api.test/api/v1/auth/logout', {
			method: 'POST',
			credentials: 'omit',
			headers: { 'Content-Type': 'application/json' }
		});
		expect(calls[0].init.method).toBe('POST');
		expect(calls[0].init.credentials).toBe('include');
		expect(headers().get('content-type')).toBe('application/json');
	});
});
