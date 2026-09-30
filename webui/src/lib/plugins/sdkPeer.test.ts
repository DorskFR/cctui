import { describe, expect, it, vi } from 'vitest';
import {
	checkHostPeers,
	describeMismatches,
	peerMismatches,
	satisfies
} from '../../../plugin-sdk/peer';
import type { PluginRuntimeManifest } from '../../../plugin-sdk/types';

const host: PluginRuntimeManifest = { cctuiApi: 1, cctuiApiMinor: 1, svelte: '5.25.3', tsumikit: '0.63.1' };

describe('satisfies', () => {
	it('accepts an exact version only when it matches', () => {
		expect(satisfies('1.2.3', '1.2.3')).toBe(true);
		expect(satisfies('1.2.4', '1.2.3')).toBe(false);
	});
	it('treats the minor as the breaking unit below 1.0.0', () => {
		expect(satisfies('0.63.9', '^0.63.1')).toBe(true);
		expect(satisfies('0.64.0', '^0.63.1')).toBe(false);
		expect(satisfies('0.63.0', '^0.63.1')).toBe(false);
	});
	it('handles caret, tilde and conjunctions', () => {
		expect(satisfies('5.25.3', '^5.0.0')).toBe(true);
		expect(satisfies('6.0.0', '^5.0.0')).toBe(false);
		expect(satisfies('5.25.9', '~5.25.0')).toBe(true);
		expect(satisfies('5.26.0', '~5.25.0')).toBe(false);
		expect(satisfies('0.63.1', '>=0.63.1 <1.0.0')).toBe(true);
		expect(satisfies('1.0.0', '>=0.63.1 <1.0.0')).toBe(false);
	});
	it('refuses what it cannot parse instead of passing it', () => {
		expect(satisfies('not-a-version', '^1.0.0')).toBe(false);
		expect(satisfies('1.0.0', 'latest')).toBe(false);
		expect(satisfies('1.0.0', '')).toBe(false);
	});
	it('ignores a prerelease suffix on the host version', () => {
		expect(satisfies('5.25.3-next.1', '^5.25.0')).toBe(true);
	});
});

describe('peerMismatches', () => {
	it('is empty for a host inside every range', () => {
		expect(peerMismatches(host, { svelte: '^5.25.0', tsumikit: '^0.63.1', cctuiApi: 1 })).toEqual([]);
	});
	it('names what is off, with both sides', () => {
		const out = peerMismatches(host, { tsumikit: '^0.64.0', cctuiApi: 2 });
		expect(out).toEqual([
			{ what: 'cctuiApi', wanted: '2', found: '1' },
			{ what: 'tsumikit', wanted: '^0.64.0', found: '0.63.1' }
		]);
		expect(describeMismatches(out)).toContain('tsumikit 0.63.1 does not satisfy ^0.64.0');
	});
	it('checks only what the plugin declared', () => {
		expect(peerMismatches(host, {})).toEqual([]);
	});
});

describe('checkHostPeers', () => {
	it('reads the host manifest and reports mismatches', async () => {
		const fetchImpl = vi.fn(async () => new Response(JSON.stringify(host)));
		await expect(
			checkHostPeers({ svelte: '^5.25.0', fetchImpl: fetchImpl as unknown as typeof fetch })
		).resolves.toEqual([]);
		expect(fetchImpl).toHaveBeenCalledWith('/plugin-runtime/manifest.json');
	});
	it('answers null when the manifest cannot be read, so an older host is not a mismatch', async () => {
		const missing = vi.fn(async () => new Response('', { status: 404 }));
		await expect(checkHostPeers({ fetchImpl: missing as unknown as typeof fetch })).resolves.toBeNull();
		const broken = vi.fn(async () => {
			throw new TypeError('offline');
		});
		await expect(checkHostPeers({ fetchImpl: broken as unknown as typeof fetch })).resolves.toBeNull();
	});
});
