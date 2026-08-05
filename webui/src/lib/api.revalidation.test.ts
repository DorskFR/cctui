import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';

const realFetch = globalThis.fetch;

function jsonResponse(body: unknown, headers: Record<string, string> = {}, status = 200) {
	return new Response(status === 304 ? null : JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json', ...headers }
	});
}

beforeEach(() => {
	window.CCTUI_CONFIG = { apiBase: 'https://api.test/api/v1' };
});

afterEach(() => {
	globalThis.fetch = realFetch;
	delete window.CCTUI_CONFIG;
	vi.restoreAllMocks();
});

describe('x-etag revalidation', () => {
	it('sends If-None-Match after seeing x-etag and replays the body on 304', async () => {
		const calls: Request[] = [];
		let n = 0;
		globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
			calls.push(new Request(input, init));
			n += 1;
			if (n === 1) return jsonResponse({ v: 1 }, { 'x-etag': '"abc"' });
			return jsonResponse(null, { 'x-etag': '"abc"' }, 304);
		}) as typeof fetch;

		const first = await api.get<{ v: number }>('/reval-a');
		expect(first).toEqual({ v: 1 });
		expect(calls[0].headers.get('if-none-match')).toBeNull();

		const second = await api.get<{ v: number }>('/reval-a');
		expect(second).toEqual({ v: 1 });
		expect(calls[1].headers.get('if-none-match')).toBe('"abc"');
	});

	it('a changed body replaces the stored etag', async () => {
		let n = 0;
		const calls: Request[] = [];
		globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
			calls.push(new Request(input, init));
			n += 1;
			if (n === 1) return jsonResponse({ v: 1 }, { 'x-etag': '"a1"' });
			return jsonResponse({ v: 2 }, { 'x-etag': '"a2"' });
		}) as typeof fetch;

		expect(await api.get('/reval-b')).toEqual({ v: 1 });
		expect(await api.get('/reval-b')).toEqual({ v: 2 });
		await api.get('/reval-b');
		expect(calls[2].headers.get('if-none-match')).toBe('"a2"');
	});

	it('responses without x-etag never send If-None-Match', async () => {
		const calls: Request[] = [];
		globalThis.fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
			calls.push(new Request(input, init));
			return jsonResponse({ v: 1 });
		}) as typeof fetch;

		await api.get('/reval-c');
		await api.get('/reval-c');
		expect(calls[1].headers.get('if-none-match')).toBeNull();
	});
});
