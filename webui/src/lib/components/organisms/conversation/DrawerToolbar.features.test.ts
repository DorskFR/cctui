// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { Feature } from '@bindings/Feature';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import DrawerToolbar from './DrawerToolbar.svelte';
import { allFilter } from './filters';
import type { ViewOpts } from './types';

const sessionContext = vi.hoisted(() => vi.fn());

vi.mock('$lib/queries', () => ({
	useSessionContext: (id: () => string) => {
		sessionContext(id());
		return { data: { rules: [], memories: [] }, isError: false };
	},
	useContextItems: () => ({ data: [] }),
	useAttachContext: () => async () => ({}),
	newerVersion: () => false
}));

const SWITCHABLE: Feature[] = ['auto_approve', 'terminal', 'plugins', 'context'];

function off(...names: Feature[]) {
	setCapabilitiesForTest({
		features: SWITCHABLE.filter((f) => !names.includes(f)),
		switchable: SWITCHABLE
	});
}

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	setCapabilitiesForTest(null);
	sessionContext.mockClear();
});

async function render() {
	comp = mount(DrawerToolbar, {
		target: document.body,
		props: {
			view: { msgFilter: allFilter(true), prettyJson: true, prettyDiff: true, prettyTables: true, paneWidth: null } as ViewOpts,
			autoApprove: false,
			ontoggleAuto: vi.fn(),
			ontoggleTerminal: vi.fn(),
			plugins: [{ id: 'p1', label: 'Board', icon: 'grid', open: false, onselect: vi.fn() }],
			sessionId: 's1',
			contextAttachable: true
		}
	});
	await new Promise((r) => setTimeout(r, 0));
	return document.querySelector('.toolbar') as HTMLElement;
}

const autoApprove = (bar: HTMLElement) => bar.querySelector('[aria-label^="Auto-approve"]');
const terminal = (bar: HTMLElement) => bar.querySelector('[data-journey="terminal"]');
const plugin = (bar: HTMLElement) => bar.querySelector('[data-plugin="p1"]');
const context = (bar: HTMLElement) => bar.querySelector('[data-journey="session-context"]');

describe('conversation toolbar feature gates', () => {
	it('shows every control with every feature on', async () => {
		off();
		const bar = await render();
		expect(autoApprove(bar)).toBeTruthy();
		expect(terminal(bar)).toBeTruthy();
		expect(plugin(bar)).toBeTruthy();
		expect(context(bar)).toBeTruthy();
		expect(sessionContext).toHaveBeenCalledWith('s1');
	});

	it('drops the auto-approve toggle without auto_approve', async () => {
		off('auto_approve');
		expect(autoApprove(await render())).toBeNull();
	});

	it('drops the terminal toggle without terminal', async () => {
		off('terminal');
		expect(terminal(await render())).toBeNull();
	});

	it('drops plugin toggles without plugins', async () => {
		off('plugins');
		expect(plugin(await render())).toBeNull();
	});

	it('mounts no context chip and asks nothing without context', async () => {
		off('context');
		expect(context(await render())).toBeNull();
		expect(sessionContext).not.toHaveBeenCalled();
	});
});
