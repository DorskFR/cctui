// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { handleError, isBenignNotice, isKitScrollCaptureError } from './hooks.client';
import { toasts } from '#lib/toast.svelte.ts';
import { STALE_CHUNK_KEY } from '#lib/staleChunk.ts';

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

const firefoxScroll = `can't access property "scroll", F[e] is undefined`;

describe('isKitScrollCaptureError', () => {
	it('matches the router scroll capture TypeError in each engine', () => {
		expect(isKitScrollCaptureError(new TypeError(firefoxScroll))).toBe(true);
		expect(
			isKitScrollCaptureError(new TypeError("Cannot set properties of undefined (setting 'scroll')"))
		).toBe(true);
		expect(
			isKitScrollCaptureError(new TypeError("undefined is not an object (evaluating 'F[e].scroll')"))
		).toBe(true);
		expect(isKitScrollCaptureError(`Uncaught TypeError: ${firefoxScroll}`)).toBe(true);
	});

	it('lets other TypeErrors and non-TypeErrors through', () => {
		expect(isKitScrollCaptureError(new TypeError(`can't access property "id", s is undefined`))).toBe(
			false
		);
		expect(
			isKitScrollCaptureError(new TypeError(`can't access property "scroll", el is undefined`))
		).toBe(false);
		expect(isKitScrollCaptureError(new Error(firefoxScroll))).toBe(false);
		expect(isKitScrollCaptureError(undefined)).toBe(false);
	});
});

describe('handleError', () => {
	function call(error: unknown, kind = 'unknown') {
		return handleError({ error, kind } as unknown as Parameters<typeof handleError>[0]);
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

	it('leaves an unknown route to the error page instead of toasting it', () => {
		call({ status: 404, message: 'Not found: /nope' }, 'framework');
		expect(toasts.items).toHaveLength(0);
	});

	it('still toasts an ordinary failure', () => {
		const reload = vi.spyOn(location, 'reload').mockImplementation(() => {});
		call(new Error('boom'));
		expect(reload).not.toHaveBeenCalled();
		expect(toasts.items).toHaveLength(1);
		expect(toasts.items[0].message).toContain('boom');
		reload.mockRestore();
	});

	it('logs the router scroll capture error instead of toasting it', () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		call(new TypeError(firefoxScroll));
		expect(toasts.items).toHaveLength(0);
		expect(warn).toHaveBeenCalledTimes(1);
		warn.mockRestore();
	});

	it('still toasts an unrelated TypeError', () => {
		call(new TypeError(`can't access property "id", s is undefined`));
		expect(toasts.items).toHaveLength(1);
	});

	it('leaves an expected error(...) to the error page without a toast', () => {
		expect(call({ status: 404, body: { message: 'Not Found' } }, 'app')).toBeUndefined();
		expect(toasts.items).toHaveLength(0);
	});
});

describe('window error listener', () => {
	beforeEach(() => toasts.reset());

	function raise(error: Error) {
		window.dispatchEvent(new ErrorEvent('error', { error, message: `TypeError: ${error.message}` }));
	}

	it('keeps the router scroll capture error out of the toasts', () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		raise(new TypeError(firefoxScroll));
		expect(toasts.items).toHaveLength(0);
		warn.mockRestore();
	});

	it('toasts any other uncaught error', () => {
		raise(new TypeError('x is not a function'));
		expect(toasts.items).toHaveLength(1);
	});
});
