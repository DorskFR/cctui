// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import UserAttachments from './UserAttachments.svelte';
import type { SessionAttachment } from '$lib/queries/types';

const sid = '0a1b2c3d-1111-2222-3333-444455556666';
const hash = 'f'.repeat(64);

let data: SessionAttachment[] = [];

vi.mock('$lib/queries', () => ({
	useSessionAttachments: () => ({
		get data() {
			return data;
		}
	})
}));

vi.mock('$lib/attachmentStore', () => ({
	attachmentStore: { cachedText: async () => null, cacheText: async () => {} }
}));

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	data = [];
	document.body.innerHTML = '';
});

const png = (over: Partial<SessionAttachment> = {}): SessionAttachment => ({
	id: 'att-1',
	session_id: sid,
	message_id: null,
	name: 'Screenshot 2026-09-25 at 13.23.11.png',
	hash,
	size: 1234,
	content_type: 'image/png',
	created_at: 1_700_000_000_000,
	machine_id: 'mach-1',
	...over
});

async function render(a: SessionAttachment) {
	data = [a];
	comp = mount(UserAttachments, {
		target: document.body,
		props: { refs: { sessionId: sid, names: [a.name] }, ts: a.created_at + 1000 }
	});
	await flush();
}

describe('a user-uploaded image', () => {
	it('renders as a thumbnail pointing at the session blob route', async () => {
		await render(png());
		const img = document.querySelector('.thumb img') as HTMLImageElement | null;
		expect(img, 'an image upload must render a thumbnail, not the chip fallback').not.toBeNull();
		expect(img?.getAttribute('src')).toBe(`/api/v1/sessions/${sid}/blobs/${hash}`);
		expect(document.querySelector('.chip')).toBeNull();
	});

	it('captions the thumbnail with a middle-ellipsized name that keeps the tail', async () => {
		await render(png({ name: 'Screenshot 2026-10-03 at 12.21.15.png' }));
		const caption = document.querySelector('.thumb .caption');
		expect(caption?.textContent).toMatch(/…/);
		expect(caption?.textContent?.endsWith('12.21.15.png')).toBe(true);
		expect(document.querySelector('.thumb')?.getAttribute('title')).toContain(
			'Screenshot 2026-10-03 at 12.21.15.png'
		);
	});

	it('falls back to a file chip only once the blob itself fails to load', async () => {
		await render(png());
		const img = document.querySelector('.thumb img') as HTMLImageElement;
		img.dispatchEvent(new Event('error'));
		await flush();
		expect(document.querySelector('.thumb img')).toBeNull();
		expect(document.querySelector('.chip')?.textContent).toContain('Screenshot');
	});

	it('keeps a non-image upload on the chip', async () => {
		await render(png({ name: 'report.pdf', content_type: 'application/pdf' }));
		expect(document.querySelector('.thumb img')).toBeNull();
		expect(document.querySelector('.chip')?.textContent).toContain('report.pdf');
	});
});
