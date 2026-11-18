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
	settings.setSessionList({ density: 'normal' });
	document.body.innerHTML = '';
});

describe('Layout settings row density', () => {
	it('switches the session rows to compact', () => {
		component = mount(SectionHost, {
			target: document.body,
			props: { section: LayoutSection as Component<Record<string, never>> }
		});
		flushSync();
		const compact = [...document.querySelectorAll<HTMLElement>('button, [role="radio"], label')].find(
			(el) => el.textContent?.trim() === 'Compact'
		);
		expect(compact).toBeDefined();
		compact?.click();
		flushSync();
		expect(settings.state.sessionList.density).toBe('compact');
	});
});
