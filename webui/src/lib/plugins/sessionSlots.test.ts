import { describe, expect, it } from 'vitest';
import {
	pluginChipRenderer,
	pluginChips,
	readPluginSlot,
	readPluginSlots,
	registerPluginChipRenderer,
	registeredChipRenderers,
	youtrackChip
} from './sessionSlots';

describe('readPluginSlots', () => {
	it('reads the object slots under metadata.plugins', () => {
		const meta = { draft: {}, plugins: { youtrack: { issue: 'CCT-910' }, slack: { ts: '1' } } };
		expect(readPluginSlots(meta)).toEqual({ youtrack: { issue: 'CCT-910' }, slack: { ts: '1' } });
		expect(readPluginSlot(meta, 'youtrack')).toEqual({ issue: 'CCT-910' });
		expect(readPluginSlot(meta, 'github')).toBeNull();
	});

	it('survives every shape a hand-edited row can be in', () => {
		for (const meta of [null, undefined, 'junk', 7, [], { plugins: null }, { plugins: 'x' }, { plugins: [] }]) {
			expect(readPluginSlots(meta)).toEqual({});
		}
	});

	it('drops non-object slot values', () => {
		expect(readPluginSlots({ plugins: { youtrack: 'CCT-910', slack: { ts: '1' } } })).toEqual({
			slack: { ts: '1' }
		});
	});
});

describe('renderer registry', () => {
	it('ships the youtrack renderer', () => {
		expect(registeredChipRenderers()).toContain('youtrack');
		expect(pluginChipRenderer('youtrack')).toBe(youtrackChip);
		expect(pluginChipRenderer('nope')).toBeUndefined();
	});

	it('renders only slots that have a renderer', () => {
		const chips = pluginChips({ plugins: { youtrack: { issue: 'CCT-910' }, mystery: { a: 1 } } });
		expect(chips.map((c) => c.pluginId)).toEqual(['youtrack']);
	});

	it('lets a new plugin add its own chip', () => {
		registerPluginChipRenderer('demo', (d) => ({
			label: String(d.tag),
			title: 'demo',
			href: null,
			icon: 'tag'
		}));
		const chips = pluginChips({ plugins: { demo: { tag: 'X' }, youtrack: { issue: 'CCT-1' } } });
		expect(chips.map((c) => `${c.pluginId}:${c.label}`)).toEqual(['demo:X', 'youtrack:CCT-1']);
	});

	it('skips a renderer that throws instead of losing the whole row', () => {
		registerPluginChipRenderer('boom', () => {
			throw new Error('nope');
		});
		const chips = pluginChips({ plugins: { boom: {}, youtrack: { issue: 'CCT-1' } } });
		expect(chips.map((c) => c.pluginId)).toEqual(['youtrack']);
	});
});

describe('youtrack renderer', () => {
	it('needs an issue id and nothing else', () => {
		expect(youtrackChip({})).toBeNull();
		expect(youtrackChip({ issue: '  ' })).toBeNull();
		expect(youtrackChip({ issue: 'CCT-910' })).toEqual({
			label: 'CCT-910',
			title: 'CCT-910',
			href: null,
			icon: 'tag'
		});
	});

	it('puts the summary and state in the tooltip and links the stored url', () => {
		const chip = youtrackChip({
			issue: 'CCT-910',
			summary: 'session plugins slot',
			state: 'In progress',
			url: 'https://youtrack.example/issue/CCT-910'
		});
		expect(chip?.title).toBe('CCT-910\nsession plugins slot\nIn progress');
		expect(chip?.href).toBe('https://youtrack.example/issue/CCT-910');
	});

	it('renders stored data with no lookup at all', () => {
		const chips = pluginChips({
			plugins: { youtrack: { issue: 'CCT-910', summary: 'kept', state: 'Done' } }
		});
		expect(chips[0].title).toBe('CCT-910\nkept\nDone');
	});
});
