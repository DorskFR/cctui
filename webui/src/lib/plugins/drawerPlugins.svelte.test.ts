import { flushSync } from 'svelte';
import { describe, expect, it } from 'vitest';
import { DrawerPlugins } from './drawerPlugins.svelte';
import { PluginLoader } from './loader.svelte';
import type { CctuiPluginModule, PluginInfo } from './types';
import type { Line } from '$lib/components/organisms/conversation/types';

const info = (id: string, web: string | null = `/plugins/${id}/web/index.js`): PluginInfo => ({
	id,
	name: `${id} name`,
	description: '',
	version: '1',
	icon: 'eye',
	web,
	skills: [],
	enabled: true,
	settings: [],
	config: {}
});
const pane = (() => {}) as unknown as CctuiPluginModule['sessionPane'];
const modules: Record<string, CctuiPluginModule> = {
	'/plugins/review/web/index.js': {
		cctuiApi: 1,
		sessionPane: pane,
		messageActions: (m) =>
			m.text.includes('review:') ? [{ label: 'Open', params: { url: m.text.slice(7) }, open: 'sessionPane', autoOpen: true }] : []
	},
	'/plugins/broken/web/index.js': { cctuiApi: 1 }
};
const line = (text: string, role: Line['role'] = 'assistant'): Line => ({ role, ts: 1, text });
const flush = () => new Promise((r) => setTimeout(r, 0));

async function setup(enabled: Record<string, boolean> = { review: true, broken: true }) {
	const loader = new PluginLoader(async (web) => {
		const m = modules[web];
		if (!m) throw new Error('missing');
		return m;
	});
	const dp = new DrawerPlugins(
		{ sessionId: () => 's1', installed: () => [info('review'), info('broken'), info('skills', null)], enabled: () => enabled },
		loader
	);
	dp.ensureLoaded();
	await flush();
	flushSync();
	return dp;
}

describe('DrawerPlugins', () => {
	it('loads enabled web plugins and lists a button per ready pane', async () => {
		const dp = await setup();
		expect(dp.enabledList.map((p) => p.id)).toEqual(['review', 'broken']);
		expect(dp.ready.map((r) => r.info.id)).toEqual(['review', 'broken']);
		expect(dp.buttons.map((b) => [b.id, b.label, b.icon, b.open])).toEqual([['review', 'review name', 'eye', false]]);
	});
	it('lists nothing while everything is off', async () => {
		const dp = await setup({});
		expect(dp.buttons).toEqual([]);
		expect(dp.actionsFor(line('review:https://a'))).toEqual([]);
	});
	it('toggles the pane from the header and opens it with params from an action', async () => {
		const dp = await setup();
		dp.buttons[0].onselect();
		expect(dp.open).toEqual({ id: 'review', params: {} });
		expect(dp.current?.info.id).toBe('review');
		dp.buttons[0].onselect();
		expect(dp.open).toBeNull();
		const [a] = dp.actionsFor(line('review:https://a'));
		dp.openWith(a.pluginId, a.params);
		expect(dp.open).toEqual({ id: 'review', params: { url: 'https://a' } });
		dp.close();
		expect(dp.current).toBeNull();
	});
	it('auto-opens once for the newest assistant line, never over an open pane', async () => {
		const dp = await setup();
		const lines = [line('review:https://old'), line('review:https://new'), line('review:https://user', 'user')];
		dp.autoOpenFrom(lines);
		expect(dp.open).toEqual({ id: 'review', params: { url: 'https://new' } });
		dp.close();
		dp.autoOpenFrom(lines);
		expect(dp.open).toBeNull();
		dp.autoOpenFrom([...lines, line('review:https://next')]);
		expect(dp.open?.params).toEqual({ url: 'https://next' });
		dp.autoOpenFrom([...lines, line('review:https://next'), line('review:https://blocked')]);
		expect(dp.open?.params).toEqual({ url: 'https://next' });
	});
});
