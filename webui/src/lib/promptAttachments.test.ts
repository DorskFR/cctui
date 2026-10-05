// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import Host from './PromptAttachments.host.test.svelte';
import type { PromptAttachments, PromptAttachmentsOpts } from './promptAttachments.svelte';

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

async function make(over: Partial<PromptAttachmentsOpts> = {}) {
	let input = over.input?.() ?? '';
	let ready: PromptAttachments | undefined;
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
	return { a: ready as PromptAttachments, sync, text: () => input };
}

const file = (name: string, body = 'x') => new File([body], name, { type: 'text/plain' });
const image = (name: string) => new File(['x'], name, { type: 'image/png' });

const pasteEvent = (text: string) =>
	({
		preventDefault: () => {},
		clipboardData: { items: [], files: [], getData: () => text }
	}) as unknown as ClipboardEvent;

describe('PromptAttachments', () => {
	it('marks an added file in the draft and persists under the restored key', async () => {
		const { a, sync, text } = await make({ input: () => 'note' });
		a.add([file('a.txt')]);
		await flush();
		expect(a.files.map((f) => f.name)).toEqual(['a.txt']);
		expect(text()).toBe('note [a.txt]');
		expect(sync.persist).toHaveBeenLastCalledWith('draft:s1', a.files);
	});

	it('inserts the file names at the caret of the textarea', async () => {
		const el = document.body.appendChild(document.createElement('textarea'));
		el.value = 'first point. second point.';
		el.setSelectionRange(12, 12);
		let draft = el.value;
		const { a } = await make({ input: () => draft, setInput: (t) => (draft = t), el: () => el });
		a.add([file('one.png'), file('two.png')]);
		await flush();
		expect(draft).toBe('first point. [one.png] [two.png] second point.');
		el.remove();
	});

	it('sends each file reference where the user put it, under its staged name', async () => {
		const { a } = await make();
		a.add([file('a.txt'), file('b.txt')]);
		await flush();
		const body = await a.stage('b here [b.txt], a here [a.txt]', async () => ({
			paths: ['/tmp/a.txt', '/tmp/b-1.txt']
		}));
		expect(body).toBe(
			'b here [b-1.txt], a here [a.txt]\n\nAttached files (2):\n- /tmp/a.txt\n- /tmp/b-1.txt'
		);
	});

	it('still lists a file whose name the user deleted from the text', async () => {
		const { a } = await make();
		a.add([file('a.txt'), file('b.txt')]);
		await flush();
		const body = await a.stage('look [b.txt]', async () => ({ paths: ['/tmp/a.txt', '/tmp/b.txt'] }));
		expect(body).toBe('look [b.txt]\n\nAttached files (2):\n- /tmp/a.txt\n- /tmp/b.txt');
	});

	it('leads the sent body with image names, which Claude keeps once it eats the paths', async () => {
		const { a } = await make();
		a.add([image('shot.png')]);
		await flush();
		const body = await a.stage('', async () => ({ paths: ['/tmp/cctui-uploads/s1/shot.png'] }));
		expect(body).toBe('[shot.png]\n\nAttached file:\n- /tmp/cctui-uploads/s1/shot.png');
	});

	it('tokens the draft for a masked large paste', async () => {
		const { a, text } = await make();
		a.onPaste(pasteEvent('y'.repeat(2500)));
		await flush();
		expect(a.files.map((f) => f.name)).toEqual(['paste-1.txt']);
		expect(text()).toContain('[paste-1.txt]');
	});

	it('removes a file and its name from the text', async () => {
		const { a, text } = await make();
		a.add([file('a.txt'), file('b.txt')]);
		await flush();
		a.remove('a.txt');
		expect(a.files.map((f) => f.name)).toEqual(['b.txt']);
		expect(text()).toBe('[b.txt]');
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

	it('keeps a file attached while the list was still restoring', async () => {
		let resolve!: (r: { files: File[]; missing: string[] }) => void;
		const sync = fakeSync();
		sync.restore.mockImplementation(() => new Promise((r) => (resolve = r)));
		const { a } = await make({ sync });
		a.add([file('pasted.txt')]);
		await flush();
		resolve({ files: [file('restored.txt')], missing: [] });
		await flush();
		expect(a.files.map((f) => f.name)).toEqual(['restored.txt', 'pasted.txt']);
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
