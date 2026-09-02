import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ACTION_TOAST_MS, toasts } from './toast.svelte';

describe('toasts with an inline action', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		for (const t of [...toasts.items]) toasts.dismiss(t.id);
	});
	afterEach(() => vi.useRealTimers());

	it('keeps an action toast on screen longer than a plain one', () => {
		toasts.ok('plain');
		toasts.ok('undoable', { label: 'Undo', run: () => {} });
		expect(toasts.items).toHaveLength(2);
		vi.advanceTimersByTime(3500);
		expect(toasts.items.map((t) => t.text)).toEqual(['undoable']);
		vi.advanceTimersByTime(ACTION_TOAST_MS - 3500);
		expect(toasts.items).toHaveLength(0);
	});

	it('runs the action once and dismisses the toast first', async () => {
		const run = vi.fn();
		const id = toasts.ok('archived', { label: 'Undo', run });
		toasts.act(id);
		toasts.act(id); // second tap: toast already gone, must be a no-op
		await vi.runAllTimersAsync();
		expect(run).toHaveBeenCalledTimes(1);
		expect(toasts.items).toHaveLength(0);
	});

	it('surfaces a failing action as an error toast', async () => {
		const id = toasts.ok('archived', {
			label: 'Undo',
			run: async () => {
				throw new Error('boom');
			}
		});
		toasts.act(id);
		await vi.advanceTimersByTimeAsync(0); // flush the action promise chain only
		expect(toasts.items.map((t) => [t.kind, t.text])).toEqual([['err', 'boom']]);
	});
});
