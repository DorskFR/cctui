// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { HarnessAutoupdateInfo } from '@bindings/HarnessAutoupdateInfo';
import type { MachineHarnessInfo } from '@bindings/MachineHarnessInfo';
import type { CodexSandbox } from '@bindings/CodexSandbox';

const api = vi.hoisted(() => ({
	harnessAutoupdate: vi.fn(),
	setHarnessAutoupdate: vi.fn(),
	setMachineHarnessAutoupdate: vi.fn(),
	restartHarness: vi.fn()
}));
vi.mock('$lib/queries', () => ({ endpoints: api }));
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));
vi.mock('$lib/harnesses.svelte', () => ({ harnessTable: () => [] }));

import { ApiError } from '$lib/api';
import { toasts } from '$lib/toast.svelte';
import HarnessUpdateGroup from './HarnessUpdateGroup.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	vi.clearAllMocks();
	document.body.innerHTML = '';
});

const policy = { enabled: false, interval_hours: 24, harnesses: [] };
const row = (id: string, codex_sandbox: CodexSandbox): MachineHarnessInfo => ({
	machine_id: id,
	name: id,
	policy: null,
	effective: policy,
	report: { versions: {}, outcomes: [], managed_by_image: false, codex_sandbox, binaries: {} },
	report_at: null
});
const info = (machines: MachineHarnessInfo[]): HarnessAutoupdateInfo => ({ instance: policy, machines });
const settle = async () => {
	for (let i = 0; i < 4; i++) await tick();
	flushSync();
};
const docsLinks = () => [...document.querySelectorAll('a[href$="codex-sandbox.md"]')];
const machineOf = (a: Element) => a.closest('li.machine')?.querySelector('li.machine > *')?.textContent;

describe('HarnessUpdateGroup codex sandbox badge', () => {
	it('links the fix docs only when user namespaces are denied', async () => {
		api.harnessAutoupdate.mockResolvedValue(
			info([
				row('denied', { state: 'userns_denied', detail: 'apparmor' }),
				row('unknown', { state: 'unknown', detail: 'probe timed out' }),
				row('fine', { state: 'ok' })
			])
		);
		comp = mount(HarnessUpdateGroup, { target: document.body });
		await settle();
		expect(document.querySelectorAll('li.machine')).toHaveLength(3);
		expect(document.body.textContent).toContain('probe timed out');
		const links = docsLinks();
		expect(links).toHaveLength(1);
		expect(machineOf(links[0])).toBe('denied');
	});

	it('shows no link at all when nothing is denied', async () => {
		api.harnessAutoupdate.mockResolvedValue(info([row('unknown', { state: 'unknown', detail: 'x' })]));
		comp = mount(HarnessUpdateGroup, { target: document.body });
		await settle();
		expect(document.querySelectorAll('li.machine')).toHaveLength(1);
		expect(docsLinks()).toHaveLength(0);
	});
});

describe('HarnessUpdateGroup restart when idle', () => {
	const skewed: MachineHarnessInfo = {
		...row('m2pro', { state: 'ok' }),
		report: {
			versions: { 'claude-code': { cli: '2.1.286', daemon: '2.1.285' } },
			outcomes: [],
			managed_by_image: false,
			binaries: {}
		}
	};
	const restart = async () => {
		api.harnessAutoupdate.mockResolvedValue(info([skewed]));
		comp = mount(HarnessUpdateGroup, { target: document.body });
		await settle();
		const button = [...document.querySelectorAll('button')].find((b) =>
			b.textContent?.toLowerCase().includes('idle')
		);
		expect(button).toBeDefined();
		button!.click();
		await settle();
	};

	it('toasts the request once the server accepts it', async () => {
		api.restartHarness.mockResolvedValue(undefined);
		await restart();
		expect(api.restartHarness).toHaveBeenCalledWith('m2pro', 'claude-code');
		expect(toasts.ok).toHaveBeenCalledOnce();
		expect(toasts.error).not.toHaveBeenCalled();
	});

	it('shows the server refusal for a daemon too old to drain, not a success', async () => {
		const refusal = 'the cctui-daemon on m2pro is older than 0.25.7 and would ignore a drain restart; update it first';
		api.restartHarness.mockRejectedValue(new ApiError(409, refusal));
		await restart();
		expect(toasts.error).toHaveBeenCalledWith(refusal);
		expect(toasts.ok).not.toHaveBeenCalled();
	});
});
