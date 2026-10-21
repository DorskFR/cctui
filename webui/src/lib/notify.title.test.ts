import { describe, expect, it } from 'vitest';
import { notify } from './notify.svelte';

describe('tab title', () => {
	it('leads with the open session and falls back to the brand', () => {
		expect(notify.baseTitle()).toBe('cctui');
		notify.setSessionTitle('SUPER ORCHESTRATOR (super orchestrator resume)');
		expect(notify.baseTitle()).toBe('SUPER ORCHESTRATOR (super orchestrator resume) · cctui');
		notify.setInstanceName('home');
		expect(notify.baseTitle()).toBe(
			'SUPER ORCHESTRATOR (super orchestrator resume) · cctui (home)'
		);
		notify.setSessionTitle('  ');
		expect(notify.baseTitle()).toBe('cctui (home)');
		notify.setInstanceName(null);
		notify.setSessionTitle(null);
		expect(notify.baseTitle()).toBe('cctui');
	});
});
