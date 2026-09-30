// @vitest-environment happy-dom
import { readdirSync, readFileSync } from 'node:fs';
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
import welcome from '../../../journeys/welcome.journey';
import {
	DONE_PROBES,
	entryRoute,
	PUBLIC_JOURNEYS,
	READINESS,
	readinessHint,
	requiredParams
} from '../journey';
import { createProbes } from './probes';

const SPECS: Journey[] = [
	welcome,
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
const FILL_PARAMS = ['var.label', 'var.prompt', 'var.query', 'var.facet', 'var.blank'];
const PROBES = Object.keys(createProbes(new QueryClient()));

/** The expectation keys whose value addresses the DOM; `url`, `probe` and
 *  `event` carry strings that are not paths. */
const TARGET_KEYS = ['visible', 'hidden', 'enabled', 'disabled', 'text', 'value', 'checked', 'count'];

/** `a/b[key]` addresses `[data-journey="a"] [data-journey="b"][data-journey-key="key"]`,
 *  so every segment name of every path a step walks has to exist in the markup. */
function anchorNames(step: { target?: unknown; expect?: unknown[] }): string[] {
	const out = new Set<string>();
	const walk = (value: unknown) => {
		const path =
			typeof value === 'string' ? value : (value as { within?: string } | null)?.within;
		if (typeof path !== 'string') return;
		for (const segment of path.split('/')) {
			const name = segment.split('[')[0]!.trim();
			if (name && !name.includes('{')) out.add(name);
		}
	};
	walk(step.target);
	for (const e of step.expect ?? []) {
		for (const [key, v] of Object.entries(e as Record<string, unknown>)) {
			if (!TARGET_KEYS.includes(key)) continue;
			walk(Array.isArray(v) ? v[0] : v);
		}
	}
	return [...out];
}

describe('public journey set', () => {
	it('registers every curriculum guide, in curriculum order', () => {
		expect(PUBLIC_JOURNEYS).toEqual([
			'welcome',
			'sessions-list',
			'accounts-pools',
			'enroll-machine',
			'spawn-session',
			'follow-session',
			'search-sessions',
			'usage-overview',
			'settings-tour'
		]);
		expect(pub('search-sessions').steps.map((s) => s.id)).toEqual([
			'box',
			'whole',
			'free-text',
			'facet',
			'combine',
			'clear'
		]);
	});

	it('has a public tour behind every guide it registers', () => {
		for (const id of PUBLIC_JOURNEYS) {
			expect(byId(id), id).toBeDefined();
			expect(pub(id).steps.length, id).toBeGreaterThan(0);
		}
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
		for (const p of Object.values(READINESS)) expect(PROBES).toContain(p);
		for (const p of Object.values(DONE_PROBES)) expect(PROBES).toContain(p);
	});

	it('states what a guide is waiting for in words, for every guide that waits', () => {
		for (const id of Object.keys(READINESS)) {
			expect(PUBLIC_JOURNEYS, id).toContain(id);
			const hint = readinessHint(id);
			expect(hint, id).toBeTruthy();
			expect(hint, id).not.toContain(id);
		}
		for (const id of PUBLIC_JOURNEYS) {
			if (!(id in READINESS)) expect(readinessHint(id), id).toBeUndefined();
		}
	});

	it('addresses real-instance names only through params the host supplies', () => {
		for (const id of PUBLIC_JOURNEYS) {
			const ir = pub(id);
			for (const p of requiredParams(ir)) expect(HOST_PARAMS, `${id} {${p}}`).toContain(p);
			for (const step of ir.steps) {
				for (const [what, json] of [
					['target', JSON.stringify(step.target ?? '')],
					['expect', JSON.stringify(step.expect ?? [])]
				] as const) {
					const where = `${id}/${step.id} ${what}`;
					expect(json, where).not.toMatch(/admin|acme-research|production|a0000000/);
					expect(json, where).not.toMatch(/Machines \d/);
				}
			}
		}
	});

	it('anchors every public step to a data-journey name the app still renders', () => {
		const anchors = new Set<string>();
		for (const file of readdirSync('src', { recursive: true, encoding: 'utf8' })) {
			if (!/\.(svelte|ts)$/.test(file) || file.endsWith('.test.ts')) continue;
			const source = readFileSync(`src/${file}`, 'utf8');
			// A hook reaches the DOM either as a markup attribute or, when a kit
			// component renders the element, through its `attrs` object.
			for (const re of [/\bjourney="([^"]+)"/g, /'data-journey':\s*'([^']+)'/g]) {
				for (const m of source.matchAll(re)) anchors.add(m[1]);
			}
		}
		expect(anchors.size).toBeGreaterThan(0);
		for (const id of PUBLIC_JOURNEYS) {
			for (const step of pub(id).steps) {
				for (const name of anchorNames(step)) {
					expect(anchors, `${id}/${step.id} anchors "${name}"`).toContain(name);
				}
			}
		}
	});

	it('can open every anchored guide from the guides page', () => {
		for (const id of PUBLIC_JOURNEYS) {
			const ir = pub(id);
			if (!ir.steps.some((s) => s.target !== undefined)) continue;
			expect(entryRoute(ir), id).toBeTruthy();
		}
	});

	it('opens the welcome tour on the landing route it anchors', () => {
		const tour = pub('welcome');
		expect(tour.steps.every((s) => s.target !== undefined)).toBe(true);
		expect(entryRoute(tour)).toBe('/');
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

	it('fills a real field, not the wrapper its anchor sits on', () => {
		// A typing human's input event bubbles, so a wrapper works in guide mode and
		// fails in the book, which fills the resolved element itself. These are the
		// anchors that sit on a field rather than around one.
		const FIELDS = ['label', 'prompt', 'message'];
		for (const id of PUBLIC_JOURNEYS) {
			for (const step of pub(id).steps) {
				if (step.do.kind !== 'fill') continue;
				const where = `${id}/${step.id}`;
				if (typeof step.target === 'object') {
					expect(step.target, where).toMatchObject({ within: expect.any(String) });
					continue;
				}
				const leaf = String(step.target).split('/').at(-1)!.split('[')[0];
				expect(FIELDS, `${where} fills "${leaf}", which is not a known field anchor`).toContain(leaf);
			}
		}
	});

	it('never parks a guide on state the user has not created yet', () => {
		// A probe expectation in guide mode polls until `step.timeout`, and a guide
		// that waits for a machine to enrol is a guide that never ends.
		for (const id of PUBLIC_JOURNEYS) {
			for (const step of pub(id).steps) {
				expect(step.timeout ?? 0, `${id}/${step.id}`).toBeLessThanOrEqual(30000);
				for (const e of step.expect ?? []) {
					expect('probe' in e, `${id}/${step.id} waits on a probe`).toBe(false);
				}
			}
		}
	});

	it('marks no public step optional, which guide mode cannot honour', () => {
		// `optional` is only consulted when resolving a target times out, and the
		// human actor resolves without a timeout — so it skips nothing and the
		// step hangs instead.
		for (const id of PUBLIC_JOURNEYS) {
			for (const step of pub(id).steps) {
				expect(step.optional, `${id}/${step.id}`).toBeUndefined();
			}
		}
	});

	it('keeps every step of the spawn dialog interactive while it is open', () => {
		// The dialog is modal, so everything outside it — including the guide card's
		// own Next button — is not hit-testable. A passive step there is a dead end.
		const steps = pub('spawn-session').steps;
		const open = steps.findIndex((s) => s.id === 'open');
		const close = steps.findIndex((s) => s.id === 'save');
		expect(open).toBeGreaterThanOrEqual(0);
		expect(close).toBeGreaterThan(open);
		for (const step of steps.slice(open, close + 1)) {
			expect(step.do.kind, `spawn-session/${step.id}`).not.toBe('none');
		}
	});

	it('keeps the follow-session public tour free of mutations', () => {
		for (const step of pub('follow-session').steps) {
			expect(step.do.kind, step.id).not.toBe('fill');
		}
		expect(pub('follow-session').steps.map((s) => s.id)).toEqual([
			'open',
			'header',
			'meta',
			'details',
			'activity',
			'actions',
			'kinds',
			'line-actions',
			'filters',
			'filter-menu',
			'tools-only',
			'tools-restore',
			'reply'
		]);
	});

	it('teaches the drawer without depending on a session that may end mid-tour', () => {
		expect(requiredParams(pub('follow-session'))).toEqual([]);
		const open = pub('follow-session').steps[0];
		expect(open.expect).not.toContainEqual({ visible: 'conversation/line[assistant]' });
	});

	it('keeps sessions-list on its own surface and off the theme picker', () => {
		expect(pub('sessions-list').steps.map((s) => s.id)).toEqual([
			'list',
			'anatomy',
			'sections',
			'options',
			'view',
			'density-mobile',
			'search',
			'clear'
		]);
		for (const step of pub('sessions-list').steps) {
			expect(JSON.stringify(step.target ?? ''), step.id).not.toMatch(/data-tsu|theme/i);
		}
	});

	it('gives the density switch a twin for the width that does not have it', () => {
		const ids = pub('sessions-list').steps;
		expect(ids.find((s) => s.id === 'view')!.when).toEqual({ viewport: 'desktop' });
		expect(ids.find((s) => s.id === 'density-mobile')!.when).toEqual({ viewport: 'mobile' });
	});

	it('indexes every anchor whose component repeats, in both lane specs', () => {
		const indexed: Record<string, string[]> = {
			'follow-session': ['open', 'kinds', 'line-actions']
		};
		for (const [id, stepIds] of Object.entries(indexed)) {
			for (const stepId of stepIds) {
				const step = pub(id).steps.find((s) => s.id === stepId)!;
				expect(step.target, `${id}/${stepId}`).toMatchObject({ nth: 0 });
			}
		}
	});

	it('counts rather than indexes when an expectation means "all of them"', () => {
		const tools = book('follow-session').steps.find((s) => s.id === 'tools-only')!;
		for (const e of tools.expect ?? []) {
			expect(JSON.stringify(e), 'tools-only').not.toMatch(/nth/);
		}
		expect(tools.expect).toContainEqual({ hidden: 'conversation/line[assistant]' });
	});

	it('adds an account only after the book has captured the board', () => {
		const ids = pub('accounts-pools').steps.map((s) => s.id);
		expect(ids).toEqual(['board', 'anatomy', 'pools', 'add', 'close']);
		// The dialog must be shut again, or the run ends with it covering the board.
		expect(ids.indexOf('close')).toBeGreaterThan(ids.indexOf('add'));
		expect(ids.at(-1)).toBe('close');
	});

	it('walks spawn-session through the machine and folder before the fills', () => {
		const ids = pub('spawn-session').steps.map((s) => s.id);
		expect(ids).toEqual(['open', 'where', 'name', 'prompt', 'profiles', 'save', 'drafts']);
		const open = pub('spawn-session').steps[0];
		expect(open.expect).not.toContainEqual({ enabled: 'draft' });
		// The draft button only enables once machine+folder are set, so no step
		// may block on it.
		for (const step of pub('spawn-session').steps) {
			expect(step.expect ?? [], step.id).not.toContainEqual({ enabled: 'draft' });
		}
	});
});

describe('book fidelity', () => {
	it('keeps every screenshot capture the docs are built from', () => {
		expect(captures(book('welcome'))).toEqual([
			'overview',
			'attention',
			'nav',
			'sessions',
			'start',
			'accounts',
			'access',
			'guides'
		]);
		expect(captures(book('enroll-machine'))).toEqual([
			'access',
			'command',
			'copy',
			'online',
			'user',
			'tabs'
		]);
		expect(captures(book('accounts-pools'))).toEqual([
			'board',
			'anatomy',
			'pools',
			'add',
			'closed'
		]);
		expect(captures(book('spawn-session'))).toEqual([
			'dialog',
			'where',
			'filled',
			'profiles',
			'saved',
			'draft'
		]);
		expect(captures(book('follow-session'))).toEqual([
			'drawer',
			'header',
			'details',
			'activity',
			'timeline',
			'line',
			'tools',
			'restored',
			'reply'
		]);
		expect(captures(book('sessions-list'))).toEqual([
			'list',
			'anatomy',
			'sections',
			'options',
			'view',
			'search',
			'cleared'
		]);
		expect(captures(book('search-sessions'))).toEqual([
			'box',
			'before',
			'text',
			'facet',
			'combined',
			'cleared'
		]);
		expect(captures(book('usage-overview'))).toEqual(['tiles', 'periods', 'windows', 'analytics']);
		expect(captures(book('settings-tour'))).toEqual([
			'theme',
			'language',
			'sessions',
			'execution',
			'privacy',
			'guides'
		]);
	});

	it('shoots the guides screen, which the book had never photographed', () => {
		const guides = book('settings-tour').steps.find((s) => s.id === 'guides')!;
		expect(guides.route).toBe('/settings/guides');
		expect(guides.capture?.name).toBe('guides');
		expect(JSON.stringify(guides.target)).toContain('guide');
	});

	it('ships no qaOnly step, so the book and the guide teach the same thing', () => {
		for (const id of PUBLIC_JOURNEYS) {
			expect(book(id).steps.map((s) => s.id), id).toEqual(pub(id).steps.map((s) => s.id));
		}
	});

	it('leaves no step a user cannot advance past', () => {
		const stuck = PUBLIC_JOURNEYS.flatMap((id) =>
			pub(id)
				.steps.filter((s) => s.guide !== 'next' && s.do === undefined)
				.map((s) => `${id}/${s.id}`)
		);
		expect(stuck).toEqual([]);
	});
});
