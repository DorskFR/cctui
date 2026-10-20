// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { AccountProvider, OAuthAccount } from '$lib/queries';

const actions = vi.hoisted(() => ({
	updateProvider: vi.fn(async (_a: string, id: string) => ({ id }) as unknown),
	moveProvider: vi.fn(async () => ({}))
}));
vi.mock('$lib/queries', () => ({
	useAccountActions: () => actions,
	useAccountUsage: () => ({ data: undefined }),
	useLimitReset: () => vi.fn(),
	useSettingsCatalog: () => ({ data: undefined, error: null }),
	useUsageProbes: () => ({})
}));
vi.mock('$lib/paraglide/messages', async (importOriginal) => {
	const real = (await importOriginal<{ m: Record<string, unknown> }>()).m;
	return { m: new Proxy(real, { get: (t, k: string) => t[k] ?? (() => k) }) };
});

import ProviderSettingsSection from './ProviderSettingsSection.svelte';

const prov = (id: string, provider: string): AccountProvider =>
	({
		id,
		account_id: 'a1',
		provider,
		family: provider,
		managed: false,
		soft_limits: null,
		model_aliases: {},
		models: []
	}) as unknown as AccountProvider;
const claude = prov('p1', 'anthropic');
const codex = prov('p2', 'openai');
const account = { id: 'a1', name: 'work', user_id: 'u1', providers: [claude, codex] } as unknown as OAuthAccount;

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.clearAllMocks();
});

function render(props: Record<string, unknown>) {
	const onselect = vi.fn();
	const onmoved = vi.fn();
	comp = mount(ProviderSettingsSection, {
		target: document.body,
		props: { account, providers: [claude, codex], selected: claude, onselect, onmoved, ...props }
	});
	flushSync();
	return { onselect, onmoved };
}

const tabs = () => [...document.querySelectorAll<HTMLElement>('[role="tab"]')];
const button = (label: string) =>
	[...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === label)!;
const activeNav = () =>
	[...document.querySelectorAll('[aria-current="page"], .active')].map((e) => e.textContent?.trim());

describe('ProviderSettingsSection', () => {
	it('renders the settings inline on the account page, not in a dialog', () => {
		render({});
		expect(document.getElementById('provider-settings')).not.toBeNull();
		expect(document.querySelector('dialog')).toBeNull();
	});

	it('offers one tab per editable provider and selects through onselect', () => {
		const { onselect } = render({});
		expect(tabs()).toHaveLength(2);
		tabs()[1].click();
		flushSync();
		expect(onselect).toHaveBeenCalledWith(codex);
	});

	it('shows no tabs when the account has a single editable provider', () => {
		render({ providers: [claude] });
		expect(tabs()).toHaveLength(0);
	});

	it('opens on Limits by default', () => {
		render({});
		expect(activeNav().some((t) => t?.includes('Limits'))).toBe(true);
		expect(activeNav().some((t) => t?.includes('Aliases'))).toBe(false);
	});

	it('lands on the deep-linked section', () => {
		render({ initialPage: 'aliases' });
		expect(activeNav().some((t) => t?.includes('Aliases'))).toBe(true);
		expect(activeNav().some((t) => t?.includes('Limits'))).toBe(false);
	});

	it('saving keeps the page open instead of navigating away', async () => {
		const { onmoved } = render({});
		button('Save').click();
		await vi.waitFor(() => expect(actions.updateProvider).toHaveBeenCalledOnce());
		expect(actions.updateProvider.mock.calls[0].slice(0, 2)).toEqual(['a1', 'p1']);
		expect(onmoved).not.toHaveBeenCalled();
		expect(document.getElementById('provider-settings')).not.toBeNull();
	});
});
