import { describe, expect, it, vi } from 'vitest';
import {
	enabledWebPlugins,
	isPluginId,
	loadPluginModule,
	PluginModuleError,
	resetPluginModuleCache,
	validatePluginModule
} from './discovery';
import type { PluginInfo } from './types';

const info = (over: Partial<PluginInfo> = {}): PluginInfo => ({
	id: 'demo',
	name: 'Demo',
	description: '',
	version: '1.0.0',
	icon: null,
	web: '/plugins/demo/web/index.js?v=abcd1234',
	skills: [],
	enabled: false,
	settings: [],
	config: {},
	...over
});

describe('plugin ids', () => {
	it('accepts the manifest id shape only', () => {
		expect(isPluginId('yubisashi')).toBe(true);
		expect(isPluginId('a-1')).toBe(true);
		expect(isPluginId('Bad')).toBe(false);
		expect(isPluginId('has space')).toBe(false);
		expect(isPluginId('')).toBe(false);
		expect(isPluginId('x'.repeat(41))).toBe(false);
		expect(isPluginId(1)).toBe(false);
	});
});

describe('enabledWebPlugins', () => {
	it('keeps only switched-on plugins that ship a web bundle', () => {
		const list = [info(), info({ id: 'skills', web: null }), info({ id: 'off' })];
		expect(enabledWebPlugins(list, { demo: true, skills: true }).map((p) => p.id)).toEqual(['demo']);
		expect(enabledWebPlugins(list, {})).toEqual([]);
	});
});

describe('validatePluginModule', () => {
	const pane = () => {};
	it('accepts a v1 module', () => {
		const mod = { default: { cctuiApi: 1, sessionPane: pane, messageActions: () => [] } };
		expect(validatePluginModule(mod)).toBe(mod.default);
		expect(validatePluginModule({ default: { cctuiApi: 1 } })).toEqual({ cctuiApi: 1 });
	});
	it('refuses a missing default export or another API major', () => {
		expect(() => validatePluginModule({})).toThrow(PluginModuleError);
		expect(() => validatePluginModule({ default: 3 })).toThrow(PluginModuleError);
		expect(() => validatePluginModule({ default: { cctuiApi: 2 } })).toThrow(/cctuiApi 2, host is 1/);
		expect(() => validatePluginModule({ default: { cctuiApi: '1' } })).toThrow(PluginModuleError);
	});
	it('refuses malformed contributions', () => {
		expect(() => validatePluginModule({ default: { cctuiApi: 1, sessionPane: {} } })).toThrow(/sessionPane/);
		expect(() => validatePluginModule({ default: { cctuiApi: 1, messageActions: 'x' } })).toThrow(/messageActions/);
	});
});

describe('loadPluginModule', () => {
	it('imports each bundle URL once and validates it', async () => {
		resetPluginModuleCache();
		const importer = vi.fn(async () => ({ default: { cctuiApi: 1 } }));
		const a = await loadPluginModule('/plugins/a/web/index.js?v=1', importer);
		const b = await loadPluginModule('/plugins/a/web/index.js?v=1', importer);
		expect(a).toBe(b);
		expect(importer).toHaveBeenCalledTimes(1);
		await loadPluginModule('/plugins/a/web/index.js?v=2', importer);
		expect(importer).toHaveBeenCalledTimes(2);
	});
	it('does not cache a failed load', async () => {
		resetPluginModuleCache();
		const importer = vi
			.fn<(url: string) => Promise<unknown>>()
			.mockRejectedValueOnce(new Error('404'))
			.mockResolvedValueOnce({ default: { cctuiApi: 1 } });
		await expect(loadPluginModule('/plugins/x/web/index.js', importer)).rejects.toThrow('404');
		await expect(loadPluginModule('/plugins/x/web/index.js', importer)).resolves.toEqual({ cctuiApi: 1 });
	});
});
