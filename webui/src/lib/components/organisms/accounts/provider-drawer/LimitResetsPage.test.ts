// @vitest-environment happy-dom
import { describe, it, expect, afterEach } from 'vitest';
import { mount, unmount } from 'svelte';
import type { LimitResetEntry } from '$lib/queries';
import LimitResetsPage from './LimitResetsPage.svelte';

let host: HTMLElement | null = null;
let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	host?.remove();
	comp = null;
	host = null;
});

function render(entries: LimitResetEntry[]) {
	host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(LimitResetsPage, { target: host, props: { entries, onclaim: () => {} } });
	return host;
}

const claimButtons = (el: HTMLElement) => [...el.querySelectorAll<HTMLButtonElement>('.row > button')];

const entry = (extra: Partial<LimitResetEntry> = {}): LimitResetEntry => ({
	kind: 'claude',
	id: 'opus_55_explore',
	title: 'Get extra wiggle room to explore Opus 5.5',
	restores: ['five_hour', 'seven_day'],
	expires_at: '2026-10-23T00:00:00Z',
	resets_left: 2,
	requires_limit: false,
	usable: true,
	unusable_reason: null,
	...extra
});

describe('LimitResetsPage', () => {
	it('renders one row per entry with its title, expiry and claims left', () => {
		const el = render([entry(), entry({ id: 'cr_1', kind: 'codex', title: 'Full reset', resets_left: null })]);
		expect(el.querySelectorAll('.row').length).toBe(2);
		expect(el.textContent).toContain('explore Opus 5.5');
		expect(el.textContent).toContain('Full reset');
		expect(el.textContent).toContain('Expires');
		expect(el.textContent).not.toContain('{');
		expect(claimButtons(el).length).toBe(2);
	});

	it('renders the expiry as a machine-readable <time> rather than bare text', () => {
		const el = render([entry()]);
		const stamps = [...el.querySelectorAll('time')].map((t) => t.getAttribute('datetime'));
		expect(stamps).toContain('2026-10-23T00:00:00.000Z');
	});

	it('shows the empty state and no rows when nothing is offered', () => {
		const el = render([]);
		expect(el.querySelectorAll('.row').length).toBe(0);
		expect(el.querySelectorAll('button').length).toBe(0);
		expect(el.textContent).not.toContain('{');
	});

	it('greys a program with nothing available and gives it no button', () => {
		const el = render([entry({ id: 'juniper_tide', title: null, restores: [], expires_at: null, resets_left: null, usable: false, unusable_reason: 'not_at_wall' })]);
		expect(el.querySelectorAll('.row.spent').length).toBe(1);
		expect(el.querySelectorAll('button').length).toBe(0);
		expect(el.textContent).not.toContain('{');
	});

	it('disables the button on an unusable entry and says why', () => {
		const el = render([entry({ usable: false, unusable_reason: 'paused' })]);
		expect(claimButtons(el)[0]?.disabled).toBe(true);
		expect(el.textContent).toContain('paused');
	});

	it('hides the claims-left line for a single remaining claim', () => {
		expect(render([entry({ resets_left: 1 })]).textContent).not.toContain('1 claims left');
	});
});
