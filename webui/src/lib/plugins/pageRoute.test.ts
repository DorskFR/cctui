import { describe, expect, it } from 'vitest';
import { hostHref, pageBasePath, pageNavItems, pluginPath, resolvePageState } from './pageRoute';
import type { PluginInfo } from './types';

const info = (over: Partial<PluginInfo> = {}): PluginInfo => ({
	id: 'demo',
	name: 'Demo',
	description: '',
	version: '1.0.0',
	icon: null,
	web: '/plugins/demo/web/index.js?v=abcd1234',
	page: { title: 'Demo app', icon: 'eye' },
	styles: [],
	skills: [],
	enabled: false,
	settings: [],
	config: {},
	instanceSettings: [],
	instanceSettingValues: {},
	backend: false,
	...over
});

describe('pluginPath', () => {
	const base = pageBasePath('demo');
	it('roots the plugin path and drops a trailing slash', () => {
		expect(pluginPath(base, '/apps/demo')).toBe('/');
		expect(pluginPath(base, '/apps/demo/')).toBe('/');
		expect(pluginPath(base, '/apps/demo/pr/12')).toBe('/pr/12');
		expect(pluginPath(base, '/apps/demo/pr/12/')).toBe('/pr/12');
	});
	it('answers / for a pathname outside the base', () => {
		expect(pluginPath(base, '/sessions')).toBe('/');
	});
});

describe('hostHref', () => {
	const base = pageBasePath('demo');
	it('joins a plugin path onto the base', () => {
		expect(hostHref(base, '/pr/12')).toBe('/apps/demo/pr/12');
		expect(hostHref(base, 'pr/12')).toBe('/apps/demo/pr/12');
		expect(hostHref(base, '/')).toBe('/apps/demo');
		expect(hostHref(base, '/pr/12?tab=files#l3')).toBe('/apps/demo/pr/12?tab=files#l3');
	});
	it('refuses to navigate the host off the plugin base', () => {
		expect(hostHref(base, 'https://evil.example/x')).toBe('/apps/demo');
		expect(hostHref(base, '//evil.example/x')).toBe('/apps/demo');
		expect(hostHref(base, 'javascript:alert(1)')).toBe('/apps/demo');
	});
});

describe('resolvePageState', () => {
	const ready = () => ({ status: 'ready' as const });
	it('refuses an id that is not a plugin id', () => {
		expect(resolvePageState({ id: 'Bad Id', list: [], enabled: {}, moduleState: ready }).status).toBe('invalid');
	});
	it('reports an unknown plugin', () => {
		expect(resolvePageState({ id: 'demo', list: [], enabled: { demo: true }, moduleState: ready }).status).toBe(
			'unknown'
		);
	});
	it('reports a plugin with no page surface', () => {
		const list = [info({ page: null })];
		expect(resolvePageState({ id: 'demo', list, enabled: { demo: true }, moduleState: ready }).status).toBe('no-page');
	});
	it('gates on the user switch', () => {
		const list = [info()];
		expect(resolvePageState({ id: 'demo', list, enabled: {}, moduleState: ready }).status).toBe('not-enabled');
		expect(resolvePageState({ id: 'demo', list, enabled: { demo: false }, moduleState: ready }).status).toBe(
			'not-enabled'
		);
	});
	it('is loading until the bundle resolves, then ready', () => {
		const list = [info()];
		expect(resolvePageState({ id: 'demo', list, enabled: { demo: true }, moduleState: () => null }).status).toBe(
			'loading'
		);
		expect(
			resolvePageState({ id: 'demo', list, enabled: { demo: true }, moduleState: () => ({ status: 'loading' }) })
				.status
		).toBe('loading');
		expect(resolvePageState({ id: 'demo', list, enabled: { demo: true }, moduleState: ready }).status).toBe('ready');
	});
	it('carries the loader error on a failed bundle', () => {
		const state = resolvePageState({
			id: 'demo',
			list: [info()],
			enabled: { demo: true },
			moduleState: () => ({ status: 'failed', error: 'boom' })
		});
		expect(state).toMatchObject({ status: 'failed', error: 'boom' });
	});
});

describe('pageNavItems', () => {
	it('lists enabled page plugins with their manifest title and icon', () => {
		const list = [
			info(),
			info({ id: 'pane-only', page: null }),
			info({ id: 'off' }),
			info({ id: 'no-title', page: { title: '' }, icon: 'upload' })
		];
		expect(pageNavItems(list, { demo: true, 'pane-only': true, 'no-title': true })).toEqual([
			{ href: '/apps/demo', label: 'Demo app', iconName: 'eye' },
			{ href: '/apps/no-title', label: 'Demo', iconName: 'upload' }
		]);
	});
	it('is empty when nothing is switched on', () => {
		expect(pageNavItems([info()], {})).toEqual([]);
	});
});
