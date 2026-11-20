// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import Host from './PromptAttachments.host.test.svelte';
import type { PromptAttachments } from './promptAttachments.svelte';
import { setUploadCaps } from './uploadCaps.svelte';
import { toasts } from './toast.svelte';
import { m } from './paraglide/messages.js';

vi.mock('./compressImage', async (orig) => ({
	...(await orig<typeof import('./compressImage')>()),
	compressImage: vi.fn(async () => {
		throw new Error('decode');
	})
}));

const flush = () => new Promise((r) => setTimeout(r, 0));
const CAP = 1024;

let comp: ReturnType<typeof mount> | null = null;
beforeEach(() => {
	toasts.reset();
	setUploadCaps({ max_files: 10, max_file_bytes: CAP, max_total_bytes: 10 * CAP });
});
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	setUploadCaps(null);
	toasts.reset();
});

async function make() {
	let input = '';
	let ready: PromptAttachments | undefined;
	comp = mount(Host, {
		target: document.body,
		props: {
			opts: { input: () => input, setInput: (t: string) => (input = t) },
			onready: (a: PromptAttachments) => (ready = a)
		}
	});
	await flush();
	return { a: ready as PromptAttachments, text: () => input };
}

const sized = (name: string, size: number, type: string) =>
	new File([new Uint8Array(size)], name, { type });
const errors = () => toasts.items.filter((t) => t.tone === 'error').map((t) => t.message);

describe('PromptAttachments with camera captures', () => {
	it('attaches a camera photo that arrives with no MIME type, typed from its extension', async () => {
		const { a, text } = await make();
		a.add([sized('1731900000000.jpg', 100, '')]);
		await flush();
		expect(a.files.map((f) => [f.name, f.type])).toEqual([['1731900000000.jpg', 'image/jpeg']]);
		expect(text()).toBe('[1731900000000.jpg]');
		expect(errors()).toEqual([]);
	});

	it('attaches a HEIC photo under the cap as-is', async () => {
		const { a } = await make();
		a.add([sized('IMG_0001.HEIC', 100, 'image/heic')]);
		await flush();
		expect(a.files.map((f) => [f.name, f.type])).toEqual([['IMG_0001.HEIC', 'image/heic']]);
		expect(a.error).toBe('');
	});

	it('names a nameless capture so staging accepts it', async () => {
		const { a } = await make();
		a.add([sized('', 100, 'image/jpeg')]);
		await flush();
		expect(a.files.map((f) => f.name)).toEqual(['photo.jpg']);
	});

	it('says why an oversized HEIC photo the browser cannot convert was not attached', async () => {
		const { a } = await make();
		a.add([sized('IMG_0002.heic', CAP + 1, 'image/heic')]);
		await vi.waitFor(() => expect(a.images.pending).toHaveLength(0));
		expect(a.files).toEqual([]);
		expect(errors()).toEqual([m.attach_heic_unsupported({ name: 'IMG_0002.heic' })]);
	});

	it('flags an oversized file that cannot be compressed instead of dropping it silently', async () => {
		const { a } = await make();
		a.add([sized('dump.bin', CAP + 1, 'application/octet-stream')]);
		await flush();
		expect(a.files.map((f) => f.name)).toEqual(['dump.bin']);
		expect(a.error).toContain('dump.bin');
		expect(errors()).toEqual([m.attach_too_large({ name: 'dump.bin', cap: '1 KB' })]);
	});

	it('tells the user a pick landing mid-upload was not attached', async () => {
		const { a } = await make();
		a.uploading = true;
		a.add([sized('photo.jpg', 100, 'image/jpeg')]);
		await flush();
		expect(a.files).toEqual([]);
		expect(errors()).toEqual([m.attach_busy()]);
	});
});
