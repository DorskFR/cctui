// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { RedteamOverview } from '@bindings/RedteamOverview';
import type { RedteamRun } from '@bindings/RedteamRun';

const http = vi.hoisted(() => ({ get: vi.fn() }));
vi.mock('#lib/api.ts', async (importOriginal) => ({
	...(await importOriginal<typeof import('#lib/api.ts')>()),
	api: http
}));

import { ApiError } from '#lib/api.ts';
import { m } from '#lib/paraglide/messages.js';
import RedteamSection from './RedteamSection.svelte';
import SectionHost from './section.host.test.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.replaceChildren();
	vi.clearAllMocks();
});

const settle = async () => {
	for (let i = 0; i < 6; i++) await tick();
	flushSync();
};
const text = () => document.body.textContent ?? '';

const t2: RedteamRun = {
	id: 1,
	received_at: 1_700_000_100,
	posted_by: 'redteam',
	escapes: [{ cell: 'codex/kube/transparent/root', probe: 'EGR-04' }],
	report: {
		schema: 1,
		tier: 't2',
		lane: '',
		run_id: '0123456789ab',
		version: '0.32.0',
		verdict: 'escaped',
		model: null,
		started_at: null,
		finished_at: 1_700_000_000,
		run_url: 'https://github.com/example/repo/actions/runs/1',
		cells: [
			{
				cell: 'claude-code/kube/transparent/root',
				verdict: 'contained',
				sink_hits: 0,
				probes: [{ id: 'EGR-01', verdict: 'blocked' }],
				reasons: []
			},
			{
				cell: 'codex/kube/transparent/root',
				verdict: 'escaped',
				sink_hits: 1,
				probes: [{ id: 'EGR-04', verdict: 'escaped' }],
				reasons: []
			}
		],
		reasons: []
	}
};

const t3: RedteamRun = {
	...t2,
	id: 2,
	escapes: [],
	report: {
		...t2.report,
		tier: 't3',
		lane: 'claude-code',
		verdict: 'contained',
		model: 'model-x',
		run_url: null,
		cells: [{ cell: 'EGR-AGENT', verdict: 'contained', sink_hits: 0, probes: [], reasons: [] }]
	}
};

const overview = (versions: RedteamOverview['versions']): RedteamOverview => ({
	current_version: '0.32.0',
	versions
});

const render = async () => {
	comp = mount(SectionHost, { target: document.body, props: { section: RedteamSection } });
	await settle();
};

describe('RedteamSection', () => {
	it('shows the empty state when no run has been recorded', async () => {
		http.get.mockResolvedValue(overview([]));
		await render();
		expect(http.get).toHaveBeenCalledWith('/admin/redteam');
		expect(text()).toContain(m.redteam_empty_title());
	});

	it('lists escaped probes first, before every cell row', async () => {
		http.get.mockResolvedValue(overview([{ version: '0.32.0', t2, t3: [t3] }]));
		await render();
		const escapes = document.querySelector('[data-redteam-escapes]');
		expect(escapes?.textContent).toContain('EGR-04 · codex/kube/transparent/root');
		expect(escapes?.textContent).toContain(m.redteam_escapes_title({ count: 1 }));
		const body = text();
		const escapedAt = body.indexOf('EGR-04 · codex/kube/transparent/root');
		expect(escapedAt).toBeGreaterThan(-1);
		expect(escapedAt).toBeLessThan(body.indexOf('claude-code/kube/transparent/root'));
		expect(body.indexOf('codex/kube/transparent/root', escapedAt + 10)).toBeLessThan(
			body.indexOf('claude-code/kube/transparent/root')
		);
		expect(body).toContain(m.redteam_version_current({ version: '0.32.0' }));
		expect(body).toContain(m.redteam_tier_t3({ lane: 'claude-code' }));
		const link = document.querySelector<HTMLAnchorElement>('a[href="https://github.com/example/repo/actions/runs/1"]');
		expect(link?.textContent).toContain(m.redteam_run_link());
	});

	it('shows no escape block for a contained run', async () => {
		http.get.mockResolvedValue(overview([{ version: '0.31.0', t2: null, t3: [t3] }]));
		await render();
		expect(document.querySelector('[data-redteam-escapes]')).toBeNull();
		expect(text()).toContain(m.redteam_version({ version: '0.31.0' }));
		expect(text()).toContain('EGR-AGENT');
	});

	it('tells a non-admin the results are admin-only', async () => {
		http.get.mockRejectedValue(new ApiError(403, 'admin token required'));
		await render();
		expect(text()).toContain(m.redteam_forbidden());
	});
});
