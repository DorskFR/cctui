import { describe, expect, it } from 'vitest';
import { compile } from '@dorsk/journey';
import appsPagePlugin from '../../../journeys/apps-page-plugin.journey';

const book = compile(appsPagePlugin);
const pub = compile(appsPagePlugin, { public: true });

describe('apps-page-plugin spec', () => {
	it('is a book-only journey: the fixture enables the plugin, so nothing survives --public', () => {
		expect(pub.steps).toEqual([]);
		expect(book.steps.length).toBeGreaterThan(0);
	});

	it('waits on no optional step, so a target the fixture never renders cannot burn its budget', () => {
		for (const step of book.steps) expect(step.optional, step.id).toBeUndefined();
	});

	it('only targets what the enabled plugin renders', () => {
		for (const step of book.steps) {
			expect(JSON.stringify(step.target), step.id).toMatch(/^"pagedemo/);
			expect(JSON.stringify(step.expect ?? []), step.id).not.toMatch(/not-enabled/);
		}
	});

	it('walks the plugin page then one of its routes', () => {
		expect(book.steps.map((s) => s.id)).toEqual(['page', 'navigate']);
	});
});
