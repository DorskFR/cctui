import { describe, expect, it, vi } from 'vitest';

const slots = new Map<string, object>();
vi.mock('$lib/drafts', () => ({
	drafts: { get: () => '' },
	readSpawnSlot: (key: string) => slots.get(key) ?? null,
	LAST_SPAWN_NAME: 'name',
	LAST_SPAWN_LABELS: 'labels',
	nextSessionName: (n: string) => n
}));

import { seedForm } from './spawnSeed';

describe('seedForm', () => {
	it('restores the draft but leaves context picks to the filters of this open', () => {
		slots.set('slot', {
			working_dir: '/other/project',
			prompt: 'hi',
			context_items: ['facts-for-the-old-project'],
			context_auto: false
		});
		const { form } = seedForm(null, 'slot');
		expect(form.working_dir).toBe('/other/project');
		expect(form.prompt).toBe('hi');
		expect(form.context_items).toEqual([]);
		expect(form.context_auto).toBe(true);
	});
});
