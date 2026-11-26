// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionProfile } from '@bindings/SessionProfile';
import type { OAuthAccount } from '#lib/queries/index.ts';
import Host from './ProfileList.host.test.svelte';
import type { ProfileSpecForm } from './profiles';

vi.mock('#lib/queries/index.ts', () => {
	const q = (data: unknown) => ({ data, isLoading: false, isError: false });
	return {
		useContextItems: () => q([]),
		useHarnessModels: () => q(null),
		useHarnesses: () => q(null),
		useMachineAdapters: () => q(null)
	};
});
vi.mock('#lib/features.svelte.ts', () => ({ hasFeature: () => false }));

const accounts = [
	{
		id: 'a1',
		name: 'personal',
		emoji: '🐼',
		providers: [{ id: 'pr1', provider: 'anthropic', models: [], model_aliases: null }]
	}
] as unknown as OAuthAccount[];
const orchestrator: SessionProfile = {
	id: 'p1',
	user_id: 'u1',
	name: 'Orchestrator',
	harness: 'claude-code',
	account_id: 'a1',
	pool_id: null,
	no_account: false,
	model_alias: 'fable',
	effort: 'medium',
	permission_mode: 'yolo',
	service_tier: null,
	context_items: [],
	auto_followup: null,
	sort_order: 0,
	created_at: '',
	updated_at: ''
};
const codexQuick: SessionProfile = {
	...orchestrator,
	id: 'p2',
	name: 'Quick codex',
	harness: 'codex',
	account_id: null,
	model_alias: null,
	effort: 'high',
	permission_mode: 'auto'
};

const oncreate = vi.fn<(name: string, spec: ProfileSpecForm) => Promise<boolean>>();
const onsave = vi.fn();
const ondelete = vi.fn();
const onmanage = vi.fn();
let held: { selectedId: string | null; oneOff: ProfileSpecForm | null };
let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
	oncreate.mockReset().mockResolvedValue(true);
	onsave.mockReset();
	ondelete.mockReset();
	onmanage.mockReset();
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

function render(manageHref?: string, target: Element = document.body) {
	component = mount(Host, {
		target,
		props: {
			profiles: [orchestrator, codexQuick],
			accounts,
			oncreate,
			onsave,
			ondelete,
			manageHref,
			onmanage,
			onready: (s) => (held = s)
		}
	});
	flushSync();
}

const buttons = () => [...document.querySelectorAll<HTMLButtonElement>('button')];
const button = (label: string) => {
	const b = buttons().find(
		(x) => (x.textContent?.trim() || x.getAttribute('aria-label') || '') === label
	);
	if (!b) throw new Error(`button ${label} not found`);
	return b;
};
const profileForm = () => document.querySelector<HTMLElement>('[data-journey="profile-form"]');
const trigger = () => {
	const el = document.querySelector<HTMLButtonElement>('[data-journey="profile-trigger"]');
	if (!el) throw new Error('profile picker trigger not found');
	return el;
};
// happy-dom implements no Popover API: wire show/hide to the toggle a browser fires.
function openPicker() {
	const id = trigger().getAttribute('popovertarget');
	const el = id ? document.getElementById(id) : null;
	if (!el) throw new Error('profile picker panel not found');
	const fire = (newState: 'open' | 'closed') =>
		el.dispatchEvent(Object.assign(new Event('toggle'), { newState }));
	Object.assign(el, { showPopover: () => fire('open'), hidePopover: () => fire('closed') });
	trigger().click();
	(el as HTMLElement & { showPopover: () => void }).showPopover();
	flushSync();
	return el;
}
const mode = (v: string) => {
	const el = document.querySelector<HTMLButtonElement>(`button[data-mode="${v}"]`);
	if (!el) throw new Error(`permission mode ${v} not found`);
	return el;
};

describe('ProfileList as a picker (the spawn form)', () => {
	it('has no editing control and links its footer to where profiles are managed', () => {
		render('/settings/sessions#launch');
		expect(() => button('Adjust profile')).toThrow();
		const panel = openPicker();
		expect(panel.querySelector('[data-journey="new-profile"]')).toBeNull();
		const link = panel.querySelector<HTMLAnchorElement>('a[data-journey="manage-profiles"]');
		expect(link?.getAttribute('href')).toBe('/settings/sessions#launch');
		link?.addEventListener('click', (e) => e.preventDefault());
		link?.click();
		flushSync();
		expect(onmanage).toHaveBeenCalledTimes(1);
		expect(profileForm()).toBeNull();
	});
});

describe('ProfileList picker placement', () => {
	const rect = (top: number, bottom: number) =>
		({ top, bottom, left: 0, right: 400, x: 0, y: top, width: 400, height: bottom - top }) as DOMRect;
	function inArea(cardTop: number, labelAbove?: [number, number]) {
		const area = document.createElement('div');
		area.style.overflowY = 'auto';
		area.getBoundingClientRect = () => rect(100, 700);
		document.body.append(area);
		if (labelAbove) {
			const label = document.createElement('label');
			label.getBoundingClientRect = () => rect(...labelAbove);
			area.append(label);
		}
		render('/settings/sessions#launch', area);
		const card = document.querySelector<HTMLElement>('.card');
		if (!card) throw new Error('picker card not found');
		card.getBoundingClientRect = () => rect(cardTop, cardTop + 60);
		trigger().dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
		flushSync();
		return card;
	}

	it('opens upward, capped to the room above, when the card sits low in the scroll area', () => {
		const card = inArea(600);
		expect(card.dataset.placement).toBe('top-start');
		expect(card.style.getPropertyValue('--sheet-max')).toBe('492px');
	});

	it('stops an upward sheet below the label of the field above', () => {
		const card = inArea(600, [300, 320]);
		expect(card.dataset.placement).toBe('top-start');
		expect(card.style.getPropertyValue('--sheet-max')).toBe('272px');
	});

	it('opens downward, capped to the room below, when there is space under the card', () => {
		const card = inArea(150);
		expect(card.dataset.placement).toBe('bottom-start');
		expect(card.style.getPropertyValue('--sheet-max')).toBe('482px');
	});
});

describe('ProfileList as the editor (Settings)', () => {
	it('the cog toggles an edit form for the current profile', () => {
		render();
		button('Adjust profile').click();
		flushSync();
		expect(profileForm()?.dataset.mode).toBe('edit');
		expect(profileForm()?.dataset.profileId).toBe('p1');
		button('Adjust profile').click();
		flushSync();
		expect(profileForm()).toBeNull();
	});

	it('"Save to profile" hands the adjusted kit back', () => {
		render();
		button('Adjust profile').click();
		flushSync();
		mode('auto').click();
		flushSync();
		button('Save to profile').click();
		flushSync();
		expect(onsave).toHaveBeenCalledTimes(1);
		expect(onsave.mock.calls[0][0]).toBe('p1');
		expect(onsave.mock.calls[0][1]).toBe('Orchestrator');
		expect(onsave.mock.calls[0][2]).toMatchObject({ harness: 'claude-code', permission_mode: 'auto' });
		expect(profileForm()).toBeNull();
	});

	it('"Use once" adjusts the selection only', () => {
		render();
		button('Adjust profile').click();
		flushSync();
		mode('ask').click();
		flushSync();
		button('Use once').click();
		flushSync();
		expect(onsave).not.toHaveBeenCalled();
		expect(held.oneOff).toMatchObject({ permission_mode: 'ask' });
	});

	it('Delete hands back the edited profile', () => {
		render();
		button('Adjust profile').click();
		flushSync();
		button('Delete profile').click();
		flushSync();
		expect(ondelete).toHaveBeenCalledWith('p1');
		expect(profileForm()).toBeNull();
	});

	it('New profile opens a create form that writes nothing until Create profile', async () => {
		render();
		const panel = openPicker();
		panel.querySelector<HTMLButtonElement>('[data-journey="new-profile"]')?.click();
		flushSync();
		expect(profileForm()?.dataset.mode).toBe('create');
		expect(document.querySelector('[data-journey="profile-draft"]')).not.toBeNull();
		const name = document.querySelector<HTMLInputElement>('input[id^="sp-profile-name"]');
		if (!name) throw new Error('profile name input not found');
		name.value = 'Reviewer';
		name.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		expect(oncreate).not.toHaveBeenCalled();
		button('Create profile').click();
		await Promise.resolve();
		flushSync();
		expect(oncreate).toHaveBeenCalledTimes(1);
		expect(oncreate.mock.calls[0][0]).toBe('Reviewer');
		expect(profileForm()).toBeNull();
	});
});
