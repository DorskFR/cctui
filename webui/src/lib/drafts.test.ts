import { beforeEach, describe, expect, it } from 'vitest';
import { attachmentStore } from './attachmentStore';
import { clearSpawnSlot, drafts, promptHistory, SPAWN_SLOT, spawnSlotKey } from './drafts';

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

describe('promptHistory', () => {
	it('stores most-recent-last and ignores empty or whitespace prompts', () => {
		promptHistory.push('first');
		promptHistory.push('  ');
		promptHistory.push('');
		promptHistory.push('  second  ');

		expect(promptHistory.get()).toEqual(['first', 'second']);
	});

	it('moves a repeated prompt to the end instead of duplicating it', () => {
		promptHistory.push('a');
		promptHistory.push('b');
		promptHistory.push('c');
		promptHistory.push('a');

		expect(promptHistory.get()).toEqual(['b', 'c', 'a']);
	});

	it('caps the list, dropping the oldest entries', () => {
		for (let i = 0; i < 20; i++) promptHistory.push(`p${i}`);

		const list = promptHistory.get();
		expect(list.length).toBe(15);
		expect(list[0]).toBe('p5');
		expect(list.at(-1)).toBe('p19');
	});

	it('is global, not per session, and clears', () => {
		promptHistory.push('shared');
		expect(localStorage.getItem('cctui_prompt_history')).toBe('["shared"]');

		promptHistory.clear();
		expect(promptHistory.get()).toEqual([]);
	});

	it('survives a corrupt payload', () => {
		localStorage.setItem('cctui_prompt_history', 'not json');
		expect(promptHistory.get()).toEqual([]);
	});
});
