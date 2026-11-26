// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { UsageReadout } from './usage-readout.svelte';

let fire: () => void = () => {};
beforeEach(() => {
	vi.stubGlobal(
		'ResizeObserver',
		class {
			constructor(cb: () => void) {
				fire = cb;
			}
			observe() {}
			unobserve() {}
			disconnect() {}
		}
	);
});
afterEach(() => {
	vi.unstubAllGlobals();
	document.body.innerHTML = '';
});

const box = (el: HTMLElement, right: number, width: number) =>
	(el.getBoundingClientRect = () => ({ right, width }) as DOMRect);

function row(parent: HTMLElement, right: number, width: number) {
	const root = document.createElement('div');
	const readout = document.createElement('span');
	root.append(readout);
	parent.append(root);
	box(root, right, 300);
	box(readout, right, width);
	const r = new UsageReadout();
	return { r, stop: r.track(root, readout) };
}

describe('UsageReadout', () => {
	it('sizes every row stacked in a group to its widest readout', () => {
		const group = document.createElement('section');
		group.dataset.usageGroup = '';
		const list = document.createElement('div');
		group.append(list);
		document.body.append(group);
		const a = row(list, 400, 70.4);
		const b = row(group, 400, 95.2);
		const side = row(group, 800, 60);
		fire();
		expect([a.r.px, b.r.px, side.r.px]).toEqual([96, 96, 60]);
		for (const x of [a, b, side]) x.stop();
	});

	it('falls back to the row parent outside a group', () => {
		const one = document.createElement('div');
		const two = document.createElement('div');
		document.body.append(one, two);
		const a = row(one, 400, 50);
		const b = row(two, 400, 90);
		fire();
		expect([a.r.px, b.r.px]).toEqual([50, 90]);
		a.stop();
		b.stop();
	});
});
