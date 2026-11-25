// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { StorageLifecycleRules } from '@bindings/StorageLifecycleRules';
import type { StorageOverview } from '@bindings/StorageOverview';

const http = vi.hoisted(() => ({ get: vi.fn(), put: vi.fn(), post: vi.fn() }));
vi.mock('#lib/api.ts', async (importOriginal) => ({
	...(await importOriginal<typeof import('#lib/api.ts')>()),
	api: http
}));
vi.mock('#lib/toast.svelte.ts', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));

import { ApiError } from '#lib/api.ts';
import { m } from '#lib/paraglide/messages.js';
import ServerStorageSection from './ServerStorageSection.svelte';
import SectionHost from './section.host.test.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.replaceChildren();
	vi.clearAllMocks();
});

const settle = async () => {
	for (let i = 0; i < 6; i++) await tick();
	flushSync();
};
const button = (text: string) =>
	[...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === text) as HTMLButtonElement;
const text = () => document.body.textContent ?? '';

const overview = (rules: StorageLifecycleRules = { trim: null, drop: null }): StorageOverview => ({
	database_bytes: 10 * 1024 ** 3,
	tables: [
		{
			name: 'stream_events',
			total_bytes: 9 * 1024 ** 3,
			table_bytes: 1024 ** 3,
			index_bytes: 1024 ** 3,
			toast_bytes: 7 * 1024 ** 3,
			row_estimate: 2_600_000
		}
	],
	images: { blobs: 12, image_bytes: 3 * 1024 ** 2, table_bytes: 4 * 1024 ** 2 },
	sample: {
		percent: 0.5,
		rows_sampled: 13_000,
		sessions: [
			{
				session_id: 'abcdef123456',
				name: 'big session',
				harness: 'claude-code',
				machine: 'box',
				machine_kind: 'persistent',
				events: 40_000,
				bytes: 2 * 1024 ** 3,
				tool_output_bytes: 1024 ** 3
			}
		],
		tools: [{ tool: 'Bash', results: 9000, bytes: 512 * 1024 ** 2 }],
		by_size: [{ key: 'ge_1m', results: 10, bytes: 50 * 1024 ** 2 }],
		by_age: [{ key: 'ge_90d', results: 10, bytes: 50 * 1024 ** 2 }]
	},
	sample_error: null,
	lifecycle: { rules, last_run: null, bytes_freed_total: 0 }
});

const render = async () => {
	comp = mount(SectionHost, { target: document.body, props: { section: ServerStorageSection } });
	await settle();
};

describe('ServerStorageSection', () => {
	it('shows catalog sizes, images and the sampled breakdown, with nothing enabled', async () => {
		http.get.mockResolvedValue(overview());
		await render();
		expect(http.get).toHaveBeenCalledWith('/admin/storage');
		for (const shown of ['10 GB', 'stream_events', '3.0 MB', 'big session', 'Bash', '512 MB']) {
			expect(text()).toContain(shown);
		}
		expect(document.querySelector('[data-testid="storage-policy"]')?.textContent?.trim()).toBe(
			m.settings_server_storage_lifecycle_keep_all()
		);
		expect(text()).toContain(m.settings_server_storage_last_run_never());
	});

	it('tells a non-admin the page is admin-only', async () => {
		http.get.mockRejectedValue(new ApiError(403, 'admin token required'));
		await render();
		expect(text()).toContain(m.settings_server_storage_forbidden());
	});

	it('saves a rule only after previewing it', async () => {
		http.get.mockResolvedValue(overview());
		http.post.mockResolvedValue({ percent: 0.5, rows_trimmed: 1200, rows_dropped: 0, bytes_freed: 3 * 1024 ** 3 });
		http.put.mockResolvedValue({
			rules: { trim: { enabled: true, after_days: 30, above_bytes: 16 * 1024 }, drop: null },
			last_run: null,
			bytes_freed_total: 0
		});
		await render();
		const save = () => button(m.settings_server_storage_apply());
		expect(save()).toBeUndefined();

		document.querySelector<HTMLElement>('[role="switch"]')?.click();
		await settle();
		expect(save().disabled).toBe(true);

		button(m.settings_server_storage_preview_button()).click();
		await settle();
		const rules = { trim: { enabled: true, after_days: 30, above_bytes: 16 * 1024 }, drop: null };
		expect(http.post).toHaveBeenCalledWith('/admin/storage/lifecycle/preview', rules);
		expect(document.querySelector('[data-testid="storage-preview"]')?.textContent).toContain('3.0 GB');
		expect(save().disabled).toBe(false);

		save().click();
		await settle();
		expect(http.put).toHaveBeenCalledWith('/admin/storage/lifecycle', rules);
		expect(document.querySelector('[data-testid="storage-policy"]')?.textContent?.trim()).toBe(
			m.settings_server_storage_lifecycle_active()
		);
	});

	it('saves switching every rule off without a preview or Apply', async () => {
		const on = { trim: { enabled: true, after_days: 30, above_bytes: 16 * 1024 }, drop: null };
		http.get.mockResolvedValue(overview(on));
		http.put.mockResolvedValue({ rules: { trim: null, drop: null }, last_run: null, bytes_freed_total: 0 });
		await render();
		vi.useFakeTimers();
		try {
			expect(button(m.settings_server_storage_apply())).toBeDefined();
			document.querySelector<HTMLElement>('[role="switch"]')?.click();
			await vi.advanceTimersByTimeAsync(700);
			expect(http.post).not.toHaveBeenCalled();
			expect(http.put).toHaveBeenCalledTimes(1);
		} finally {
			vi.useRealTimers();
		}
	});
});
