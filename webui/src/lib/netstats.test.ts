import { describe, expect, it } from 'vitest';
import { formatBytes, net, normalizeRoute } from './netstats.svelte';

describe('normalizeRoute', () => {
	it('collapses uuid segments', () => {
		expect(normalizeRoute('/sessions/0d4e05cc-b588-48b0-b610-f86754fb7f2b/conversation')).toBe(
			'/sessions/:id/conversation'
		);
	});
	it('collapses numeric and long-hex segments', () => {
		expect(normalizeRoute('/machines/12345/keys')).toBe('/machines/:id/keys');
		expect(normalizeRoute('/uploads/f5f094d1e57a5b1563170755')).toBe('/uploads/:id');
	});
	it('keeps plain segments', () => {
		expect(normalizeRoute('/sessions/search/values')).toBe('/sessions/search/values');
	});
});

describe('formatBytes', () => {
	it('scales units', () => {
		expect(formatBytes(512)).toBe('512 B');
		expect(formatBytes(2048)).toBe('2.0 KB');
		expect(formatBytes(1615785)).toBe('1.54 MB');
	});
});

describe('net accumulator', () => {
	it('aggregates per normalized route and totals', () => {
		const before = net.total;
		net.recordApi('https://x.test/api/v1/sessions', 1000);
		net.recordApi('https://x.test/api/v1/sessions/0d4e05cc-b588-48b0-b610-f86754fb7f2b/conversation', 500);
		net.recordApi('https://x.test/api/v1/sessions/11111111-2222-3333-4444-555555555555/conversation', 250);
		net.recordWs(100);
		expect(net.total - before).toBe(1850);
		const conv = net.routes().find((r) => r.route === '/sessions/:id/conversation');
		expect(conv?.count).toBe(2);
		expect(conv?.bytes).toBe(750);
	});
});
