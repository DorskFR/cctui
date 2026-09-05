import { describe, it, expect, afterEach } from 'vitest';
import { mount, unmount } from 'svelte';
import type { TokenUsageWindows } from '@bindings/TokenUsageWindows';
import WindowsTable from './WindowsTable.svelte';

let host: HTMLElement | null = null;
let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	host?.remove();
	comp = null;
	host = null;
});

function render(windows: TokenUsageWindows | undefined) {
	host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(WindowsTable, { target: host, props: { windows } });
	return host;
}

const w = (input: number, output: number, cache_read: number) => ({ input, output, cache_read });

describe('WindowsTable', () => {
	it('renders a header plus one row per window, bars scaled to 30d', () => {
		const el = render({
			hour: w(10, 5, 5),
			today: w(40, 20, 40),
			day: w(50, 25, 25),
			week: w(200, 100, 100),
			month: w(500, 250, 250)
		});
		expect(el.querySelectorAll('.row').length).toBe(6);

		const fills = [...el.querySelectorAll<HTMLElement>('.fill')];
		expect(fills.length).toBe(5);
		expect(fills[0].style.width).toBe('2%');
		expect(fills[4].style.width).toBe('100%');
		expect(el.textContent).toContain('Last hour');
	});

	it('renders empty bars while the query is loading', () => {
		const el = render(undefined);
		const fills = [...el.querySelectorAll<HTMLElement>('.fill')];
		expect(fills.every((f) => f.style.width === '0%')).toBe(true);
	});
});
