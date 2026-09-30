// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { OAuthAccount } from '$lib/queries';
import KitFields from './KitFields.svelte';
import { EMPTY_SPEC, type ProfileSpecForm } from './profiles';

vi.mock('$lib/queries', () => ({
	useHarnessModels: () => ({ data: null, isLoading: false, isError: false })
}));

const acct = (id: string, provider: string): OAuthAccount =>
	({
		id,
		name: id,
		emoji: null,
		providers: [{ id: `pr-${id}`, provider, models: [], model_aliases: null }]
	}) as unknown as OAuthAccount;
// "Spark local (OpenCode)": a fireworks key, which only OpenCode runs.
const accounts = [acct('claude', 'anthropic'), acct('spark', 'fireworks'), acct('gpt', 'openai')];
const pools = [
	{ id: 'claude-pool', name: 'claude', members: [{ account_id: 'claude' }] },
	{ id: 'spark-pool', name: 'spark', members: [{ account_id: 'spark' }] }
] as never[];

let component: ReturnType<typeof mount> | undefined;
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
});

function render(spec: Partial<ProfileSpecForm>) {
	const draft = $state<ProfileSpecForm>({ ...EMPTY_SPEC, ...spec });
	component = mount(KitFields, {
		target: document.body,
		props: { draft, accounts, pools, usage: [], machineId: 'm1' }
	});
	flushSync();
	return draft;
}

function harnessCard(label: string): HTMLButtonElement {
	const card = [...document.querySelectorAll<HTMLButtonElement>('[role="radio"]')].find((b) =>
		b.textContent?.includes(label)
	);
	if (!card) throw new Error(`no ${label} harness card`);
	return card;
}

describe('KitFields account pick follows the harness', () => {
	it('keeps an account that backs the harness', () => {
		const draft = render({ harness: 'claude-code', account_id: 'claude' });
		expect(draft.account_id).toBe('claude');
	});

	it('drops a fireworks account saved under Claude Code back to Auto', () => {
		const draft = render({ harness: 'claude-code', account_id: 'spark' });
		expect(draft.account_id).toBeNull();
	});

	it('clears the account when the harness switch makes it incompatible', () => {
		const draft = render({ harness: 'codex', account_id: 'gpt' });
		expect(draft.account_id).toBe('gpt');
		harnessCard('Claude Code').click();
		flushSync();
		expect(draft.harness).toBe('claude-code');
		expect(draft.account_id).toBeNull();
	});

	it('clears a pool none of whose members can run the harness', () => {
		const kept = render({ harness: 'claude-code', pool_id: 'claude-pool' });
		expect(kept.pool_id).toBe('claude-pool');
		harnessCard('Codex').click();
		flushSync();
		expect(kept.pool_id).toBeNull();
	});

	it('clears a fireworks-only pool under Claude Code', () => {
		const draft = render({ harness: 'claude-code', pool_id: 'spark-pool' });
		expect(draft.pool_id).toBeNull();
	});
});
