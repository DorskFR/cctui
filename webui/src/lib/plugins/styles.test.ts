// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from 'vitest';
import { ensurePluginStyles, styleUrls } from './styles';
import type { PluginInfo } from './types';

const info = (over: Partial<PluginInfo> = {}): PluginInfo => ({
	id: 'demo',
	name: 'Demo',
	description: '',
	version: '1.0.0',
	icon: null,
	web: '/plugins/demo/web/index.js?v=abcd1234',
	page: null,
	styles: [],
	skills: [],
	enabled: true,
	settings: [],
	config: {},
	instanceSettings: [],
	instanceSettingValues: {},
	backend: false,
	...over
});

describe('styleUrls', () => {
	it("resolves a manifest-relative entry against the bundle's folder and keeps its cache-buster", () => {
		expect(styleUrls(info({ styles: ['app.css', 'themes/dark.css'] }))).toEqual([
			'/plugins/demo/web/app.css?v=abcd1234',
			'/plugins/demo/web/themes/dark.css?v=abcd1234'
		]);
	});
	it('passes an already-served absolute path through', () => {
		expect(styleUrls(info({ styles: ['/plugins/demo/web/app.css?v=1'] }))).toEqual([
			'/plugins/demo/web/app.css?v=1'
		]);
	});
	it('falls back to the plugin root for a skills-only plugin with styles', () => {
		expect(styleUrls(info({ web: null, styles: ['app.css'] }))).toEqual(['/plugins/demo/app.css']);
	});
	it('drops anything that could leave the plugin', () => {
		expect(
			styleUrls(info({ styles: ['../../etc/passwd.css', 'https://evil.example/x.css', '//evil/x.css', ''] }))
		).toEqual([]);
	});
	it('is empty when the manifest declares none', () => {
		expect(styleUrls(info())).toEqual([]);
	});
});

describe('ensurePluginStyles', () => {
	beforeEach(() => {
		document.head.innerHTML = '';
	});

	it('links each stylesheet once, however often a plugin is mounted', () => {
		const plugin = info({ styles: ['app.css'] });
		ensurePluginStyles(plugin);
		ensurePluginStyles(plugin);
		const links = [...document.head.querySelectorAll('link[rel="stylesheet"]')];
		expect(links.map((l) => l.getAttribute('href'))).toEqual(['/plugins/demo/web/app.css?v=abcd1234']);
		expect(links[0].getAttribute('data-plugin')).toBe('demo');
	});

	it('links the new URL after an upgrade changed the cache-buster', () => {
		ensurePluginStyles(info({ styles: ['app.css'] }));
		ensurePluginStyles(info({ styles: ['app.css'], web: '/plugins/demo/web/index.js?v=ffff0000' }));
		expect(document.head.querySelectorAll('link[rel="stylesheet"]')).toHaveLength(2);
	});

	it('adds nothing for a plugin without styles', () => {
		ensurePluginStyles(info());
		expect(document.head.querySelectorAll('link')).toHaveLength(0);
	});
});
