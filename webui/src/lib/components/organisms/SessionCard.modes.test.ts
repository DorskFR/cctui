// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/queries', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/queries')>()),
	useAccounts: () => ({ data: undefined })
}));
import type { SessionListItem } from '@bindings/SessionListItem';
import { mount, unmount } from 'svelte';
import SessionCard from './SessionCard.svelte';

let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function session(extra: Partial<SessionListItem> = {}): SessionListItem {
	return {
		id: 'sess-1',
		parent_id: null,
		machine_id: 'm1',
		machine_name: 'dev1',
		working_dir: '/home/dorsk/Documents/cctui',
		status: 'active',
		liveness: 'active',
		bucket: 'working',
		token_usage: { tokens_in: 10, tokens_out: 5, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0.5 },
		metadata: { git_branch: 'cct-925-sessions-modes' },
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
		pr_links: ['https://github.com/DorskFR/cctui/pull/311'],
		last_message_text: 'hello',
		...extra
	} as SessionListItem;
}

function render(props: Record<string, unknown>): HTMLElement {
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(SessionCard, { target: host, props: { session: session(), onopen: () => {}, ...props } });
	return host;
}

describe('SessionCard modes', () => {
	it('the compact row gives the name its own line and puts machine · project · branch · model in muted sans below', () => {
		const el = render({ variant: 'row', pendingCount: 2 });
		expect(el.querySelector('.sc-wrap.compact')).not.toBeNull();
		expect(el.querySelector('.title.row')).not.toBeNull();
		expect(el.querySelector('[data-tsu="WorkingDir"]')).toBeNull();
		expect(el.querySelector('.line2 .meta')?.textContent?.trim()).toBe(
			'dev1 · cctui · cct-925-sessions-modes · opus-4-8'
		);
		expect(el.querySelector('.line2 .preview')?.textContent?.trim()).toBe('hello');
		expect(el.textContent).toContain('2 perm');
		expect(el.textContent).not.toContain('Σ');
	});

	it('hides the machine when the section header already names it', () => {
		const el = render({ variant: 'row', showMachine: false });
		expect(el.querySelector('.line2 .meta')?.textContent?.trim()).toBe(
			'cctui · cct-925-sessions-modes · opus-4-8'
		);
	});

	it('the detailed card footer reads project · branch in sans, with the PR link', () => {
		const el = render({ variant: 'card' });
		expect(el.querySelector('.preview')?.textContent).toBe('hello');
		const where = el.querySelector('.where');
		expect(where?.textContent?.trim()).toBe('cctui · cct-925-sessions-modes');
		expect(where?.getAttribute('title')).toContain('/home/dorsk/Documents/cctui');
		expect(el.querySelector('[data-tsu="WorkingDir"]')).toBeNull();
		const pr = el.querySelector('a.pr-link');
		expect(pr?.textContent?.trim()).toBe('DorskFR/cctui#311');
		expect(pr?.getAttribute('href')).toBe('https://github.com/DorskFR/cctui/pull/311');
		expect(el.querySelector('.model .full')?.textContent).toBe('opus-4-8');
	});

	it('is the sess-card size container every readout degrades against', () => {
		const el = render({ variant: 'card' });
		expect(el.querySelector('.sc-wrap')).not.toBeNull();
	});
});
