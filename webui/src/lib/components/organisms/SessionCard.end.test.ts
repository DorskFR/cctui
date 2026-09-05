import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/queries', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/queries')>()),
	useAccounts: () => ({ data: undefined })
}));
import { mount, unmount } from 'svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import SessionCard from './SessionCard.svelte';

let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function session(extra: Partial<SessionListItem>): SessionListItem {
	return {
		id: 'sess-1',
		parent_id: null,
		machine_id: 'm1',
		working_dir: '/w',
		status: 'inactive',
		liveness: 'dead',
		bucket: 'working',
		token_usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 },
		metadata: {},
		adapter_id: 'claude-code',
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
		...extra
	} as SessionListItem;
}

function render(s: SessionListItem): HTMLElement {
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(SessionCard, { target: host, props: { session: s, onopen: () => {} } });
	return host;
}

describe('SessionCard end-of-life badge', () => {
	it('renders the crash reason with its detail in the tooltip', () => {
		const el = render(
			session({
				end_reason: 'crashed',
				end_detail: 'claude -p exited (exit status: 1); last stderr:\nboom',
				ended_at: '2026-09-04T10:00:00Z'
			})
		);
		const badge = el.querySelector('.end-badge');
		expect(badge).not.toBeNull();
		expect(badge?.textContent?.trim()).toBe('crashed');
		expect(badge?.getAttribute('title')).toContain('exit status: 1');
		expect(badge?.classList.contains('end-muted')).toBe(false);
	});

	it('renders a failed spawn with its detail in the badge and the tooltip', () => {
		const el = render(
			session({
				adapter_id: 'codex',
				end_reason: 'spawn_failed',
				end_detail: 'unknown model gpt-nope; available: gpt-5-codex',
				ended_at: '2026-09-04T10:00:00Z'
			})
		);
		const badge = el.querySelector('.end-badge');
		expect(badge?.textContent?.trim()).toBe('failed: unknown model gpt-nope; available: gpt-5-codex');
		expect(badge?.getAttribute('title')).toContain('available: gpt-5-codex');
	});

	it('fades a reaped session and shows nothing for a live one', () => {
		const reaped = render(session({ end_reason: 'reaped_inactive', ended_at: '2026-09-04T10:00:00Z' }));
		const badge = reaped.querySelector('.end-badge');
		expect(badge?.textContent?.trim()).toBe('reaped');
		expect(badge?.classList.contains('end-muted')).toBe(true);
		if (comp) unmount(comp);
		comp = null;
		const live = render(session({ status: 'active', liveness: 'active' }));
		expect(live.querySelector('.end-badge')).toBeNull();
	});
});
