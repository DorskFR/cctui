import { describe, expect, it } from 'vitest';
import { compile, type Journey } from '@dorsk/journey';
import welcome from '../../../journeys/welcome.journey';

const pub = compile(welcome, { public: true });
const book = compile(welcome);

describe('welcome spec', () => {
	it('anchors every step, so the tour points at the app instead of describing it', () => {
		for (const ir of [pub, book]) {
			for (const step of ir.steps) {
				expect(step.target, step.id).toBeDefined();
			}
		}
	});

	it('survives --public intact so it is the same tour on an empty instance', () => {
		expect(pub.steps.map((s) => s.id)).toEqual(book.steps.map((s) => s.id));
		expect(pub.steps.length).toBeGreaterThan(0);
	});

	it('gives every step a title and a body to draw', () => {
		for (const step of pub.steps) {
			expect(step.say?.title, step.id).toBeTruthy();
			expect(step.say?.body, step.id).toBeTruthy();
		}
	});

	it('walks the real screens in order', () => {
		expect(pub.steps.map((s) => s.id)).toEqual([
			'overview',
			'attention',
			'to-sessions',
			'sessions',
			'start',
			'to-accounts',
			'accounts',
			'to-access',
			'access',
			'settings',
			'guides'
		]);
	});

	it('changes page by asking the user to click the nav, not by interrupting', () => {
		// Only the closing hop to the guides page declares a route; every other
		// screen is reached by the user clicking the nav item the step points at.
		const hops = pub.steps.filter((s) => s.id.startsWith('to-'));
		for (const hop of hops) expect(hop.target, hop.id).toBe(`nav[${hop.id.slice(3)}]`);
		expect(hops.map((s) => s.id)).toEqual(['to-sessions', 'to-accounts', 'to-access']);
		for (const hop of hops) expect(hop.do.kind, hop.id).toBe('click');
		expect(pub.steps.slice(1).filter((s) => s.route !== undefined).map((s) => s.id)).toEqual([
			'guides'
		]);
	});

	it('marks no step optional, which guide mode cannot honour anyway', () => {
		for (const step of pub.steps) expect(step.optional, step.id).toBeUndefined();
	});

	it('ends on the guides page, pointing at the guide to take next', () => {
		const last = pub.steps.at(-1)!;
		expect(last.route).toBe('/settings/guides');
		expect(last.target).toBe('guide[sessions-list]');
	});

	it('never autostarts: the user opens it from the Guides page', () => {
		expect((welcome as Journey).autostart).toBeUndefined();
		expect(welcome.route).toBe('/');
	});

	it('bumps the version so the rewritten tour re-shows once', () => {
		expect(welcome.version).toBeGreaterThan(2);
	});
});
