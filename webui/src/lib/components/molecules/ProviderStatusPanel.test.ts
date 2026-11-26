// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { ProviderStatus } from '#lib/queries/index.ts';
import ProviderStatusPanel from './ProviderStatusPanel.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

const statuses: ProviderStatus[] = [
	{
		family: 'anthropic',
		indicator: 'minor',
		description: 'Partially Degraded Service',
		url: 'https://status.claude.com',
		components: [{ name: 'claude.ai', status: 'degraded_performance' }],
		incidents: [{ name: 'Add-in missing for some users', impact: 'minor' }]
	},
	{
		family: 'openai',
		indicator: 'critical',
		description: 'Partial System Degradation',
		url: 'https://status.openai.com',
		components: [],
		incidents: [{ name: 'Elevated errors with reviews', impact: 'major' }]
	}
];

function render() {
	comp = mount(ProviderStatusPanel, { target: document.body, props: { statuses } });
	flushSync();
	return document.querySelector('[data-testid="provider-status-panel"]') as HTMLElement;
}

describe('ProviderStatusPanel', () => {
	it('lays every provider out from the left edge', () => {
		const panel = render();
		for (const el of [panel, ...panel.querySelectorAll<HTMLElement>('*')]) {
			expect(el.className).not.toMatch(/center/);
			expect(el.style.textAlign).not.toBe('center');
		}
		expect(panel.querySelector('[data-tsu="Link"], a')?.className ?? '').not.toMatch(
			/align-center/
		);
	});

	it('gives each provider its name, status, affected parts and status page', () => {
		const panel = render();
		const rows = [...panel.querySelectorAll<HTMLElement>('section[data-family]')];
		expect(rows.map((r) => r.dataset.family)).toEqual(['anthropic', 'openai']);

		const [claude, codex] = rows;
		expect(claude.querySelector('.head')?.textContent).toContain('Claude');
		expect(claude.querySelector('.head')?.textContent).toContain('degraded');
		expect(claude.textContent).toContain('Partially Degraded Service');
		expect(claude.querySelector('[data-testid="provider-status-affected"]')?.textContent).toContain(
			'Affected: claude.ai'
		);
		expect(claude.textContent).toContain('Add-in missing for some users');
		const link = claude.querySelector('a') as HTMLAnchorElement;
		expect(link.href).toBe('https://status.claude.com/');
		expect(link.target).toBe('_blank');
		expect(link.textContent).toContain('Status page');

		expect(codex.querySelector('.head')?.textContent).toContain('Codex');
		expect(codex.querySelector('.head')?.textContent).toContain('major outage');
		expect(codex.querySelector('[data-testid="provider-status-affected"]')).toBeNull();
		expect((codex.querySelector('a') as HTMLAnchorElement).href).toBe('https://status.openai.com/');
	});

	it('separates providers with a rule', () => {
		const panel = render();
		expect(panel.querySelectorAll('[role="separator"], hr, [data-tsu="Divider"]').length).toBe(1);
	});
});
