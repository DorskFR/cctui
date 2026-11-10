// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/queries', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/queries')>()),
	useAccounts: () => ({ data: undefined })
}));
import type { Label } from '@bindings/Label';
import type { SessionListItem } from '@bindings/SessionListItem';
import { mount, unmount } from 'svelte';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import SessionCard from './SessionCard.svelte';

let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	setCapabilitiesForTest(null);
});

const ops = { id: 'l1', name: 'ops', color: '#888888' } as Label;

function session(): SessionListItem {
	return {
		id: 'sess-1',
		parent_id: null,
		machine_id: 'm1',
		machine_name: 'dev1',
		working_dir: '/srv/project',
		status: 'active',
		liveness: 'active',
		bucket: 'working',
		adapter_id: 'claude-code',
		token_usage: { tokens_in: 10, tokens_out: 5, cache_read_tokens: 0, cache_creation_tokens: 0, cost_usd: 0 },
		model: 'claude-opus-4-8',
		auto_approve: false,
		cache_cold: false,
		hibernated: false,
		has_token_credentials: false,
		account_traffic_observed: false,
		pr_links: [],
		last_message_text: 'hello',
		pinned: true,
		labels: [ops],
		unread_count: 0,
		tool_use_count: 0,
		metadata: {}
	} as unknown as SessionListItem;
}

function render(variant: 'row' | 'card', organising: boolean) {
	setCapabilitiesForTest({ features: organising ? ['organising'] : [], switchable: ['organising'] });
	const onTogglePin = vi.fn();
	const onAttachLabel = vi.fn();
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(SessionCard, {
		target: host,
		props: {
			session: session(),
			variant,
			onopen: () => {},
			onTogglePin,
			allLabels: [ops],
			onAttachLabel,
			onDetachLabel: vi.fn()
		}
	});
	return { host, onTogglePin, onAttachLabel };
}

describe('SessionCard without organising', () => {
	for (const variant of ['row', 'card'] as const) {
		it(`shows no star and no label chips (${variant})`, () => {
			const { host } = render(variant, false);
			expect(host.querySelector('.star')).toBeNull();
			expect(host.querySelector('.labels')).toBeNull();
			expect(host.textContent).not.toContain('ops');
		});

		it(`keeps the star and the label chips with organising on (${variant})`, () => {
			const { host } = render(variant, true);
			expect(host.querySelector('.star')).not.toBeNull();
			expect(host.querySelector('.labels')).not.toBeNull();
		});
	}
});
