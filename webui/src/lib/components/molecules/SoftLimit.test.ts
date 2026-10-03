// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { mount, unmount } from 'svelte';
import SoftLimit from './SoftLimit.svelte';
import { USAGE_LABEL_W, USAGE_READOUT_W } from './cap-bar.logic';

let comps: ReturnType<typeof mount>[] = [];
afterEach(() => {
	for (const c of comps) unmount(c);
	comps = [];
	document.body.innerHTML = '';
});

type Props = Record<string, unknown>;
const render = (props: Props) => {
	const target = document.createElement('div');
	document.body.appendChild(target);
	comps.push(mount(SoftLimit, { target, props: { label: 'Weekly', ...props } as never }));
	const bar = target.querySelector<HTMLElement>('[data-tsu="CapBar"]');
	if (!bar) throw new Error('no CapBar');
	return bar;
};

const col = (bar: HTMLElement, name: 'label-w' | 'readout-w') =>
	new RegExp(`--${name}: ([^;]+);`).exec(bar.getAttribute('style') ?? '')?.[1];

describe('SoftLimit column widths', () => {
	it('gives every window the same label and readout columns', () => {
		const soon = new Date(Date.now() + 3 * 3_600_000).toISOString();
		const bars = [
			render({ utilization: 42, resets: soon }),
			render({ utilization: null }),
			render({ usd: true, amountUsd: 12.5, capUsd: 200 }),
			render({ usd: true, amountUsd: null })
		];
		for (const bar of bars) {
			expect(col(bar, 'label-w')).toBe(USAGE_LABEL_W);
			expect(col(bar, 'readout-w')).toBe(USAGE_READOUT_W);
		}
	});

	it('takes density from the list rather than measuring itself', () => {
		const dense = render({ utilization: 42, dense: true });
		expect(dense.querySelector('.label')).toBeNull();
		expect(col(dense, 'label-w')).toBe('0px');
		expect(col(dense, 'readout-w')).toBe(USAGE_READOUT_W);

		const wide = render({ utilization: 42, dense: false });
		expect(wide.querySelector('.label')?.textContent).toBe('Weekly');
	});
});
