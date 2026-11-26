import { describe, expect, it } from 'vitest';
import group from './SettingGroup.svelte?raw';
import section from './SettingSection.svelte?raw';

describe('setting surfaces', () => {
	it('frames a group with shade, not an outline', () => {
		const style = group.slice(group.indexOf('<style>'));
		expect(style).toContain('background: var(--bg-elevated)');
		expect(style).not.toMatch(/border(-bottom)?:\s*1px/);
	});

	it('leaves scope badges to the nav, not section headings', () => {
		expect(section).not.toContain('settings_scope_admin');
	});
});
