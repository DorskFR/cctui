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
			'sessions',
			'start',
			'accounts',
			'access',
			'usage',
			'guides'
		]);
		expect(pub.steps.map((s) => s.route)).toEqual([
			'/',
			'/',
			'/sessions',
			'/sessions',
			'/accounts',
			'/access',
			'/',
			'/settings/guides'
		]);
	});

	it('captures each route once, since doc mode shoots the page and not the spotlight', () => {
		const shot = book.steps.filter((s) => s.capture);
		expect(shot.length).toBeGreaterThan(0);
		const routes = shot.map((s) => s.route);
		expect(routes).toEqual([...new Set(routes)]);
		expect(new Set(book.steps.map((s) => s.route)).size).toBe(routes.length);
	});

	it('keeps the data-dependent step optional', () => {
		const attention = pub.steps.find((s) => s.id === 'attention');
		expect(attention?.optional).toBe(true);
		expect(attention?.expect ?? []).toEqual([]);
	});

	it('never autostarts: the user opens it from the Guides page', () => {
		expect((welcome as Journey).autostart).toBeUndefined();
		expect(welcome.route).toBe('/');
	});

	it('bumps the version so the rewritten tour re-shows once', () => {
		expect(welcome.version).toBeGreaterThan(2);
	});
});
