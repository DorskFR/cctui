// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { filePickGuard, PICK_KEY } from './filePickGuard';

function memStore() {
	const data = new Map<string, string>();
	return {
		getItem: (k: string) => data.get(k) ?? null,
		setItem: (k: string, v: string) => void data.set(k, v),
		removeItem: (k: string) => void data.delete(k),
		data
	};
}

function setup(store = memStore(), now = () => 1_000_000) {
	const host = document.body.appendChild(document.createElement('div'));
	const input = host.appendChild(document.createElement('input'));
	input.type = 'file';
	const onlost = vi.fn();
	const off = filePickGuard(onlost, { store, now })(host);
	return { host, input, onlost, store, off: () => (off(), host.remove()) };
}

afterEach(() => {
	document.body.innerHTML = '';
	vi.useRealTimers();
});

describe('filePickGuard', () => {
	it('reports a pick the page lost to a reload while the camera was open', () => {
		const first = setup();
		first.input.dispatchEvent(new MouseEvent('click', { bubbles: true }));
		expect(first.store.data.has(PICK_KEY)).toBe(true);
		first.off();

		const reloaded = setup(first.store, () => 1_000_000 + 30_000);
		expect(reloaded.onlost).toHaveBeenCalledOnce();
		expect(reloaded.store.data.has(PICK_KEY)).toBe(false);
	});

	it('stays quiet when the picked file arrived', () => {
		const first = setup();
		first.input.dispatchEvent(new MouseEvent('click', { bubbles: true }));
		first.input.dispatchEvent(new Event('change', { bubbles: true }));
		first.off();
		expect(setup(first.store).onlost).not.toHaveBeenCalled();
	});

	it('stays quiet when the picker was cancelled', () => {
		const first = setup();
		first.input.dispatchEvent(new MouseEvent('click', { bubbles: true }));
		first.input.dispatchEvent(new Event('cancel', { bubbles: true }));
		first.off();
		expect(setup(first.store).onlost).not.toHaveBeenCalled();
	});

	it('clears the marker once the page comes back on its own', () => {
		vi.useFakeTimers();
		const g = setup();
		g.input.dispatchEvent(new MouseEvent('click', { bubbles: true }));
		document.dispatchEvent(new Event('visibilitychange'));
		vi.advanceTimersByTime(3_000);
		expect(g.store.data.has(PICK_KEY)).toBe(false);
		g.off();
	});

	it('ignores a stale marker', () => {
		const store = memStore();
		store.setItem(PICK_KEY, String(1_000_000));
		expect(setup(store, () => 1_000_000 + 11 * 60_000).onlost).not.toHaveBeenCalled();
		expect(store.data.has(PICK_KEY)).toBe(false);
	});
});
