// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	updateInstance: vi.fn(),
	selfUpdateTarget: vi.fn(),
	setSelfUpdateTarget: vi.fn()
}));
const version = vi.hoisted(() => ({ data: { instance_name: 'old' } as { instance_name: string } | undefined }));
const invalidateQueries = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('#lib/queries/index.ts', () => ({
	endpoints: api,
	qk: { version: ['version'] },
	useVersion: () => version,
	useAllMachines: () => ({ data: [] })
}));
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: () => ({ invalidateQueries }) }));
vi.mock('#lib/features.svelte.ts', () => ({ hasFeature: (f: string) => f === 'self_update' }));

import InstanceSection from './InstanceSection.svelte';
import { serverSave } from './serverSave.svelte.ts';
import { m } from '#lib/paraglide/messages.js';

let comp: ReturnType<typeof mount> | null = null;
beforeEach(() => {
	vi.useFakeTimers();
	api.updateInstance.mockResolvedValue({ name: 'new' });
	api.selfUpdateTarget.mockResolvedValue({
		source: 'settings',
		target: { machine_id: 'm1', working_dir: '/srv', adapter_id: 'claude-code' }
	});
	api.setSelfUpdateTarget.mockImplementation(async (t) => ({ source: t ? 'settings' : 'none', target: t }));
});
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	vi.useRealTimers();
	vi.clearAllMocks();
	document.body.innerHTML = '';
});

async function render() {
	comp = mount(InstanceSection, { target: document.body, props: { isAdmin: true } });
	flushSync();
	await vi.runAllTimersAsync();
	await tick();
}
const nameInput = () =>
	[...document.body.querySelectorAll<HTMLInputElement>('input')].find((i) => i.maxLength === 48);
const type = (el: HTMLInputElement, v: string) => {
	el.value = v;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
};

describe('InstanceSection auto-save', () => {
	it('has no Save button', async () => {
		await render();
		const labels = [...document.body.querySelectorAll('button')].map((b) => b.textContent?.trim());
		expect(labels).not.toContain('Save');
	});

	it('saves the instance name after a typing pause, once', async () => {
		await render();
		const input = nameInput();
		if (!input) throw new Error('no instance name input');
		input.dispatchEvent(new FocusEvent('focus'));
		type(input, ' new ');
		expect(api.updateInstance).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(700);
		input.dispatchEvent(new FocusEvent('blur'));
		await vi.runAllTimersAsync();
		expect(api.updateInstance).toHaveBeenCalledTimes(1);
		expect(api.updateInstance).toHaveBeenCalledWith('new');
		expect(serverSave.status).toBe('saved');
	});

	it('saves the self-update directory after a typing pause', async () => {
		await render();
		const dir = [...document.body.querySelectorAll<HTMLInputElement>('input')].find((i) => i.value === '/srv');
		if (!dir) throw new Error('no directory input');
		type(dir, '/opt/cctui');
		await vi.advanceTimersByTimeAsync(700);
		expect(api.setSelfUpdateTarget).toHaveBeenCalledWith({
			machine_id: 'm1',
			working_dir: '/opt/cctui',
			adapter_id: 'claude-code'
		});
	});

	it('restarts the wait on every keystroke and saves the settled directory', async () => {
		await render();
		const dir = [...document.body.querySelectorAll<HTMLInputElement>('input')].find((i) => i.value === '/srv');
		if (!dir) throw new Error('no directory input');
		for (const v of ['/o', '/op', '/opt', '/opt/c', '/opt/cctui']) {
			type(dir, v);
			await vi.advanceTimersByTimeAsync(300);
		}
		expect(api.setSelfUpdateTarget).not.toHaveBeenCalled();
		await vi.advanceTimersByTimeAsync(400);
		expect(api.setSelfUpdateTarget).toHaveBeenCalledTimes(1);
		expect(api.setSelfUpdateTarget).toHaveBeenCalledWith({
			machine_id: 'm1',
			working_dir: '/opt/cctui',
			adapter_id: 'claude-code'
		});
	});

	it('saves only the settled instance name while typing', async () => {
		await render();
		const input = nameInput();
		if (!input) throw new Error('no instance name input');
		input.dispatchEvent(new FocusEvent('focus'));
		for (const v of ['p', 'pr', 'pro', 'prod']) {
			type(input, v);
			await vi.advanceTimersByTimeAsync(300);
		}
		await vi.advanceTimersByTimeAsync(400);
		expect(api.updateInstance).toHaveBeenCalledTimes(1);
		expect(api.updateInstance).toHaveBeenCalledWith('prod');
	});

	it('sends a pending edit when the page goes away before the pause ends', async () => {
		await render();
		const dir = [...document.body.querySelectorAll<HTMLInputElement>('input')].find((i) => i.value === '/srv');
		if (!dir) throw new Error('no directory input');
		type(dir, '/opt');
		if (comp) unmount(comp);
		comp = null;
		expect(api.setSelfUpdateTarget).toHaveBeenCalledWith({
			machine_id: 'm1',
			working_dir: '/opt',
			adapter_id: 'claude-code'
		});
	});

	it('does not save an empty directory', async () => {
		await render();
		const dir = [...document.body.querySelectorAll<HTMLInputElement>('input')].find((i) => i.value === '/srv');
		if (!dir) throw new Error('no directory input');
		type(dir, '  ');
		await vi.advanceTimersByTimeAsync(700);
		expect(api.setSelfUpdateTarget).not.toHaveBeenCalled();
	});
});

describe('InstanceSection labels', () => {
	it('marks the section and the instance name row as server-wide and admin-only', async () => {
		await render();
		const leaves = [...document.body.querySelectorAll('*')]
			.filter((el) => el.children.length === 0)
			.map((el) => el.textContent?.trim());
		expect(leaves.filter((t) => t === m.settings_scope_admin()).length).toBeGreaterThanOrEqual(2);
		expect(leaves).toContain(m.settings_scope_server());
	});

	it('names the self-update row once', async () => {
		await render();
		const text = document.body.textContent ?? '';
		expect(text.split(m.settings_self_update_label()).length - 1).toBe(1);
	});
});
