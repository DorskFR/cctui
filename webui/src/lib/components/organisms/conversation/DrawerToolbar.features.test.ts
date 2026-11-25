// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import type { Feature } from '@bindings/Feature';
import { setCapabilitiesForTest } from '#lib/features.svelte.ts';
import DrawerToolbar from './DrawerToolbar.svelte';
import toolbarSource from './DrawerToolbar.svelte?raw';
import { allFilter } from './filters';
import type { ViewOpts } from './types';

const sessionContext = vi.hoisted(() => vi.fn());

vi.mock('#lib/queries/index.ts', () => ({
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

const autoApprove = (bar: HTMLElement) => bar.querySelector('[data-journey="auto-approve"]');
const panels = (bar: HTMLElement) => bar.querySelector('[data-journey="panels"]');
const context = (bar: HTMLElement) => bar.querySelector('[data-journey="session-context"]');

describe('conversation toolbar feature gates', () => {
	it('shows every control with every feature on', async () => {
		off();
		const bar = await render();
		expect(autoApprove(bar)).toBeTruthy();
		expect(panels(bar)).toBeTruthy();
		expect(context(bar)).toBeTruthy();
		expect(sessionContext).toHaveBeenCalledWith('s1');
	});

	it('drops the auto-approve toggle without auto_approve', async () => {
		off('auto_approve');
		expect(autoApprove(await render())).toBeNull();
	});

	it('gates the terminal and plugin rows of the Panels menu on their features', () => {
		expect(toolbarSource).toContain("ontoggleTerminal && hasFeature('terminal') ? { open: terminalOpen, toggle: ontoggleTerminal } : null");
		expect(toolbarSource).toContain("hasFeature('plugins') ? plugins : []");
	});

	it('drops the Panels menu once both terminal and plugins are off', async () => {
		off('terminal', 'plugins');
		expect(panels(await render())).toBeNull();
	});

	it('mounts no context chip and asks nothing without context', async () => {
		off('context');
		expect(context(await render())).toBeNull();
		expect(sessionContext).not.toHaveBeenCalled();
	});
});
