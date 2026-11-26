import { describe, expect, it } from 'vitest';
import group from './SettingGroup.svelte?raw';
import section from './SettingSection.svelte?raw';

describe('setting surfaces', () => {
	it('frames a group with shade, not an outline', () => {
		const style = group.slice(group.indexOf('<style>'));
		expect(style).toContain('background: var(--bg-elevated)');
		expect(style).not.toMatch(/border(-bottom)?:\s*1px/);
	});

	it('can mark a section heading as admin-only', () => {
		expect(section).toContain('settings_scope_admin');
	});
});
