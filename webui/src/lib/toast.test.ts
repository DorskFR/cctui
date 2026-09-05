import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { toasts } from './toast.svelte';

describe('toasts (kit store re-exported for the webui)', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		for (const t of [...toasts.items]) toasts.dismiss(t.id);
	});
	afterEach(() => vi.useRealTimers());

	it('keeps an action toast on screen longer than a plain one', () => {
		toasts.ok('plain');
		toasts.ok('undoable', undefined, { label: 'Undo', run: () => {} });
		expect(toasts.items).toHaveLength(2);
		vi.advanceTimersByTime(4000);
		expect(toasts.items.map((t) => t.message)).toEqual(['undoable']);
		vi.advanceTimersByTime(3000);
		expect(toasts.items).toHaveLength(0);
	});

	it('runs the action once and dismisses the toast when it settles', async () => {
		const run = vi.fn();
		const id = toasts.ok('archived', undefined, { label: 'Undo', run });
		void toasts.act(id);
		void toasts.act(id);
		await vi.runAllTimersAsync();
		expect(run).toHaveBeenCalledTimes(1);
		expect(toasts.items).toHaveLength(0);
	});

	it('surfaces a failing action as an error toast', async () => {
		const id = toasts.ok('archived', undefined, {
			label: 'Undo',
			run: async () => {
				throw new Error('boom');
			}
		});
		void toasts.act(id);
		await vi.advanceTimersByTimeAsync(0);
		expect(toasts.items.map((t) => [t.tone, t.message])).toEqual([['error', 'boom']]);
	});
});
