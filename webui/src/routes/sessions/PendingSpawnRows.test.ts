// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { FailedDispatch } from '@bindings/FailedDispatch';

const { failures } = vi.hoisted(() => ({ failures: { list: [] as FailedDispatch[] } }));
vi.mock('#lib/queries/failedDispatches.ts', async (importOriginal) => {
	const real = await importOriginal<typeof import('#lib/queries/failedDispatches.ts')>();
	return {
		...real,
		useFailedDispatches: () => ({
			get data() {
				return failures.list;
			}
		})
	};
});

import PendingSpawnRows from './PendingSpawnRows.svelte';

const failure = (id: string, over: Partial<FailedDispatch> = {}): FailedDispatch =>
	({
		session_id: id,
		dispatcher: 'kube',
		reason: 'crashed',
		detail: 'OOMKilled',
		dispatched_at: new Date().toISOString(),
		worker_exit: { exit_code: 137, log_tail: 'line 1\nline 2' },
		...over
	}) as FailedDispatch;

let comp: ReturnType<typeof mount> | null = null;
afterEach(async () => {
	if (comp) await unmount(comp);
	comp = null;
	failures.list = [];
	document.body.replaceChildren();
});

function render() {
	comp = mount(PendingSpawnRows, { target: document.body, props: { sp: { items: [] } } });
	flushSync();
}

describe('PendingSpawnRows', () => {
	it('pluralizes the failed dispatch count', () => {
		failures.list = [failure('a')];
		render();
		expect(document.querySelector('a[href="/dispatch-failures"]')?.textContent).toContain(
			'1 failed dispatch →'
		);
		unmount(comp!);
		failures.list = [failure('a'), failure('b')];
		render();
		expect(document.querySelector('a[href="/dispatch-failures"]')?.textContent).toContain(
			'2 failed dispatches →'
		);
	});

	it('links to the failed dispatches instead of rendering their cards', () => {
		failures.list = [
			failure('a'),
			failure('b'),
			failure('gone', { dismissed: true }),
			failure('rerun', { superseded_by: 'next' })
		];
		render();
		expect(document.querySelector('[data-journey="failed-dispatch"]'), 'no failure card').toBeNull();
		expect(document.body.textContent).not.toContain('line 2');
		const link = document.querySelector('a[href="/dispatch-failures"]');
		expect(link?.textContent).toContain('2');
		expect(document.querySelectorAll('a[href="/dispatch-failures"]')).toHaveLength(1);
	});

	it('renders nothing without a pending spawn or an open failure', () => {
		failures.list = [failure('gone', { dismissed: true })];
		render();
		expect(document.querySelector('[data-journey-key="pending"]')).toBeNull();
	});
});
