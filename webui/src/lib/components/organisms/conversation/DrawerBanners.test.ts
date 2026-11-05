// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { ToolGuardState } from '@bindings/ToolGuardState';

const guard = { data: undefined as ToolGuardState | undefined };
const set = vi.fn();
vi.mock('$lib/queries', () => ({ useAccounts: () => ({ data: [] }) }));
vi.mock('$lib/queries/toolGuard', () => ({
	useToolGuard: () => guard,
	useToolGuardActions: () => ({ set })
}));
const toasts = vi.hoisted(() => ({ ok: vi.fn(), error: vi.fn() }));
vi.mock('$lib/toast.svelte', () => ({ toasts }));

import DrawerBanners from './DrawerBanners.svelte';

const state = (over: Partial<ToolGuardState> = {}): ToolGuardState => ({
	bypass: false,
	by: null,
	at: null,
	policy_active: true,
	can_toggle: true,
	...over
});

let comp: ReturnType<typeof mount> | null = null;
let stream: { softLimit: null; toolBlock: unknown; dismissToolBlock: ReturnType<typeof vi.fn> };

beforeEach(() => {
	set.mockReset();
	toasts.ok.mockReset();
	toasts.error.mockReset();
	stream = {
		softLimit: null,
		toolBlock: { tool_name: 'Bash', rule: 'term' },
		dismissToolBlock: vi.fn()
	};
});

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

function render(): HTMLElement {
	const host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(DrawerBanners, {
		target: host,
		props: { sessionId: 's-1', needsInput: false, stream } as never
	});
	flushSync();
	return host;
}

const settle = async () => {
	await new Promise((r) => setTimeout(r, 0));
	flushSync();
};

const button = (host: HTMLElement, id: string) =>
	host.querySelector<HTMLButtonElement>(`[data-testid="${id}"]`);

describe('DrawerBanners tool-block bypass', () => {
	it('asks before bypassing, then bypasses and dismisses the notice', async () => {
		guard.data = state();
		set.mockResolvedValue(state({ bypass: true }));
		const host = render();

		button(host, 'tool-guard-bypass')?.click();
		flushSync();
		expect(set).not.toHaveBeenCalled();
		expect(host.textContent).toContain('Lift the words filter for this session?');

		button(host, 'tool-guard-bypass-confirm')?.click();
		await settle();
		expect(set).toHaveBeenCalledWith('s-1', true);
		expect(stream.dismissToolBlock).toHaveBeenCalledOnce();
		expect(toasts.ok).toHaveBeenCalledOnce();
	});

	it('cancelling the confirm leaves the guard on', () => {
		guard.data = state();
		const host = render();
		button(host, 'tool-guard-bypass')?.click();
		flushSync();
		const cancel = [...host.querySelectorAll('button')].find(
			(b) => b.textContent?.trim() === 'Cancel'
		);
		cancel?.click();
		flushSync();
		expect(set).not.toHaveBeenCalled();
		expect(button(host, 'tool-guard-bypass')).not.toBeNull();
		expect(host.textContent).toContain('cctui blocked a Bash tool call');
	});

	it('does not offer the bypass to a viewer who does not own the account', () => {
		guard.data = state({ can_toggle: false });
		const host = render();
		expect(button(host, 'tool-guard-bypass')).toBeNull();
		expect(host.textContent).toContain('Dismiss');
	});

	it('keeps the notice when the bypass is refused', async () => {
		guard.data = state();
		set.mockRejectedValue(new Error('the account’s tool policy belongs to its owner'));
		const host = render();
		button(host, 'tool-guard-bypass')?.click();
		flushSync();
		button(host, 'tool-guard-bypass-confirm')?.click();
		await settle();
		expect(stream.dismissToolBlock).not.toHaveBeenCalled();
		expect(toasts.error).toHaveBeenCalledOnce();
	});
});

describe('DrawerBanners needs-input', () => {
	it('draws no banner over the transcript for a session waiting on the user', () => {
		stream.toolBlock = null;
		const host = document.createElement('div');
		document.body.appendChild(host);
		comp = mount(DrawerBanners, {
			target: host,
			props: { sessionId: 's-1', needsInput: true, stream } as never
		});
		flushSync();
		expect(host.querySelector('.attn-banner')).toBeNull();
		expect(host.textContent).not.toContain('Waiting for your input');
	});
});
