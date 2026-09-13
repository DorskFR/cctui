import { describe, expect, it } from 'vitest';
import { compile } from '@dorsk/journey';
import welcome from '../../../journeys/welcome.journey';

const pub = compile(welcome, { public: true });
const book = compile(welcome);

describe('welcome deck spec', () => {
	it('names no target, which is what routes it to the carousel presenter', () => {
		for (const ir of [pub, book]) {
			for (const step of ir.steps) {
				expect(step.target, step.id).toBeUndefined();
				expect(step.expect ?? [], step.id).toEqual([]);
			}
		}
	});

	it('survives --public intact so the deck is the same on an empty instance', () => {
		expect(pub.steps.map((s) => s.id)).toEqual(book.steps.map((s) => s.id));
		expect(pub.steps.length).toBeGreaterThan(0);
	});

	it('gives every card a title and a body to draw', () => {
		for (const step of pub.steps) {
			expect(step.say?.title, step.id).toBeTruthy();
			expect(step.say?.body, step.id).toBeTruthy();
		}
	});

	it('walks every navigable surface of the app', () => {
		expect(pub.steps.map((s) => s.id)).toEqual([
			'what',
			'shape',
			'overview',
			'sessions',
			'spawn',
			'follow',
			'accounts',
			'access',
			'bookmarks',
			'review',
			'usage',
			'settings',
			'guides'
		]);
	});

	it('autostarts once on the landing route', () => {
		expect(welcome.autostart).toEqual({ route: '/', once: true });
		expect(welcome.route).toBe('/');
	});
});
