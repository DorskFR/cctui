import { describe, expect, it, vi } from 'vitest';
import { AutoOpenOnce, autoOpenKey, collectMessageActions } from './messageActions';
import type { CctuiPluginModule, PluginInfo } from './types';

const info = (id: string, icon: string | null = null): PluginInfo => ({
	id,
	name: id,
	description: '',
	version: '1',
	icon,
	web: `/plugins/${id}/web/index.js`,
	skills: [],
	enabled: true,
	settings: [],
	config: {}
});
const pane = (() => {}) as unknown as CctuiPluginModule['sessionPane'];
const src = (id: string, module: Partial<CctuiPluginModule>, icon: string | null = null) => ({
	info: info(id, icon),
	module: { cctuiApi: 1 as const, ...module }
});
const msg = { role: 'assistant', text: 'yubisashi: https://x' };

describe('collectMessageActions', () => {
	it('returns the actions of every ready plugin for an assistant line', () => {
		const sources = [
			src('a', { sessionPane: pane, messageActions: () => [{ label: 'Open', params: { url: 'u' }, open: 'sessionPane' }] }),
			src('b', { sessionPane: pane, messageActions: () => [{ label: 'B', icon: 'eye', params: {}, open: 'sessionPane', autoOpen: true }] }, 'grid')
		];
		expect(collectMessageActions(sources, msg)).toEqual([
			{ pluginId: 'a', label: 'Open', icon: 'grid', params: { url: 'u' }, autoOpen: false },
			{ pluginId: 'b', label: 'B', icon: 'eye', params: {}, autoOpen: true }
		]);
	});
	it('ignores non-assistant or empty messages', () => {
		const sources = [src('a', { sessionPane: pane, messageActions: () => [{ label: 'x', params: {}, open: 'sessionPane' }] })];
		expect(collectMessageActions(sources, { role: 'user', text: 'hi' })).toEqual([]);
		expect(collectMessageActions(sources, { role: 'assistant', text: '' })).toEqual([]);
	});
	it('skips plugins without a pane, throwing plugins and malformed actions', () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		const sources = [
			src('nopane', { messageActions: () => [{ label: 'x', params: {}, open: 'sessionPane' }] }),
			src('throws', {
				sessionPane: pane,
				messageActions: () => {
					throw new Error('boom');
				}
			}),
			src('junk', {
				sessionPane: pane,
				messageActions: () =>
					[null, { label: 'no-open', params: {} }, { label: 'ok', params: { n: 1, s: 'v' }, open: 'sessionPane' }] as never
			})
		];
		expect(collectMessageActions(sources, msg)).toEqual([
			{ pluginId: 'junk', label: 'ok', icon: 'grid', params: { s: 'v' }, autoOpen: false }
		]);
		expect(warn).toHaveBeenCalledTimes(1);
		warn.mockRestore();
	});
});

describe('AutoOpenOnce', () => {
	const action = (pluginId: string, params: Record<string, string>, autoOpen = true) => ({
		pluginId,
		label: '',
		icon: 'grid' as const,
		params,
		autoOpen
	});
	it('keys on session, plugin and params regardless of key order', () => {
		expect(autoOpenKey('s', 'p', { a: '1', b: '2' })).toBe(autoOpenKey('s', 'p', { b: '2', a: '1' }));
		expect(autoOpenKey('s', 'p', { a: '1' })).not.toBe(autoOpenKey('s2', 'p', { a: '1' }));
	});
	it('honours each (session, plugin, params) once and ignores non-auto actions', () => {
		const once = new AutoOpenOnce();
		expect(once.take('s', [action('p', { url: 'u' }, false)])).toBeNull();
		expect(once.take('s', [action('p', { url: 'u' })])?.params).toEqual({ url: 'u' });
		expect(once.take('s', [action('p', { url: 'u' })])).toBeNull();
		expect(once.take('s', [action('p', { url: 'v' })])?.params).toEqual({ url: 'v' });
		expect(once.take('other', [action('p', { url: 'u' })])).not.toBeNull();
	});
});
