// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import Host from './ComposerAttachments.host.test.svelte';
import type { ComposerAttachments, ComposerAttachmentsOpts } from './composerAttachments.svelte';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
});

function fakeSync(
	restored: { files: File[]; missing: string[] } | null = { files: [], missing: [] }
) {
	return {
		restore: vi.fn(async () => restored),
		persist: vi.fn(async () => {}),
		discard: vi.fn(async () => {})
	};
}

async function make(over: Partial<ComposerAttachmentsOpts> = {}) {
	let input = over.input?.() ?? '';
	let ready: ComposerAttachments | undefined;
	const sync = over.sync ?? fakeSync();
	comp = mount(Host, {
		target: document.body,
		props: {
			opts: {
				draftKey: () => 'draft:s1',
				enabled: () => true,
				input: () => input,
				setInput: (t) => (input = t),
				stagedNames: () => [],
				...over,
				sync
			},
			onready: (a) => (ready = a)
		}
	});
	await flush();
	return { a: ready as ComposerAttachments, sync, text: () => input };
}

const file = (name: string, body = 'x') => new File([body], name, { type: 'text/plain' });

describe('ComposerAttachments', () => {
	it('adds files, tokens the draft and persists under the restored key', async () => {
		const { a, sync, text } = await make();
		a.add([file('a.txt')]);
		await flush();
		expect(a.files.map((f) => f.name)).toEqual(['a.txt']);
		expect(text()).toContain('[a.txt]');
		expect(sync.persist).toHaveBeenLastCalledWith('draft:s1', a.files);
	});

	it('removes a file by name', async () => {
		const { a } = await make();
		a.add([file('a.txt'), file('b.txt')]);
		await flush();
		a.remove('a.txt');
		expect(a.files.map((f) => f.name)).toEqual(['b.txt']);
	});

	it('ignores adds while disabled or uploading', async () => {
		const { a } = await make({ enabled: () => false });
		a.add([file('a.txt')]);
		await flush();
		expect(a.files).toEqual([]);
	});

	it('restores a persisted list and drops tokens whose files are gone', async () => {
		const sync = fakeSync({ files: [file('kept.txt')], missing: ['gone.txt'] });
		const { a, text } = await make({ input: () => 'see [kept.txt] and [gone.txt]', sync });
		expect(a.files.map((f) => f.name)).toEqual(['kept.txt']);
		expect(text()).not.toContain('gone.txt');
		expect(text()).toContain('[kept.txt]');
	});

	it('stages uploads, folds the paths under the text and clears the list', async () => {
		const { a, sync } = await make();
		a.add([file('a.txt')]);
		await flush();
		const body = await a.stage('note [a.txt]', async () => ({ paths: ['/tmp/a.txt'] }));
		expect(body).toContain('Attached file:');
		expect(body).toContain('- /tmp/a.txt');
		expect(a.files).toEqual([]);
		expect(a.uploading).toBe(false);
		expect(sync.discard).toHaveBeenCalledWith('draft:s1');
	});

	it('keeps the files when the upload fails', async () => {
		const { a } = await make();
		a.add([file('a.txt')]);
		await flush();
		const body = await a.stage('note', async () => {
			throw new Error('boom');
		});
		expect(body).toBeNull();
		expect(a.files).toHaveLength(1);
		expect(a.uploading).toBe(false);
	});

	it('passes plain text straight through with nothing staged', async () => {
		const { a } = await make();
		await expect(a.stage('hi', async () => ({ paths: [] }))).resolves.toBe('hi');
	});
});
