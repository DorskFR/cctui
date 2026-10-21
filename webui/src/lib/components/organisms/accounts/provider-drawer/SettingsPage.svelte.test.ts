// @vitest-environment happy-dom
import { describe, it, expect, afterEach } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import SettingsPage from './SettingsPage.svelte';
import type { Knob, KnobGroup } from './knobs.logic';

let host: HTMLElement | null = null;
let comp: ReturnType<typeof mount> | null = null;

afterEach(() => {
	if (comp) unmount(comp);
	host?.remove();
	comp = null;
	host = null;
});

const knob = (name: string): Knob => ({
	id: `e:${name}`,
	label: name,
	sub: name,
	care: false,
	control: 'enum',
	loc: 'env',
	name,
	values: ['none', 'worktree'],
	note: ''
});

function render(groups: KnobGroup[]) {
	host = document.createElement('div');
	document.body.appendChild(host);
	comp = mount(SettingsPage, { target: host, props: { groups, settings: {} } });
	flushSync();
	return host;
}

describe('SettingsPage isolation scope', () => {
	it('says where the bg isolation knob applies', () => {
		const el = render([{ title: 'sessions', page: 'advanced', knobs: [knob('CLAUDE_BG_ISOLATION')] }]);
		const scope = el.querySelector('.scope')?.textContent ?? '';
		expect(scope).toContain('cctui daemon');
		expect(scope).toContain('worker profile');
		expect(scope).toContain('default none');
	});

	it('adds no scope line to other knobs', () => {
		const el = render([{ title: 'sessions', page: 'advanced', knobs: [knob('CLAUDE_CODE_OTHER')] }]);
		expect(el.querySelector('.scope')).toBeNull();
	});
});
