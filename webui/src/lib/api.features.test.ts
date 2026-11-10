// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FeatureOffError, api, apiBlob, apiFetch, routeFeature } from './api';
import { setCapabilitiesForTest } from './features.svelte';
import { endpoints } from './queries/endpoints';

const realFetch = globalThis.fetch;
let sent: string[] = [];

beforeEach(() => {
	sent = [];
	window.CCTUI_CONFIG = { apiBase: 'https://api.test' };
	globalThis.fetch = vi.fn(async (input: RequestInfo | URL) => {
		sent.push(String(input));
		return Response.json([]);
	}) as typeof fetch;
});

afterEach(() => {
	globalThis.fetch = realFetch;
	delete window.CCTUI_CONFIG;
	setCapabilitiesForTest(null);
	vi.restoreAllMocks();
});

const off = (...names: string[]) =>
	setCapabilitiesForTest({ features: [], switchable: names });

describe('route feature lookup', () => {
	it('names the feature of a templated route', () => {
		expect(routeFeature('GET', '/plugins')).toBe('plugins');
		expect(routeFeature('POST', '/plugins/ghreview/backend/a/b')).toBe('plugins');
		expect(routeFeature('GET', '/voice/config')).toBe('voice');
	});

	it('prefers a literal segment over a placeholder', () => {
		expect(routeFeature('GET', '/sessions/search?q=x')).toBe('transcript_search');
		expect(routeFeature('GET', '/sessions/abc')).toBeNull();
	});

	it('treats paths outside the route table as core', () => {
		expect(routeFeature('POST', '/auth/login')).toBeNull();
		expect(routeFeature('GET', '/no/such/path')).toBeNull();
	});
});

describe('request gating', () => {
	it('sends no request to a route of an off feature', async () => {
		off('plugins', 'voice', 'self_update');
		await expect(api.get('/plugins')).rejects.toBeInstanceOf(FeatureOffError);
		await expect(endpoints.voiceConfig()).rejects.toMatchObject({ status: 404, feature: 'voice' });
		await expect(endpoints.changelog()).rejects.toBeInstanceOf(FeatureOffError);
		await expect(apiFetch('https://api.test/api/v1/admin/plugins')).rejects.toBeInstanceOf(FeatureOffError);
		await expect(api.postBlob('/admin/instance/speech/test')).rejects.toBeInstanceOf(FeatureOffError);
		expect(sent).toEqual([]);
	});

	it('refuses a blob under an off feature', async () => {
		off('files');
		await expect(apiBlob('https://api.test/api/v1/sessions/s1/linked-file?path=a.png')).rejects.toBeInstanceOf(FeatureOffError);
		expect(sent).toEqual([]);
	});

	it('still sends core requests while features are off', async () => {
		off('plugins', 'voice');
		await api.get('/accounts');
		await apiFetch('https://api.test/api/v1/me');
		expect(sent).toEqual(['https://api.test/api/v1/accounts', 'https://api.test/api/v1/me']);
	});

	it('sends requests of an on feature', async () => {
		setCapabilitiesForTest({ features: ['plugins'], switchable: ['plugins', 'voice'] });
		await api.get('/plugins');
		expect(sent).toEqual(['https://api.test/api/v1/plugins']);
	});

	it('sends everything before capabilities load', async () => {
		await api.get('/plugins');
		expect(sent).toHaveLength(1);
	});

	it('does not gate a foreign origin', async () => {
		off('plugins');
		await api.get('/accounts');
		await apiFetch('https://elsewhere.test/api/v1/plugins');
		expect(sent).toContain('https://elsewhere.test/api/v1/plugins');
	});
});
