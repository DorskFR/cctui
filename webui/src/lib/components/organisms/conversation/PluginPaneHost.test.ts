import { describe, expect, it } from 'vitest';
import hostSource from './PluginPaneHost.svelte?raw';

describe('plugin pane resize grip', () => {
	it("lives on the pane's free left edge and grows the pane when dragged left", () => {
		const start = hostSource.indexOf('use:resizeHandle');
		const grip = hostSource.slice(start, hostSource.indexOf('}}', start));
		expect(grip).toContain("side: 'right'");
		expect(grip).toContain('min: DOCK_MIN_PX');
		expect(grip).toContain('max: maxPx');
		expect(hostSource).toMatch(/\.grip \{[^}]*left: -5px/);
		expect(hostSource).not.toMatch(/\.grip \{[^}]*right: -5px/);
	});

	it('is keyboard reachable and persists the width per plugin', () => {
		expect(hostSource).toContain('role="separator"');
		expect(hostSource).toContain('tabindex="0"');
		expect(hostSource).toContain('cctui_plugin_pane_width:');
	});
});
