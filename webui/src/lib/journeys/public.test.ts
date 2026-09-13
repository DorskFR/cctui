import { describe, expect, it } from 'vitest';
import { QueryClient } from '@tanstack/svelte-query';
import { compile, type Journey } from '@dorsk/journey';
import accountsPools from '../../../journeys/accounts-pools.journey';
import enrollMachine from '../../../journeys/enroll-machine.journey';
import followSession from '../../../journeys/follow-session.journey';
import searchSessions from '../../../journeys/search-sessions.journey';
import sessionsList from '../../../journeys/sessions-list.journey';
import settingsTour from '../../../journeys/settings-tour.journey';
import spawnSession from '../../../journeys/spawn-session.journey';
import usageOverview from '../../../journeys/usage-overview.journey';
import { GATES, DONE_PROBES, PUBLIC_JOURNEYS, requiredParams } from '../journey';
import { createProbes } from './probes';

const SPECS: Journey[] = [
	enrollMachine,
	accountsPools,
	spawnSession,
	followSession,
	usageOverview,
	sessionsList,
	settingsTour,
	searchSessions
];
const byId = (id: string) => SPECS.find((j) => j.id === id)!;
const pub = (id: string) => compile(byId(id), { public: true });
const book = (id: string) => compile(byId(id));
const captures = (ir: ReturnType<typeof compile>) =>
	ir.steps.flatMap((s) => (s.capture ? [s.capture.name] : []));
// Must mirror the keys guideParams() fills.
const HOST_PARAMS = ['fixture.me', 'account', 'pool', 'fixture.session'];
const FILL_PARAMS = ['var.label', 'var.prompt'];
const PROBES = Object.keys(createProbes(new QueryClient()));

describe('public journey set', () => {
	it('offers the onboarding guides first, in the spec order, and never search-sessions', () => {
		expect(PUBLIC_JOURNEYS).toEqual([
			'enroll-machine',
			'accounts-pools',
			'spawn-session',
			'follow-session',
			'usage-overview',
			'sessions-list',
			'settings-tour'
		]);
		expect(PUBLIC_JOURNEYS).not.toContain('search-sessions');
		expect(pub('search-sessions').steps).toEqual([]);
	});

	it('strips every qaOnly step and qa.* probe from the public IR', () => {
		for (const j of SPECS) {
			const ir = compile(j, { public: true });
			for (const step of ir.steps) {
				expect(step.qaOnly, `${j.id}/${step.id}`).toBeUndefined();
				for (const e of step.expect ?? []) {
					if ('probe' in e) expect(e.probe, `${j.id}/${step.id}`).not.toMatch(/^qa\./);
				}
			}
		}
	});

	it('only references probes the host registers', () => {
		for (const id of PUBLIC_JOURNEYS) {
			for (const step of pub(id).steps) {
				for (const e of step.expect ?? []) {
					if ('probe' in e) expect(PROBES, `${id}/${step.id}`).toContain(e.probe);
				}
			}
		}
		for (const g of Object.values(GATES)) expect(PROBES).toContain(g.probe);
		for (const p of Object.values(DONE_PROBES)) expect(PROBES).toContain(p);
		for (const g of Object.values(GATES)) expect(PUBLIC_JOURNEYS).toContain(g.prerequisite);
	});

	it('addresses real-instance names only through params the host supplies', () => {
		for (const id of PUBLIC_JOURNEYS) {
			const ir = pub(id);
			for (const p of requiredParams(ir)) expect(HOST_PARAMS, `${id} {${p}}`).toContain(p);
			for (const step of ir.steps) {
				const target = JSON.stringify(step.target ?? '');
				expect(target, `${id}/${step.id}`).not.toMatch(/admin|acme-research|production|a0000000/);
				expect(target, `${id}/${step.id}`).not.toMatch(/Machines \d/);
			}
		}
	});

	it('never asks the user to type a prescribed string', () => {
		for (const id of PUBLIC_JOURNEYS) {
			for (const step of pub(id).steps) {
				if (step.do.kind !== 'fill') continue;
				expect(typeof step.do.value, `${id}/${step.id}`).toBe('object');
				expect(FILL_PARAMS).toContain((step.do.value as { $param: string }).$param);
				for (const e of step.expect ?? []) expect('value' in e, `${id}/${step.id}`).toBe(false);
			}
		}
	});

	it('gates the enroll probe wait on a long timeout', () => {
		const enroll = pub('enroll-machine').steps.find((s) => s.id === 'enroll')!;
		expect(enroll.timeout).toBe(600000);
		expect(enroll.expect).toContainEqual({ probe: 'machines.online' });
	});

	it('keeps the follow-session public tour free of mutations', () => {
		for (const step of pub('follow-session').steps) {
			expect(step.do.kind, step.id).not.toBe('fill');
		}
		expect(pub('follow-session').steps.map((s) => s.id)).toEqual([
			'open',
			'header',
			'meta',
			'actions',
			'kinds',
			'line-actions',
			'mobile-filters',
			'filters',
			'filter-menu',
			'reply'
		]);
	});

	it('teaches the drawer without depending on a session that may end mid-tour', () => {
		const ids = pub('follow-session').steps.map((s) => s.id);
		expect(requiredParams(pub('follow-session'))).toEqual([]);
		expect(ids.indexOf('mobile-filters')).toBeLessThan(ids.indexOf('filters'));
		const open = pub('follow-session').steps[0];
		expect(open.expect).not.toContainEqual({ visible: 'conversation/line[assistant]' });
	});

	it('keeps sessions-list on its own surface and off the theme picker', () => {
		expect(pub('sessions-list').steps.map((s) => s.id)).toEqual([
			'list',
			'search',
			'sections',
			'starred',
			'options',
			'grouping',
			'view',
			'group-sort',
			'group-actions'
		]);
		for (const step of pub('sessions-list').steps) {
			expect(JSON.stringify(step.target ?? ''), step.id).not.toMatch(/data-tsu|theme/i);
		}
	});

	it('indexes the per-section group anchors, which render once per group', () => {
		for (const id of ['group-sort', 'group-actions']) {
			const step = pub('sessions-list').steps.find((s) => s.id === id)!;
			expect(step.target, id).toMatchObject({ nth: 0 });
			expect(step.optional, id).toBe(true);
			for (const e of step.expect ?? []) {
				expect(e, id).toMatchObject({ visible: { nth: 0 } });
			}
		}
	});

	it('adds an account only after the book has captured the board', () => {
		expect(pub('accounts-pools').steps.map((s) => s.id)).toEqual(['board', 'add']);
		const ids = book('accounts-pools').steps.map((s) => s.id);
		expect(ids).toEqual(['board', 'pool', 'handle', 'menu', 'add']);
		expect(ids.indexOf('add')).toBeGreaterThan(ids.lastIndexOf('menu'));
	});

	it('walks spawn-session through the machine and folder before the fills', () => {
		const ids = pub('spawn-session').steps.map((s) => s.id);
		expect(ids).toEqual(['open', 'where', 'name', 'prompt', 'save', 'sections', 'show-drafts']);
		const open = pub('spawn-session').steps[0];
		expect(open.expect).not.toContainEqual({ enabled: 'draft' });
	});
});

describe('book fidelity', () => {
	it('keeps every screenshot capture the docs are built from', () => {
		expect(captures(book('enroll-machine'))).toEqual(['access', 'enroll', 'user', 'machines']);
		expect(captures(book('accounts-pools'))).toEqual(['board', 'pool', 'handle', 'menu']);
		expect(captures(book('spawn-session'))).toEqual(['dialog', 'filled', 'saved', 'draft']);
		expect(captures(book('follow-session'))).toEqual([
			'drawer',
			'header',
			'timeline',
			'line',
			'tools',
			'reply'
		]);
		expect(captures(book('sessions-list'))).toEqual([
			'list',
			'search',
			'sections',
			'options',
			'group'
		]);
		expect(captures(book('search-sessions'))).toEqual(['before', 'text', 'facet']);
		expect(captures(book('usage-overview'))).toEqual(['tiles', 'windows', 'analytics']);
		expect(captures(book('settings-tour'))).toEqual(['appearance', 'sessions', 'execution', 'privacy']);
	});

	it('keeps the fixture assertions in the book compile', () => {
		const list = book('sessions-list').steps.find((s) => s.id === 'list-fixture')!;
		expect(list.qaOnly).toBe(true);
		expect(list.expect).toContainEqual({ count: ['session', { min: 4 }] });
		const machines = book('enroll-machine').steps.find((s) => s.id === 'machines')!;
		expect(machines.target).toEqual({ role: 'tab', name: 'Machines 2' });
		expect(book('search-sessions').steps).toHaveLength(3);
	});
});
