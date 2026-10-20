// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
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
	[...document.querySelectorAll<HTMLElement>('ul:not(.resolved) li.row')].map((li) =>
		(li.querySelector('input[type=checkbox]') ? li.querySelector('label') : li)?.textContent?.trim()
	);
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
		const group = document.querySelector('ul.blocking-group');
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
		expect(document.querySelectorAll('[data-tsu="Badge"]').length).toBe(0);
		expect(text()).not.toMatch(/\b(do|decide|tell)\b/);
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

	it('opens the detail from a disclosure on the row and renders it as markdown', async () => {
		await render([
			action({ id: 'a', title: 'With detail', detail: 'Check **step 4**' }),
			action({ id: 'b', title: 'Without detail' })
		]);
		const toggles = document.querySelectorAll<HTMLButtonElement>('button[aria-expanded]');
		expect(toggles.length).toBe(1);
		expect(toggles[0].getAttribute('aria-expanded')).toBe('false');
		expect(document.querySelector('.detail')).toBeNull();
		expect(text()).not.toContain('step 4');
		toggles[0].click();
		await flush();
		expect(toggles[0].getAttribute('aria-expanded')).toBe('true');
		const detail = document.getElementById(toggles[0].getAttribute('aria-controls') ?? '');
		expect(detail?.querySelector('strong')?.textContent).toBe('step 4');
		toggles[0].click();
		await flush();
		expect(document.querySelector('.detail')).toBeNull();
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
		expect(document.querySelector('ul.resolved')?.closest('[hidden]')).not.toBeNull();
		toggle.click();
		await flush();
		expect(toggle.getAttribute('aria-expanded')).toBe('true');
		expect(document.querySelector('ul.resolved')?.closest('[hidden]')).toBeNull();
		const rows = [...document.querySelectorAll<HTMLElement>('ul.resolved li')];
		expect(rows.map((r) => r.classList.contains('dropped'))).toEqual([false, false, true]);
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

	it('renders nothing when no item is open', async () => {
		await render([action({ id: 'd', status: 'done', resolved_at: '2026-09-30T11:00:00Z' })]);
		expect(document.body.textContent?.trim()).toBe('');
	});
});
