// @vitest-environment happy-dom
import { mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Feature } from '@bindings/Feature';
import { setCapabilitiesForTest } from '$lib/features.svelte';
import FirstRunChecklist from './FirstRunChecklist.svelte';

const asked = vi.hoisted(() => ({ probes: [] as string[], replayed: [] as string[] }));

vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: () => ({}) }));

vi.mock('$lib/journeys/probes', () => ({
	createProbes: () =>
		new Proxy(
			{},
			{
				get: (_t, name: string) => async () => {
					asked.probes.push(name);
					return false;
				}
			}
		)
}));

vi.mock('$lib/guides', () => ({
	buildCurriculum: () => ({ sections: [] }),
	guideEntries: () => [],
	guideOptions: () => ({}),
	replayGuide: async (id: string) => {
		asked.replayed.push(id);
		return { ok: true };
	}
}));

vi.mock('$lib/journey', () => ({
	GUIDES_ROUTE: '/settings/guides',
	startFailureMessage: () => undefined
}));

vi.mock('$lib/settings.svelte', () => ({
	settings: { onboarding: { checklistDismissed: false }, setOnboarding: () => {} }
}));

const SWITCHABLE: Feature[] = ['machine_spawn', 'account_sharing'];
let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
	asked.probes = [];
	asked.replayed = [];
});
afterEach(async () => {
	if (component) await unmount(component);
	component = undefined;
	document.body.replaceChildren();
	setCapabilitiesForTest(null);
});

async function render(machineSpawn: boolean, onnew?: () => void) {
	setCapabilitiesForTest({
		features: machineSpawn ? SWITCHABLE : [],
		switchable: SWITCHABLE
	});
	component = mount(FirstRunChecklist, { target: document.body, props: { onnew } });
	await new Promise((r) => setTimeout(r, 30));
}

const steps = () =>
	[...document.querySelectorAll('[data-journey="first-run-step"]')].map((row) => row.textContent?.replace(/\s+/g, ' ').trim());
const action = (key: string) =>
	document.querySelector<HTMLElement>(`[data-journey="first-run-start"][data-journey-key="${key}"]`);

describe('FirstRunChecklist without machine_spawn', () => {
	it('asks for a dispatcher, then a dispatched session', async () => {
		await render(false);
		expect(steps()).toEqual([
			expect.stringContaining('Connect a provider account'),
			expect.stringContaining('Enroll a dispatcher'),
			expect.stringContaining('Dispatch a session')
		]);
		expect(document.body.textContent).not.toContain('Enroll a machine');
	});

	it('probes dispatchers, never machines', async () => {
		await render(false);
		expect(asked.probes).toContain('dispatchers');
		expect(asked.probes.some((p) => p.startsWith('machines'))).toBe(false);
	});

	it('links the dispatcher step to Settings, Dispatchers', async () => {
		await render(false);
		expect(action('dispatcher')?.getAttribute('href')).toBe('/settings/dispatchers');
	});

	it('opens the launch form from the last step where the caller can', async () => {
		const onnew = vi.fn();
		await render(false, onnew);
		action('dispatch')?.click();
		expect(onnew).toHaveBeenCalledOnce();
		expect(asked.replayed).toEqual([]);
	});

	it('links the last step to Sessions elsewhere', async () => {
		await render(false);
		expect(action('dispatch')?.getAttribute('href')).toBe('/sessions');
	});
});

describe('FirstRunChecklist with machine_spawn on', () => {
	it('keeps the machine steps and their guides', async () => {
		await render(true);
		expect(steps()).toEqual([
			expect.stringContaining('Connect a provider account'),
			expect.stringContaining('Enroll a machine'),
			expect.stringContaining('Start a session')
		]);
		expect(action('enroll-machine')).toBeTruthy();
		expect(action('spawn-session')).toBeTruthy();
		expect(asked.probes).toContain('machines.enrolled');
		expect(asked.probes).not.toContain('dispatchers');
	});
});
