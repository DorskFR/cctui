// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { AccountProvider } from '#lib/queries/index.ts';

vi.mock('#lib/queries/index.ts', () => ({ useAccountActions: () => ({}) }));
vi.mock('#lib/components/molecules/UsageBars.svelte', () => ({ default: () => {} }));
vi.mock('#lib/components/molecules/LimitResetButton.svelte', () => ({ default: () => {} }));

const { default: ProviderColumn } = await import('./ProviderColumn.svelte');

let comps: ReturnType<typeof mount>[] = [];
afterEach(() => {
	for (const c of comps) unmount(c);
	comps = [];
	document.body.innerHTML = '';
});

const provider = (id: string, kind: string) =>
	({
		id,
		account_id: 'acct',
		provider: kind,
		managed: false,
		header_pin: false,
		needs_reauth: false,
		request_count: 0,
		last_used_at: null
	}) as unknown as AccountProvider;

function renderRow(divided: boolean) {
	const row = document.createElement('div');
	document.body.append(row);
	for (const p of [provider('a', 'anthropic'), provider('b', 'openai')]) {
		comps.push(mount(ProviderColumn, { target: row, props: { provider: p, divided } }));
	}
	return [...row.querySelectorAll<HTMLElement>('.column')];
}

describe('ProviderColumn divider', () => {
	it('marks columns as divided when the card asks for separators', () => {
		const cols = renderRow(true);
		expect(cols).toHaveLength(2);
		expect(cols.every((c) => c.classList.contains('divided'))).toBe(true);
	});

	it('draws no divider by default', () => {
		const cols = renderRow(false);
		expect(cols.some((c) => c.classList.contains('divided'))).toBe(false);
	});
});
