import { describe, expect, it, vi } from 'vitest';

const slots = new Map<string, object>();
vi.mock('#lib/drafts.ts', () => ({
	drafts: { get: () => '' },
	readSpawnSlot: (key: string) => slots.get(key) ?? null,
	LAST_SPAWN_NAME: 'name',
	LAST_SPAWN_LABELS: 'labels',
	nextSessionName: (n: string) => n,
	normalizeDir: (d: string) => d
}));

import { buildSpawnBody } from './spawnBody';
import { seedForm } from './spawnSeed';

describe('seedForm', () => {
	it('restores the draft but leaves context picks to the filters of this open', () => {
		slots.set('slot', {
			working_dir: '/other/project',
			prompt: 'hi',
			context_items: ['facts-for-the-old-project'],
			context_auto: true
		});
		const { form } = seedForm(null, 'slot');
		expect(form.working_dir).toBe('/other/project');
		expect(form.prompt).toBe('hi');
		expect(form.context_items).toEqual([]);
		expect(form.context_auto).toBe(false);
	});

	it('sends only the explicit picks and never asks for matching memories', () => {
		const { form } = seedForm(null, 'fresh');
		expect(buildSpawnBody(form, undefined, {}, null).context).toEqual({ items: [], auto: false });
		form.context_items = ['picked'];
		expect(buildSpawnBody(form, undefined, {}, null).context).toEqual({
			items: ['picked'],
			auto: false
		});
	});

	it('sends auto-archive only once ticked', () => {
		const { form } = seedForm(null, 'fresh');
		expect(buildSpawnBody(form, undefined, {}, null).auto_archive).toBe(false);
		expect(buildSpawnBody({ ...form, auto_archive: true }, undefined, {}, null).auto_archive).toBe(true);
	});

	it('reads auto-archive from the slot unless the prefill names it', () => {
		slots.set('armed', { working_dir: '/w', auto_archive: true });
		expect(seedForm(null, 'armed').form.auto_archive).toBe(true);
		expect(seedForm({ working_dir: '/w', auto_archive: '0' }, 'armed').form.auto_archive).toBe(false);
		slots.set('junk', { working_dir: '/w', auto_archive: 'yes' });
		expect(seedForm(null, 'junk').form.auto_archive).toBe(false);
	});
});
