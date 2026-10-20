// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ServiceTokenRow } from '@bindings/ServiceTokenRow';
import type { OAuthAccount } from '$lib/queries';

const state = vi.hoisted(() => ({
	rows: [] as ServiceTokenRow[],
	mint: vi.fn(async (_a: string, body: { label: string; provider_id: string }) => ({
		token: 'cctui_s_secret',
		row: {
			id: 't-new',
			label: body.label,
			provider_id: body.provider_id,
			provider: 'anthropic',
			created_at: '2026-10-20T00:00:00Z',
			last_used_at: null
		}
	})),
	revoke: vi.fn(async () => {})
}));
vi.mock('$lib/queries', () => ({
	useServiceTokens: () => ({
		get data() {
			return state.rows;
		},
		isLoading: false
	}),
	useServiceTokenActions: () => ({ mint: state.mint, revoke: state.revoke })
}));
vi.mock('$lib/paraglide/messages', async (importOriginal) => {
	const real = (await importOriginal<{ m: Record<string, unknown> }>()).m;
	return { m: new Proxy(real, { get: (t, k: string) => t[k] ?? (() => k) }) };
});

import ServiceTokensSection from './ServiceTokensSection.svelte';
import { m } from '$lib/paraglide/messages';

const account = {
	id: 'a1',
	name: 'work',
	user_id: 'u1',
	providers: [{ id: 'p1', provider: 'anthropic', family: 'anthropic', managed: false }]
} as unknown as OAuthAccount;

const token: ServiceTokenRow = {
	id: 't1',
	label: 'translator',
	provider_id: 'p1',
	provider: 'anthropic',
	created_at: '2026-10-01T00:00:00Z',
	last_used_at: null
};

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	state.rows = [];
	vi.clearAllMocks();
});

function render() {
	comp = mount(ServiceTokensSection, { target: document.body, props: { account } });
	flushSync();
}
const flush = () => new Promise((r) => setTimeout(r, 0));
const button = (text: string) =>
	[...document.querySelectorAll<HTMLButtonElement>('button')].find(
		(b) => b.textContent?.trim() === text
	);
const click = (el: HTMLElement | null | undefined) => {
	expect(el).toBeTruthy();
	el?.click();
};

describe('ServiceTokensSection', () => {
	it('lists the live tokens with their label and provider', () => {
		state.rows = [token];
		render();
		expect(document.body.textContent).toContain('translator');
		expect(document.querySelectorAll('[data-tsu="DataTable"] tbody tr, tbody tr').length).toBe(1);
	});

	it('mints with the typed label on the account provider and reveals the secret once', async () => {
		render();
		click(button(m.service_tokens_new()));
		flushSync();
		const input = document.querySelector<HTMLInputElement>('dialog input');
		expect(input).not.toBeNull();
		if (!input) return;
		input.value = 'kusaritoi';
		input.dispatchEvent(new Event('input', { bubbles: true }));
		flushSync();
		click(button(m.service_tokens_create()));
		await flush();
		flushSync();

		expect(state.mint).toHaveBeenCalledWith('a1', { label: 'kusaritoi', provider_id: 'p1' });
		expect(document.body.textContent).toContain('cctui_s_secret');
		expect(button(m.common_copy())).toBeDefined();
	});

	it('revokes only after the confirm modal', async () => {
		state.rows = [token];
		render();
		click(document.querySelector<HTMLButtonElement>(`[title="${m.service_tokens_revoke()}"]`));
		flushSync();
		expect(state.revoke).not.toHaveBeenCalled();
		expect(document.querySelector('dialog')).not.toBeNull();

		click(button(m.service_tokens_revoke()));
		await flush();
		expect(state.revoke).toHaveBeenCalledWith('a1', 't1');
	});
});
