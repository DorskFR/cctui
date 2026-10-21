// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import FailedDispatchCard from './FailedDispatchCard.svelte';
import type { FailedDispatch } from '@bindings/FailedDispatch';

vi.mock('$lib/paraglide/messages', async () => {
	const en: Record<string, string> = (await import('../../../../messages/en.json')).default;
	const render = (key: string) => (params: Record<string, string> = {}) =>
		en[key].replace(/\{(\w+)\}/g, (_, name: string) => params[name] ?? '');
	return { m: new Proxy({}, { get: (_, key: string) => render(key) }) };
});

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

async function render(d: FailedDispatch, onrerun: (id: string) => Promise<unknown> = vi.fn(async () => {})) {
	comp = mount(FailedDispatchCard, { target: document.body, props: { dispatch: d, onrerun } });
	await flush();
	return onrerun;
}

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
});
