// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('$app/environment', () => ({ browser: true }));

const get = vi.fn();
const put = vi.fn();
const del = vi.fn();
vi.mock('./api', () => ({ api: { get, put, del } }));

/** `serverDrafts.load()` runs once per module instance, and it installs itself
 *  into `drafts` on import, so both modules are re-imported together per test. */
async function fresh() {
	vi.resetModules();
	const draftsMod = await import('./drafts');
	const serverMod = await import('./serverDrafts');
	return { ...draftsMod, serverDrafts: serverMod.serverDrafts };
}

beforeEach(() => {
	localStorage.clear();
	get.mockReset().mockResolvedValue({ drafts: [] });
	put.mockReset().mockResolvedValue(undefined);
	del.mockReset().mockResolvedValue(undefined);
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('roaming writes', () => {
	it('debounces a composer draft into a single PUT of the last value', async () => {
		const { drafts, composerKey } = await fresh();
		const key = composerKey('s1');
		drafts.set(key, 'h');
		drafts.set(key, 'he');
		drafts.set(key, 'hello');
		expect(put).not.toHaveBeenCalled();

		vi.runAllTimers();

		expect(put).toHaveBeenCalledTimes(1);
		expect(put.mock.calls[0][0]).toBe(`/drafts/${encodeURIComponent(key)}`);
		expect(put.mock.calls[0][1]).toEqual({ text: 'hello' });
	});

	it('percent-encodes a spawn-slot key so its separators and slashes survive', async () => {
		const { drafts } = await fresh();
		drafts.set('cctui_spawn_draft\u001fm1\u001f/home/x', '{}');
		vi.runAllTimers();

		expect(put.mock.calls[0][0]).toBe('/drafts/cctui_spawn_draft%1Fm1%1F%2Fhome%2Fx');
	});

	it('deletes rather than storing an empty draft, and cancels a queued PUT', async () => {
		const { drafts, composerKey } = await fresh();
		const key = composerKey('s2');
		drafts.set(key, 'typed');
		drafts.set(key, '');
		vi.runAllTimers();

		expect(put).not.toHaveBeenCalled();
		expect(del).toHaveBeenCalledWith(`/drafts/${encodeURIComponent(key)}`);
	});

	it('leaves device-local UI state out of the server store', async () => {
		const { drafts, VIEW_OPTS } = await fresh();
		drafts.set(VIEW_OPTS, '{"paneWidth":300}');
		drafts.clear(VIEW_OPTS);
		vi.runAllTimers();

		expect(put).not.toHaveBeenCalled();
		expect(del).not.toHaveBeenCalled();
	});

	it('routes the per-session and prompt histories to the server too', async () => {
		const { history, promptHistory } = await fresh();
		history.push('s9', 'a reply');
		promptHistory.push('a spawn prompt');
		vi.runAllTimers();

		const paths = put.mock.calls.map((c) => c[0] as string);
		expect(paths).toContain(`/drafts/${encodeURIComponent('cctui_history_s9')}`);
		expect(paths).toContain(`/drafts/${encodeURIComponent('cctui_prompt_history')}`);
	});

	it('flushes a queued PUT with keepalive, so a reload cannot lose it', async () => {
		const { drafts, composerKey, serverDrafts } = await fresh();
		drafts.set(composerKey('s3'), 'unsent');
		serverDrafts.flush();

		expect(put).toHaveBeenCalledTimes(1);
		expect(put.mock.calls[0][2]).toEqual({ keepalive: true });
	});

	it('stops touching the server after a logout wipe', async () => {
		const { drafts, composerKey, clearCctuiStorage } = await fresh();
		clearCctuiStorage();
		drafts.set(composerKey('s4'), 'typed after logout');
		vi.runAllTimers();

		expect(put).not.toHaveBeenCalled();
		expect(del).not.toHaveBeenCalled();
	});
});

describe('load', () => {
	it('adopts the server copy and imports this browser once', async () => {
		const { drafts, composerKey, serverDrafts } = await fresh();
		const mine = composerKey('local-only');
		const theirs = composerKey('from-phone');
		drafts.set(mine, 'written here');
		put.mockClear();
		get.mockResolvedValue({
			drafts: [{ key: theirs, text: 'written there', updated_at: '2026-01-01T00:00:00Z' }]
		});

		await serverDrafts.load();
		vi.runAllTimers();

		expect(drafts.get(theirs)).toBe('written there');
		expect(put).toHaveBeenCalledTimes(1);
		expect(put.mock.calls[0][1]).toEqual({ text: 'written here' });
	});

	it('is idempotent: a second call does not refetch', async () => {
		const { serverDrafts } = await fresh();
		await serverDrafts.load();
		await serverDrafts.load();

		expect(get).toHaveBeenCalledTimes(1);
	});

	it('keeps the cached copy when the server is unreachable', async () => {
		const { drafts, composerKey, serverDrafts } = await fresh();
		const key = composerKey('offline');
		drafts.set(key, 'kept');
		get.mockRejectedValue(new Error('offline'));

		await serverDrafts.load();

		expect(drafts.get(key)).toBe('kept');
	});

	it('does not re-import a draft the server no longer has', async () => {
		const { PROMPT_HISTORY, serverDrafts } = await fresh();
		localStorage.setItem('cctui_drafts_imported', '1');
		localStorage.setItem(PROMPT_HISTORY, '["deleted elsewhere"]');

		await serverDrafts.load();
		vi.runAllTimers();

		expect(put).not.toHaveBeenCalled();
	});

	it('does not overwrite the local copy of a key the server also holds', async () => {
		const { drafts, composerKey, serverDrafts } = await fresh();
		const key = composerKey('both');
		drafts.set(key, 'local');
		put.mockClear();
		get.mockResolvedValue({
			drafts: [{ key, text: 'server', updated_at: '2026-01-01T00:00:00Z' }]
		});

		await serverDrafts.load();
		vi.runAllTimers();

		expect(drafts.get(key)).toBe('server');
		expect(put).not.toHaveBeenCalled();
	});
});
