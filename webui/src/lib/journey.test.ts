import { beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import { DONE_PREFIX, PROGRESS_KEY } from '@dorsk/journey/runtime';
import {
	guideParams,
	MOBILE_QUERY,
	parseDoneKey,
	requiredParams,
	settingsStorage,
	viewportVariant
} from './journey';
import { auth } from './auth.svelte';
import { mergeDefaults, settings } from './settings.svelte';

const api = vi.hoisted(() => ({
	me: vi.fn(),
	accounts: vi.fn(),
	accountPools: vi.fn(),
	sessions: vi.fn()
}));
vi.mock('$lib/queries/endpoints', () => ({ endpoints: api }));

const KEY = 'cctui_settings';

function blob() {
	return mergeDefaults(JSON.parse(localStorage.getItem(KEY) ?? 'null')).onboarding;
}

beforeEach(() => {
	auth.isAuthed = false;
	localStorage.clear();
	settings.setOnboarding({ seenVersion: {}, progress: null });
});

describe('parseDoneKey', () => {
	it('splits the runtime done marker into id and version', () => {
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list@3`)).toEqual({ id: 'sessions-list', version: 3 });
		expect(parseDoneKey(`${DONE_PREFIX}a@b@2`)).toEqual({ id: 'a@b', version: 2 });
	});
	it('rejects anything else', () => {
		expect(parseDoneKey(PROGRESS_KEY)).toBeNull();
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list`)).toBeNull();
		expect(parseDoneKey(`${DONE_PREFIX}sessions-list@x`)).toBeNull();
	});
});

describe('settingsStorage', () => {
	it('keeps progress in data.onboarding.progress', async () => {
		const record = JSON.stringify({ id: 'sessions-list', version: 1, index: 1 });
		await settingsStorage.set(PROGRESS_KEY, record);
		expect(blob().progress).toBe(record);
		expect(await settingsStorage.get(PROGRESS_KEY)).toBe(record);
		await settingsStorage.remove(PROGRESS_KEY);
		expect(blob().progress).toBeNull();
		expect(await settingsStorage.get(PROGRESS_KEY)).toBeNull();
	});

	it('keeps the done marker as seenVersion[id], matched on the exact version', async () => {
		const key = `${DONE_PREFIX}sessions-list@2`;
		expect(await settingsStorage.get(key)).toBeNull();
		await settingsStorage.set(key, '1');
		expect(blob().seenVersion).toEqual({ 'sessions-list': 2 });
		expect(await settingsStorage.get(key)).toBe('1');
		expect(await settingsStorage.get(`${DONE_PREFIX}sessions-list@3`)).toBeNull();
		await settingsStorage.remove(key);
		expect(blob().seenVersion).toEqual({});
	});

	it('ignores keys it does not own', async () => {
		await settingsStorage.set('journey:other', 'x');
		expect(await settingsStorage.get('journey:other')).toBeNull();
		expect(blob()).toEqual({ seenVersion: {}, progress: null });
	});
});

describe('requiredParams', () => {
	it('lists every {param} key a journey addresses', () => {
		expect(
			requiredParams({
				id: 'x',
				steps: [
					{ id: 'a', target: 'user[{me}]', expect: [{ visible: 'tab[keys]' }] },
					{ id: 'b', target: { role: 'tab', within: 'account[{account}]' } },
					{ id: 'c', expect: [{ count: ['pool[{pool}]/member', { min: 1 }] }, { probe: 'accounts' }] }
				]
			})
		).toEqual(['me', 'account', 'pool']);
	});
	it('ignores literal keys', () => {
		expect(requiredParams({ id: 'x', steps: [{ id: 'a', target: 'tab[machines]' }] })).toEqual([]);
	});
});

describe('viewportVariant', () => {
	it('maps the toolbar breakpoint to the mobile variant', () => {
		expect(viewportVariant((q) => q === MOBILE_QUERY)).toEqual({ viewport: 'mobile' });
		expect(viewportVariant(() => false)).toEqual({ viewport: 'desktop' });
	});
});

describe('guideParams', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		api.me.mockResolvedValue({ role: 'admin', user_id: 'u1', user_name: 'root' });
		api.accounts.mockResolvedValue([]);
		api.accountPools.mockResolvedValue([]);
		api.sessions.mockResolvedValue({ sessions: [] });
	});
	const qc = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

	it('leaves absent names out so the guide that needs them is refused', async () => {
		expect(await guideParams(qc())).toEqual({ 'var.label': '', 'var.prompt': '', me: 'root' });
	});

	it('names the first account, pool and live session', async () => {
		api.accounts.mockResolvedValue([{ name: 'main' }, { name: 'other' }]);
		api.accountPools.mockResolvedValue([{ name: 'prod' }]);
		api.sessions.mockResolvedValue({
			sessions: [
				{ id: 'd', status: 'draft', liveness: 'dead' },
				{ id: 'l', status: 'active', liveness: 'active' }
			]
		});
		expect(await guideParams(qc())).toMatchObject({ me: 'root', account: 'main', pool: 'prod', session: 'l' });
	});
});
