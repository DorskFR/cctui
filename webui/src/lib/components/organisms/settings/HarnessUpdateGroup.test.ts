// @vitest-environment happy-dom
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { HarnessAutoupdateInfo } from '@bindings/HarnessAutoupdateInfo';
import type { MachineHarnessInfo } from '@bindings/MachineHarnessInfo';
import type { CodexSandbox } from '@bindings/CodexSandbox';

const api = vi.hoisted(() => ({
	harnessAutoupdate: vi.fn(),
	setHarnessAutoupdate: vi.fn(),
	setMachineHarnessAutoupdate: vi.fn()
}));
vi.mock('$lib/queries', () => ({ endpoints: api }));
vi.mock('$lib/toast.svelte', () => ({ toasts: { ok: vi.fn(), error: vi.fn() } }));
vi.mock('$lib/harnesses.svelte', () => ({ harnessTable: () => [] }));

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
