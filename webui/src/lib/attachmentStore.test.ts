import { beforeEach, describe, expect, it } from 'vitest';
import { attachmentStore, dropMissingTokens } from './attachmentStore';
import { MAX_TOTAL_BYTES } from './attachments';

const KEY = 'cctui_spawn_draft';
const file = (name: string, size = 3) => new File(['x'.repeat(size)], name, { type: 'text/plain' });

beforeEach(async () => {
	await attachmentStore.clearAll();
});

describe('attachmentStore', () => {
	it('round-trips files under a draft key', async () => {
		await attachmentStore.set(KEY, [file('a.txt'), file('b.png')]);
		const r = await attachmentStore.get(KEY);
		expect(r.files.map((f) => f.name)).toEqual(['a.txt', 'b.png']);
		expect(r.missing).toEqual([]);
	});

	it('empty list removes the record', async () => {
		await attachmentStore.set(KEY, [file('a.txt')]);
		await attachmentStore.set(KEY, []);
		expect(await attachmentStore.get(KEY)).toEqual({ files: [], missing: [] });
	});

	it('over-cap lists keep names only, reported as missing', async () => {
		await attachmentStore.set(KEY, [file('big.bin', MAX_TOTAL_BYTES + 1)]);
		const r = await attachmentStore.get(KEY);
		expect(r.files).toEqual([]);
		expect(r.missing).toEqual(['big.bin']);
	});

	it('clear and clearAll drop records', async () => {
		await attachmentStore.set(KEY, [file('a.txt')]);
		await attachmentStore.set('cctui_draft_s1', [file('b.txt')]);
		await attachmentStore.clear(KEY);
		expect((await attachmentStore.get(KEY)).files).toEqual([]);
		expect((await attachmentStore.get('cctui_draft_s1')).files.length).toBe(1);
		await attachmentStore.clearAll();
		expect((await attachmentStore.get('cctui_draft_s1')).files).toEqual([]);
	});
});

describe('dropMissingTokens', () => {
	it('removes only tokens of missing files', () => {
		const r = dropMissingTokens('fix [a.txt] and [b.txt] now [keep]', ['b.txt']);
		expect(r).toEqual({ text: 'fix [a.txt] and now [keep]', dropped: 1 });
	});

	it('is a no-op without missing names', () => {
		const text = 'see [a.txt]';
		expect(dropMissingTokens(text, [])).toEqual({ text, dropped: 0 });
	});

	it('trims a trailing token', () => {
		expect(dropMissingTokens('prompt [paste-1.txt]', ['paste-1.txt']).text).toBe('prompt');
	});
});
