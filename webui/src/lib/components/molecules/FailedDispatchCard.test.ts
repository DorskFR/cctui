// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import FailedDispatchCard from './FailedDispatchCard.svelte';
import type { FailedDispatch } from '@bindings/FailedDispatch';

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const text = () => document.body.textContent ?? '';
const rerunButton = () =>
	[...document.querySelectorAll('button')].find((b) => b.textContent?.includes('Re-dispatch'));

function dispatch(over: Partial<FailedDispatch> = {}): FailedDispatch {
	return {
		session_id: '0d4c1f2e-9a7b-4c3d-8e1f-2a3b4c5d6e7f',
		dispatcher: 'kube',
		name: 'triage-PROJ-9',
		profile: 'tonbo',
		repo: 'cctui',
		model: 'opus',
		prompt_head: 'fix the flaky test',
		env_keys: [],
		reason: 'never_registered',
		dispatched_at: '2026-10-20T08:00:00Z',
		...over
	} as FailedDispatch;
}

async function render(
	d: FailedDispatch,
	onrerun: (id: string) => Promise<unknown> = vi.fn(async () => {}),
	ondismiss?: (id: string) => Promise<unknown>
) {
	comp = mount(FailedDispatchCard, { target: document.body, props: { dispatch: d, onrerun, ondismiss } });
	await flush();
	return onrerun;
}

const dismissButton = () => document.querySelector<HTMLButtonElement>('button[aria-label="Dismiss"]');

describe('FailedDispatchCard', () => {
	it('identifies a worker that never started and re-dispatches it by its session id', async () => {
		const onrerun = await render(dispatch());
		expect(text()).toContain('never started');
		expect(text()).toContain('triage-PROJ-9');
		expect(text()).toContain('kube · tonbo · cctui · opus');
		expect(document.querySelector('pre.prompt')?.textContent).toBe('fix the flaky test');
		rerunButton()?.click();
		await flush();
		expect(onrerun).toHaveBeenCalledWith('0d4c1f2e-9a7b-4c3d-8e1f-2a3b4c5d6e7f');
	});

	it('a dispatch that needed env offers no one-click re-run and names the missing keys', async () => {
		await render(dispatch({ reason: 'refused', detail: 'quota exceeded', env_keys: ['GITHUB_TOKEN'], rerun_blocked: 'env' }));
		expect(text()).toContain('refused');
		expect(document.querySelector('pre.detail')?.textContent).toBe('quota exceeded');
		expect(text()).toContain('GITHUB_TOKEN');
		expect(rerunButton()).toBeUndefined();
	});

	it('shows why a re-dispatch failed', async () => {
		await render(dispatch({ reason: 'oom_killed' }), async () => {
			throw new Error('dispatch policy violated (daily_cap)');
		});
		expect(text()).toContain('out of memory');
		rerunButton()?.click();
		await flush();
		await flush();
		expect(text()).toContain('daily_cap');
	});
	it('dismisses by session id, and offers no dismiss without a handler', async () => {
		const ondismiss = vi.fn(async () => {});
		await render(dispatch(), undefined, ondismiss);
		dismissButton()?.click();
		await flush();
		expect(ondismiss).toHaveBeenCalledWith('0d4c1f2e-9a7b-4c3d-8e1f-2a3b4c5d6e7f');
		unmount(comp!);
		comp = null;
		await render(dispatch());
		expect(dismissButton()).toBeNull();
	});

	it('a superseded failure resolves to a link to the run that replaced it, keeping the reason', async () => {
		await render(dispatch({ reason: 'crashed', superseded_by: 'b7e2c9d1-0000-4000-8000-000000000000' }));
		expect(text()).toContain('resolved');
		expect(text()).toContain('crashed');
		expect(text()).toContain('Superseded by');
		const link = document.querySelector('a');
		expect(link?.getAttribute('href')).toBe('/sessions/b7e2c9d1-0000-4000-8000-000000000000');
		expect(link?.textContent).toBe('b7e2c9d1');
		expect(rerunButton()).toBeUndefined();
	});

	it('shows how the worker container ended and its last log lines', async () => {
		await render(
			dispatch({
				reason: 'crashed',
				detail: 'Job has reached the specified backoff limit',
				worker_exit: { container: 'worker', exit_code: 137, reason: 'Error', signal: 9, log_tail: 'npm ERR! boom' }
			})
		);
		expect(document.querySelector('pre.detail')?.textContent).toBe('Job has reached the specified backoff limit');
		expect(text()).toContain('Container worker exited with code 137 (Error · signal 9)');
		expect(document.querySelector('pre.log')?.textContent).toBe('npm ERR! boom');
	});

	it('names the missing permission when the logs could not be read', async () => {
		await render(
			dispatch({
				reason: 'crashed',
				worker_exit: { exit_code: 1, logs_error: 'needs `get` on `pods/log`' }
			})
		);
		expect(document.querySelector('pre.log')).toBeNull();
		expect(text()).toContain('Worker logs unavailable: needs `get` on `pods/log`');
	});
});
