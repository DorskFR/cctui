// @vitest-environment happy-dom
import { flushSync, mount, unmount, type Component } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { settings } from '#lib/settings.svelte.ts';
import LayoutSection from './LayoutSection.svelte';
import SectionHost from './section.host.test.svelte';

let component: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (component) unmount(component);
	component = null;
	settings.setSpawnDock({ foldOnChat: true });
	document.body.innerHTML = '';
});

describe('Layout settings launch panel fold', () => {
	it('turns off folding the launch panel during a conversation', () => {
		component = mount(SectionHost, {
			target: document.body,
			props: { section: LayoutSection as Component<Record<string, never>> }
		});
		flushSync();
		const toggle = document.querySelector<HTMLButtonElement>(
			'[role="switch"][aria-label="Fold the launch panel during a conversation"]'
		);
		expect(toggle).not.toBeNull();
		expect(toggle?.getAttribute('aria-checked')).toBe('true');
		toggle?.click();
		flushSync();
		expect(settings.spawnDock.foldOnChat).toBe(false);
	});
});
