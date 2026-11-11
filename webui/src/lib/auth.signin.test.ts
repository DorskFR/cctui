// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { apiFetch, bootCapabilities, refreshCapabilities, getAssertion } = vi.hoisted(() => ({
	apiFetch: vi.fn<(url: string, init?: RequestInit) => Promise<Response>>(),
	bootCapabilities: vi.fn<() => Promise<unknown>>(),
	refreshCapabilities: vi.fn<() => Promise<unknown>>(),
	getAssertion: vi.fn(async () => ({ id: 'cred' }))
}));

vi.mock('$app/environment', () => ({ browser: true }));
vi.mock('./api', () => ({ apiFetch }));
vi.mock('./workspaceSupport', () => ({ bootCapabilities, refreshCapabilities }));
vi.mock('./passkeys', () => ({ getAssertion, passkeysSupported: () => true }));

const { auth } = await import('./auth.svelte');

const core = { workspace: true, features: [], switchable: ['organising'] };
const ok = () => Response.json({});
const refused = () => new Response('', { status: 401 });

function holdRefresh() {
	const seen: boolean[] = [];
	let settle!: () => void;
	refreshCapabilities.mockImplementation(() => {
		seen.push(auth.isAuthed);
		return new Promise((resolve) => (settle = () => resolve(core)));
	});
	return { seen, settle: () => settle() };
}

beforeEach(() => {
	apiFetch.mockReset();
	bootCapabilities.mockReset().mockResolvedValue(null);
	refreshCapabilities.mockReset().mockResolvedValue(core);
	auth.isAuthed = false;
	auth.checking = true;
});

describe('token sign-in', () => {
	it('reads the capabilities before the session counts as signed in', async () => {
		apiFetch.mockResolvedValueOnce(ok());
		const held = holdRefresh();
		const login = auth.login('tok');
		await vi.waitFor(() => expect(refreshCapabilities).toHaveBeenCalledOnce());
		expect(held.seen).toEqual([false]);
		expect(auth.isAuthed).toBe(false);
		held.settle();
		expect(await login).toBe(true);
		expect(auth.isAuthed).toBe(true);
	});

	it('does not probe on a refused token', async () => {
		apiFetch.mockResolvedValueOnce(refused());
		expect(await auth.login('bad')).toBe(false);
		expect(refreshCapabilities).not.toHaveBeenCalled();
		expect(auth.isAuthed).toBe(false);
	});

	it('reads them again after signing out and back in', async () => {
		apiFetch.mockImplementation(async () => ok());
		await auth.login('first');
		await auth.logout();
		expect(auth.isAuthed).toBe(false);
		await auth.login('second');
		expect(refreshCapabilities).toHaveBeenCalledTimes(2);
		expect(auth.isAuthed).toBe(true);
	});
});

describe('passkey sign-in', () => {
	const challenge = () => Response.json({ challenge_id: 'c1', options: {} });

	it('reads the capabilities before the session counts as signed in', async () => {
		apiFetch.mockResolvedValueOnce(challenge()).mockResolvedValueOnce(ok());
		const held = holdRefresh();
		const login = auth.loginWithPasskey();
		await vi.waitFor(() => expect(refreshCapabilities).toHaveBeenCalledOnce());
		expect(held.seen).toEqual([false]);
		held.settle();
		expect(await login).toBe(true);
		expect(auth.isAuthed).toBe(true);
	});

	it('does not probe when the server refuses the assertion', async () => {
		apiFetch.mockResolvedValueOnce(challenge()).mockResolvedValueOnce(refused());
		expect(await auth.loginWithPasskey()).toBe(false);
		expect(refreshCapabilities).not.toHaveBeenCalled();
	});
});

describe('startup with a cookie session', () => {
	it('retries a boot probe that failed once the session is known good', async () => {
		apiFetch.mockResolvedValueOnce(ok());
		const held = holdRefresh();
		const init = auth.init();
		await vi.waitFor(() => expect(refreshCapabilities).toHaveBeenCalledOnce());
		expect(held.seen).toEqual([false]);
		expect(auth.checking).toBe(true);
		held.settle();
		await init;
		expect(auth.isAuthed).toBe(true);
		expect(auth.checking).toBe(false);
	});

	it('keeps the boot answer when it already came through', async () => {
		bootCapabilities.mockResolvedValue(core);
		apiFetch.mockResolvedValueOnce(ok());
		await auth.init();
		expect(refreshCapabilities).not.toHaveBeenCalled();
		expect(auth.isAuthed).toBe(true);
	});

	it('does not probe without a session', async () => {
		apiFetch.mockResolvedValueOnce(refused());
		await auth.init();
		expect(refreshCapabilities).not.toHaveBeenCalled();
		expect(auth.isAuthed).toBe(false);
	});
});
