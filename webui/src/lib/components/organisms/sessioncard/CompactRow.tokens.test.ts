// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('#lib/queries/index.ts', async (importOriginal) => ({
	...(await importOriginal<typeof import('#lib/queries/index.ts')>()),
	useAccounts: () => ({ data: undefined })
}));
import type { SessionListItem } from '@bindings/SessionListItem';
import { flushSync, mount, unmount } from 'svelte';
import SessionCard from '../SessionCard.svelte';

let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function row(extra: Partial<SessionListItem> = {}): HTMLElement {
	const session = {
		id: 'sess-1',
		parent_id: null,
		machine_id: 'm1',
		machine_name: 'dev1',
		working_dir: '/w/app',
		status: 'active',
		liveness: 'active',
		bucket: 'working',
		token_usage: {
			tokens_in: 1_000_000,
			tokens_out: 58_000,
			cache_read_tokens: 23_000_000,
			cache_creation_tokens: 942_000,
			cost_usd: 2.38
		},
		metadata: {},
		adapter_id: 'claude-code',
		model: 'claude-opus-4-8',
		effort: 'high',
		auto_approve: false,
		cache_cold: false,
		hibernated: false,
		pinned: false,
		labels: [],
		unread_count: 0,
		tool_use_count: 0,
		has_token_credentials: false,
		account_traffic_observed: false,
		pr_links: [],
		last_message_text: 'hello',
		...extra
	} as SessionListItem;
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(SessionCard, { target: host, props: { session, onopen: () => {}, variant: 'row' } });
	flushSync();
	return host;
}

describe('list row token total', () => {
	it('shows the bare compact total, without a "tokens" word', () => {
		const tokens = row().querySelector('.tokens');
		expect(tokens?.textContent?.trim()).toMatch(/^25(\.0)?M$/);
		expect(tokens?.getAttribute('title')).toBe(`Σ ${(25_000_000).toLocaleString()}`);
	});

	it('keeps the cold-cache marker beside the total', () => {
		const cold = row({ cache_cold: true }).querySelector('.tokens .cold');
		expect(cold?.textContent).toBe('❄️');
		expect(cold?.getAttribute('aria-label')).toMatch(/cold/i);
	});

	it('shows no cold marker on a warm cache', () => {
		expect(row().querySelector('.tokens .cold')).toBeNull();
	});
});
