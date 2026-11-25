import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { compile } from '@dorsk/journey';
import acpSession from '../../../journeys/acp-session.journey';

const book = compile(acpSession);
const pub = compile(acpSession, { public: true });
const SEEDED = 'a0000000-0000-4000-8000-000000000006';

describe('acp-session spec', () => {
	it('is a book-only journey: it opens a fixture row by id, so nothing survives --public', () => {
		expect(pub.steps).toEqual([]);
		expect(book.steps.map((s) => s.id)).toEqual(['open', 'header', 'tools']);
	});

	it('opens the gemini session the fixture seeds', () => {
		expect(JSON.stringify(book.steps[0].target)).toContain(SEEDED);
		const seed = readFileSync('../deploy/local/fixture/seed.sql', 'utf8');
		const row = seed.slice(seed.indexOf(`('${SEEDED}', NULL`));
		expect(row).toMatch(new RegExp(`^\\('${SEEDED}'[^;]*'gemini', 'idle'`));
	});

	it('captures every step it shows', () => {
		expect(book.steps.map((s) => s.capture?.name)).toEqual(['drawer', 'header', 'tools']);
	});
});
