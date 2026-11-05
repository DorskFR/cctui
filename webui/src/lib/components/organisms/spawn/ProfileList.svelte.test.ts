// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionProfile } from '@bindings/SessionProfile';
import type { OAuthAccount } from '$lib/queries';
import Host from './ProfileList.host.test.svelte';
import type { ProfileSpecForm } from './profiles';

vi.mock('$lib/queries', () => {
	const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
	return {
		useSessions: () => q({ sessions: [] }),
		useContextItems: () => q([]),
		useResolvedContext: () => q({ rules: [], memories: [] }),
		useCodexModels: () => q(null),
		useMergedCodexModels: () => q(null),
		useHarnessModels: () => q(null),
		useHarnesses: () => q(null),
		useMachineAdapters: () => q(null)
	};
});

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
const onsave = vi.fn<(id: string, name: string, spec: ProfileSpecForm) => void>();
const ondelete = vi.fn<(id: string) => void>();
let state: { selectedId: string | null; oneOff: ProfileSpecForm | null };
let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
	oncreate.mockReset().mockResolvedValue(true);
	onsave.mockReset();
	ondelete.mockReset();
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

const tick = (ms = 40) => new Promise((r) => setTimeout(r, ms));

async function open(manageHref?: string) {
	component = mount(Host, {
		target: document.body,
		props: {
			profiles: [codexQuick, orchestrator],
			accounts,
			selected: 'p1',
			manageHref,
			oncreate,
			onsave,
			ondelete,
			onready: (s) => (state = s)
		}
	});
	await tick();
	flushSync();
}

const button = (text: string) => {
	const b = [...document.querySelectorAll<HTMLButtonElement>('button')].find(
		(x) => (x.textContent?.trim() || x.getAttribute('aria-label')) === text
	);
	if (!b) throw new Error(`button ${text} not found`);
	return b;
};
const hasButton = (text: string) =>
	[...document.querySelectorAll<HTMLButtonElement>('button')].some(
		(x) => (x.textContent?.trim() || x.getAttribute('aria-label')) === text
	);
const profileForm = () => document.querySelector<HTMLElement>('[data-journey="profile-form"]');
const mode = (v: string) => {
	const el = document.querySelector<HTMLButtonElement>(`button[data-mode="${v}"]`);
	if (!el) throw new Error(`permission mode ${v} not found`);
	return el;
};
const panel = () => {
	const t = document.querySelector('[data-journey="profile-trigger"]');
	const id = t?.getAttribute('popovertarget');
	const el = id ? document.getElementById(id) : null;
	if (!el) throw new Error('profile picker panel not found');
	return el;
};
async function typeName(v: string) {
	const el = profileForm()?.querySelector<HTMLInputElement>('input[id^="sp-profile-name"]');
	if (!el) throw new Error('profile name input not found');
	el.value = v;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	await tick();
	flushSync();
}
async function click(b: HTMLElement) {
	b.click();
	await tick();
	flushSync();
}

describe('ProfileList as the profile editor (no manageHref)', () => {
	it('the cog toggles an edit form for the selected profile', async () => {
		await open();
		await click(button('Adjust profile'));
		expect(profileForm()?.dataset.mode).toBe('edit');
		expect(profileForm()?.dataset.profileId).toBe('p1');
		await click(button('Adjust profile'));
		expect(profileForm()).toBeNull();
	});

	it('"Save to profile" persists the adjusted kit', async () => {
		await open();
		await click(button('Adjust profile'));
		await click(mode('auto'));
		await click(button('Save to profile'));
		expect(onsave).toHaveBeenCalledTimes(1);
		expect(onsave.mock.calls[0][0]).toBe('p1');
		expect(onsave.mock.calls[0][1]).toBe('Orchestrator');
		expect(onsave.mock.calls[0][2]).toMatchObject({ permission_mode: 'auto', account_id: 'a1' });
		expect(profileForm()).toBeNull();
	});

	it('"Use once" keeps the profile and binds the adjusted kit as a one-off', async () => {
		await open();
		await click(button('Adjust profile'));
		await click(mode('ask'));
		await click(button('Use once'));
		expect(onsave).not.toHaveBeenCalled();
		expect(state.selectedId).toBe('p1');
		expect(state.oneOff).toMatchObject({ permission_mode: 'ask', model_alias: 'fable' });
	});

	it('Delete hands the edited profile to ondelete', async () => {
		await open();
		await click(button('Adjust profile'));
		await click(button('Delete profile'));
		expect(ondelete).toHaveBeenCalledWith('p1');
		expect(profileForm()).toBeNull();
	});

	it('New profile writes nothing until Create profile, and Cancel discards it', async () => {
		await open();
		await click(panel().querySelector<HTMLButtonElement>('[data-journey="new-profile"]') as HTMLElement);
		expect(profileForm()?.dataset.mode).toBe('create');
		expect(button('Create profile').disabled).toBe(true);
		await typeName('Reviewer');
		expect(button('Create profile').disabled).toBe(false);
		await click(button('Cancel'));
		expect(oncreate).not.toHaveBeenCalled();
		expect(profileForm()).toBeNull();

		await click(panel().querySelector<HTMLButtonElement>('[data-journey="new-profile"]') as HTMLElement);
		await typeName('Reviewer');
		await click(button('Create profile'));
		expect(oncreate).toHaveBeenCalledTimes(1);
		expect(oncreate.mock.calls[0][0]).toBe('Reviewer');
		expect(oncreate.mock.calls[0][1]).toMatchObject({ harness: 'claude-code', account_id: 'a1' });
	});
});

describe('ProfileList as a picker (manageHref)', () => {
	it('offers no cog and no New profile; the footer links to manageHref', async () => {
		await open('/settings/sessions#profiles');
		expect(hasButton('Adjust profile')).toBe(false);
		expect(panel().querySelector('[data-journey="new-profile"]')).toBeNull();
		const link = panel().querySelector<HTMLAnchorElement>('a[data-journey="manage-profiles"]');
		expect(link?.getAttribute('href')).toBe('/settings/sessions#profiles');
		expect(link?.textContent?.trim()).toBe('Manage profiles…');
		expect(profileForm()).toBeNull();
	});
});
