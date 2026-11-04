// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import { readFileSync } from 'node:fs';
import type { UserAction } from '@bindings/UserAction';
import UserActionsCard from './UserActionsCard.svelte';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const action = (over: Partial<UserAction>): UserAction => ({
	id: over.id ?? 'id-1',
	title: over.title ?? 'Approve PR #12',
	detail: over.detail ?? null,
	kind: over.kind ?? 'action',
	blocking: over.blocking ?? false,
	status: over.status ?? 'open',
	note: over.note ?? null,
	created_at: over.created_at ?? '2026-09-30T10:00:00Z',
	resolved_at: over.resolved_at ?? null,
	resolved_by: over.resolved_by ?? null
});

async function render(actions: UserAction[], interactive = true) {
	const ontick = vi.fn();
	comp = mount(UserActionsCard, { target: document.body, props: { actions, interactive, ontick } });
	await flush();
	return ontick;
}

const rowTitles = () =>
	[...document.querySelectorAll<HTMLElement>('[data-ua-row] [data-ua-title]')].map((t) => t.textContent?.trim());
const text = () => document.body.textContent ?? '';
const buttonByName = (re: RegExp) =>
	[...document.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
		re.test(`${b.textContent ?? ''} ${b.getAttribute('aria-label') ?? ''}`)
	);
const mustFind = (re: RegExp): HTMLButtonElement => {
	const b = buttonByName(re);
	if (!b) throw new Error(`no button matching ${re}`);
	return b;
};

describe('UserActionsCard', () => {
	it('groups blocking items first in their own tinted list, oldest first after', async () => {
		await render([
			action({ id: 'a', title: 'Plain A', created_at: '2026-09-30T10:00:00Z' }),
			action({ id: 'b', title: 'Plain B', created_at: '2026-09-30T11:00:00Z' }),
			action({ id: 'c', title: 'Urgent C', blocking: true, created_at: '2026-09-30T12:00:00Z' })
		]);
		expect(rowTitles()).toEqual(['Urgent C', 'Plain A', 'Plain B']);
		const group = document.querySelector('ul.ua-blocking-group');
		expect(group?.textContent).toContain('Urgent C');
		expect(group?.textContent).not.toContain('Plain A');
		expect(text()).toContain('Blocking');
	});

	it('does not render the kind labels', async () => {
		await render([
			action({ id: 'a', kind: 'action', title: 'Rotate token' }),
			action({ id: 'b', kind: 'decision', title: 'Pick a name' }),
			action({ id: 'c', kind: 'input', title: 'Tell me the DSN' })
		]);
		expect(rowTitles()).toEqual(['Rotate token', 'Pick a name', 'Tell me the DSN']);
		expect(text()).not.toMatch(/\b(do|decide|action|decision|input)\b/);
	});

	it('marks each blocking row with a labelled warning icon and a data attribute', async () => {
		await render([
			action({ id: 'a', title: 'Plain' }),
			action({ id: 'b', title: 'Urgent', blocking: true })
		]);
		const blocking = [...document.querySelectorAll<HTMLElement>('[data-ua-row][data-blocking]')];
		expect(blocking.map((r) => r.querySelector('[data-ua-title]')?.textContent?.trim())).toEqual(['Urgent']);
		expect(blocking[0].querySelector('[data-ua-title] [aria-label="Blocking"]')).not.toBeNull();
		const plain = document.querySelector('[data-ua-row]:not([data-blocking])');
		expect(plain?.querySelector('[aria-label="Blocking"]')).toBeNull();
	});

	it('keeps the title out of the checkbox label so clicking it never ticks', async () => {
		const ontick = await render([action({ id: 'a', title: 'Click me', detail: 'more' })]);
		const label = document.querySelector('[data-ua-row] label');
		expect(label?.contains(document.querySelector('[data-ua-title]'))).toBeFalsy();
		document.querySelector<HTMLElement>('[data-ua-title]')?.click();
		await flush();
		expect(ontick).not.toHaveBeenCalled();
	});

	it('ticks an item done from its checkbox', async () => {
		const ontick = await render([action({ id: 'a' })]);
		document.querySelector<HTMLInputElement>('input[type=checkbox]')?.click();
		expect(ontick).toHaveBeenCalledWith('a', 'done');
	});

	it('shows no checkbox once the session is archived', async () => {
		await render([action({ id: 'a', title: 'Read only' })], false);
		expect(document.querySelector('input[type=checkbox]')).toBeNull();
		expect(text()).toContain('Read only');
	});

	it('collapses the detail by default behind the title and renders it as markdown', async () => {
		await render([
			action({ id: 'a', title: 'With detail', detail: 'Check **step 4**' }),
			action({ id: 'b', title: 'Without detail' })
		]);
		const toggles = document.querySelectorAll<HTMLButtonElement>('[data-ua-row] button[aria-expanded]');
		expect(toggles.length).toBe(1);
		expect(toggles[0].textContent?.trim()).toBe('With detail');
		expect(toggles[0].getAttribute('aria-expanded')).toBe('false');
		const panel = () => document.getElementById(toggles[0].getAttribute('aria-controls') ?? '');
		expect(panel()?.hidden).toBe(true);
		toggles[0].click();
		await flush();
		expect(toggles[0].getAttribute('aria-expanded')).toBe('true');
		expect(panel()?.hidden).toBe(false);
		expect(panel()?.querySelector('.ua-detail strong')?.textContent).toBe('step 4');
		toggles[0].click();
		await flush();
		expect(panel()?.hidden).toBe(true);
	});

	it('folds done and dropped items into one summary row, expandable, each distinguishable', async () => {
		await render([
			action({ id: 'open', title: 'Still open' }),
			action({
				id: 'd1',
				title: 'Bumped',
				status: 'done',
				note: 'bumped to 0.25.2',
				resolved_at: '2026-09-30T11:00:00Z'
			}),
			action({ id: 'd2', title: 'Chose', status: 'done', resolved_at: '2026-09-30T11:30:00Z' }),
			action({
				id: 'x1',
				title: 'Superseded',
				status: 'dropped',
				note: 'fixture rewrite',
				resolved_at: '2026-09-30T12:00:00Z'
			})
		]);
		const toggle = mustFind(/2 done · 1 dropped/);
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
		expect(document.querySelector('ul.ua-resolved')?.closest('[hidden]')).not.toBeNull();
		toggle.click();
		await flush();
		expect(toggle.getAttribute('aria-expanded')).toBe('true');
		expect(document.querySelector('ul.ua-resolved')?.closest('[hidden]')).toBeNull();
		const rows = [...document.querySelectorAll<HTMLElement>('ul.ua-resolved li')];
		expect(rows.map((r) => r.classList.contains('ua-dropped'))).toEqual([false, false, true]);
		expect(rows[0].textContent).toContain('bumped to 0.25.2');
		expect(rows[2].textContent).toContain('fixture rewrite');
		expect(rows[0].querySelector('[aria-label="done"]')).not.toBeNull();
		expect(rows[2].querySelector('[aria-label="dropped"]')).not.toBeNull();
	});

	it('omits the resolved row when nothing is resolved', async () => {
		await render([action({ id: 'a' })]);
		expect(buttonByName(/done|dropped/)).toBeUndefined();
	});

	it('caps a long list behind "show N more" without hiding blocking items', async () => {
		const many = Array.from({ length: 30 }, (_, i) =>
			action({
				id: `o${i}`,
				title: `Item ${i}`,
				blocking: i === 29,
				created_at: `2026-09-30T${String(10 + i).padStart(2, '0')}:00:00.000Z`
			})
		);
		await render(many);
		expect(rowTitles().length).toBe(8);
		expect(rowTitles()[0]).toBe('Item 29');
		mustFind(/Show 22 more/).click();
		await flush();
		expect(rowTitles().length).toBe(30);
		expect(buttonByName(/Show less/)).toBeTruthy();
	});

	it('starts every title on the same edge whether or not it has detail', async () => {
		await render([
			action({ id: 'a', title: 'With detail', detail: 'x' }),
			action({ id: 'b', title: 'Without detail' })
		]);
		const rows = [...document.querySelectorAll<HTMLElement>('[data-ua-row]')];
		expect(rows.map((r) => r.querySelector(':scope > .ua-check + .ua-body [data-ua-title]') !== null)).toEqual([
			true,
			true
		]);
	});

	it('uses no class name that a tsumikit global utility also styles', async () => {
		const utilities = readFileSync('node_modules/@dorsk/tsumikit/dist/styles/utilities.css', 'utf8');
		const reserved = new Set([...utilities.matchAll(/\.([a-zA-Z][\w-]*)/g)].map((m) => m[1]));
		expect(reserved.has('row')).toBe(true);
		await render([
			action({ id: 'a', title: 'A', detail: 'd', blocking: true }),
			action({ id: 'b', title: 'B' }),
			action({ id: 'd', title: 'D', status: 'done', resolved_at: '2026-09-30T11:00:00Z' })
		]);
		mustFind(/1 done/).click();
		await flush();
		const own = [...document.querySelectorAll<HTMLElement>('[class*="svelte-"]')]
			.filter((el) => !el.hasAttribute('data-tsu') && !el.closest('[data-tsu="Checkbox"]'))
			.flatMap((el) => [...el.classList]);
		expect(own.length).toBeGreaterThan(5);
		const clashes = own.filter((c) => reserved.has(c));
		expect(clashes).toEqual([]);
	});

	it('renders nothing when no item is open', async () => {
		await render([action({ id: 'd', status: 'done', resolved_at: '2026-09-30T11:00:00Z' })]);
		expect(document.body.textContent?.trim()).toBe('');
	});
});
