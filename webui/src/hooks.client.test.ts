// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { handleError, isBenignNotice } from './hooks.client';
import { toasts } from '$lib/toast.svelte';
import { STALE_CHUNK_KEY } from '$lib/staleChunk';

describe('isBenignNotice', () => {
	it('swallows the ResizeObserver loop notice the browser raises as an error', () => {
		expect(isBenignNotice('ResizeObserver loop completed with undelivered notifications.')).toBe(
			true
		);
		expect(isBenignNotice('ResizeObserver loop limit exceeded')).toBe(true);
		expect(isBenignNotice('  ResizeObserver loop completed  ')).toBe(true);
	});

	it('lets real failures through, including ones that merely mention the API', () => {
		expect(isBenignNotice('TypeError: x is not a function')).toBe(false);
		expect(isBenignNotice('ResizeObserver is not defined')).toBe(false);
		expect(isBenignNotice('')).toBe(false);
	});
});

describe('handleError', () => {
	function call(error: unknown) {
		return handleError({ error } as unknown as Parameters<typeof handleError>[0]);
	}

	beforeEach(() => {
		toasts.reset();
		sessionStorage.removeItem(STALE_CHUNK_KEY);
	});

	it('reloads instead of toasting when a deploy took the chunk away', () => {
		const reload = vi.spyOn(location, 'reload').mockImplementation(() => {});
		call(new Error('Failed to fetch dynamically imported module: /_app/immutable/nodes/2.abc.js'));
		expect(reload).toHaveBeenCalledTimes(1);
		expect(toasts.items).toHaveLength(0);
		reload.mockRestore();
	});

	it('toasts the second chunk failure rather than reloading in a loop', () => {
		const reload = vi.spyOn(location, 'reload').mockImplementation(() => {});
		call(new Error('Importing a module script failed.'));
		call(new Error('Importing a module script failed.'));
		expect(reload).toHaveBeenCalledTimes(1);
		expect(toasts.items).toHaveLength(1);
		reload.mockRestore();
	});

	it('still toasts an ordinary failure', () => {
		const reload = vi.spyOn(location, 'reload').mockImplementation(() => {});
		call(new Error('boom'));
		expect(reload).not.toHaveBeenCalled();
		expect(toasts.items).toHaveLength(1);
		expect(toasts.items[0].message).toContain('boom');
		reload.mockRestore();
	});
});
