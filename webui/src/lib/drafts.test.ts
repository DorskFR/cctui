import { beforeEach, describe, expect, it } from 'vitest';
import { attachmentStore } from './attachmentStore';
import { clearSpawnSlot, drafts, SPAWN_SLOT, spawnSlotKey } from './drafts';

const file = (name: string) => new File(['xxx'], name, { type: 'text/plain' });

beforeEach(async () => {
	localStorage.clear();
	await attachmentStore.clearAll();
});

describe('clearSpawnSlot', () => {
	it('drops the slot payload, its files and the resume pointer', async () => {
		const key = spawnSlotKey('m1', '/repo/');
		drafts.set(key, '{"prompt":"hi"}');
		drafts.set(SPAWN_SLOT, key);
		await attachmentStore.set(key, [file('a.txt')]);

		clearSpawnSlot('m1', '/repo');

		expect(drafts.get(key)).toBe('');
		expect(drafts.get(SPAWN_SLOT)).toBe('');
		expect((await attachmentStore.get(key)).files).toEqual([]);
	});

	it('leaves another target slot and pointer alone', async () => {
		const mine = spawnSlotKey('m1', '/repo');
		const other = spawnSlotKey('m2', '/repo');
		drafts.set(other, '{"prompt":"keep"}');
		drafts.set(SPAWN_SLOT, other);
		await attachmentStore.set(other, [file('b.txt')]);

		clearSpawnSlot('m1', '/repo');

		expect(drafts.get(mine)).toBe('');
		expect(drafts.get(other)).toBe('{"prompt":"keep"}');
		expect(drafts.get(SPAWN_SLOT)).toBe(other);
		expect((await attachmentStore.get(other)).files.length).toBe(1);
	});
});
