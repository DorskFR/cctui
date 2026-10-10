import { beforeEach, describe, expect, it, vi } from 'vitest';

const calls: { verb: string; path: string; arg?: unknown }[] = [];
const reply = (verb: string) =>
	vi.fn(async (path: string, arg?: unknown) => {
		calls.push({ verb, path, arg });
		return { link: { id: 'l1' }, links: [], messages: [], enabled: true, invite: 'x' };
	});

vi.mock('./api', () => ({
	api: { get: reply('get'), post: reply('post'), patch: reply('patch') }
}));

const c = await import('./cctuiverse');

beforeEach(() => {
	calls.length = 0;
});

describe('cctuiverse api paths', () => {
	it('hits every owner endpoint at the contract path', async () => {
		await c.getConfig();
		await c.createInvite({ session: 's 1' }, 'me');
		await c.createInvite({ room: 'r1' }, 'me');
		await c.joinInvite('https://a/cctuiverse/join#v1.x', 's1', 'me');
		await c.listLinks({ session: 's1' });
		await c.listLinks({ room: 'r1' });
		await c.updateLink('l1', { inbound: 'hold' });
		await c.closeLink('l1');
		await c.listLinkMessages('l1', 'held');
		await c.actOnMessage('l1', 7, 'release');
		await c.actOnMessage('l1', 7, 'drop');
		await c.actOnMessage('l1', 7, 'approve');
		expect(calls).toEqual([
			{ verb: 'get', path: '/cctuiverse/config', arg: undefined },
			{ verb: 'post', path: '/sessions/s%201/cctuiverse/invites', arg: { label: 'me' } },
			{ verb: 'post', path: '/rooms/r1/cctuiverse/invites', arg: { label: 'me' } },
			{
				verb: 'post',
				path: '/cctuiverse/join',
				arg: { invite: 'https://a/cctuiverse/join#v1.x', session_id: 's1', label: 'me' }
			},
			{ verb: 'get', path: '/sessions/s1/cctuiverse/links', arg: undefined },
			{ verb: 'get', path: '/rooms/r1/cctuiverse/links', arg: undefined },
			{ verb: 'patch', path: '/cctuiverse/links/l1', arg: { inbound: 'hold' } },
			{ verb: 'post', path: '/cctuiverse/links/l1/close', arg: undefined },
			{ verb: 'get', path: '/cctuiverse/links/l1/messages', arg: { status: 'held' } },
			{ verb: 'post', path: '/cctuiverse/links/l1/messages/7/release', arg: undefined },
			{ verb: 'post', path: '/cctuiverse/links/l1/messages/7/drop', arg: undefined },
			{ verb: 'post', path: '/cctuiverse/links/l1/messages/7/approve', arg: undefined }
		]);
	});
});

describe('findInvite / stripInvite', () => {
	const url = 'https://cctui.example.com/cctuiverse/join#v1.6f1c.QUJD.ZGVm';

	it('finds an invite anywhere in the prompt', () => {
		expect(c.findInvite(`join this ${url} and say hi`)).toBe(url);
		expect(c.findInvite(url)).toBe(url);
		expect(c.findInvite('http://box:8080/sub/cctuiverse/join#v1.a.b.c')).toBe(
			'http://box:8080/sub/cctuiverse/join#v1.a.b.c'
		);
	});

	it('ignores non-invites', () => {
		expect(c.findInvite('https://cctui.example.com/cctuiverse/join')).toBeNull();
		expect(c.findInvite('https://cctui.example.com/cctuiverse/join#v2.a')).toBeNull();
		expect(c.findInvite('ftp://x/cctuiverse/join#v1.a')).toBeNull();
		expect(c.findInvite('hello')).toBeNull();
	});

	it('strips the invite and keeps the rest', () => {
		expect(c.stripInvite(`join this ${url} and say hi`, url, 'fb')).toBe('join this  and say hi');
	});

	it('falls back when only the invite was typed', () => {
		expect(c.stripInvite(`  ${url}\n`, url, 'fb')).toBe('fb');
	});
});

describe('link helpers', () => {
	const at = Date.parse('2026-01-01T00:00:00Z');

	it('maps expiry choices', () => {
		expect(c.expiryFor('never', at)).toBeNull();
		expect(c.expiryFor('24h', at)).toBe('2026-01-02T00:00:00.000Z');
		expect(c.expiryFor('7d', at)).toBe('2026-01-08T00:00:00.000Z');
	});

	it('counts down and floors at zero', () => {
		expect(c.remainingMs(null, at)).toBeNull();
		expect(c.remainingMs('2026-01-01T00:01:00Z', at)).toBe(60_000);
		expect(c.remainingMs('2025-12-31T00:00:00Z', at)).toBe(0);
		expect(c.formatCountdown(3_900_000)).toBe('1h 05m');
		expect(c.formatCountdown(249_000)).toBe('4m 09s');
	});

	it('keeps only open links', () => {
		const l = (id: string, state: 'pending' | 'active' | 'closed') =>
			({ id, state }) as unknown as c.LinkView;
		expect(c.openLinks([l('a', 'active'), l('b', 'closed'), l('c', 'pending')]).map((x) => x.id)).toEqual(['a', 'c']);
	});
});
