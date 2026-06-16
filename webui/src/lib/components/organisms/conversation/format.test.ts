import { describe, it, expect } from 'vitest';
import { orderAskTurns } from './format';
import type { AskQuestion, Line } from './types';

// CCT-338 regression coverage.
//
// Claude flushes the AskUserQuestion tool_use block (and the assistant preamble
// preceding it) only AFTER the turn advances — i.e. after the user has already
// answered. Live events and persisted rows are ordered by RECEIVE-time `ts`
// (the server stamps Utc::now() on ingest), so once history is refetched or the
// page is reloaded, the preamble + ask card sort BELOW the user's answer. The
// ordering must therefore come from causal metadata (the block's position
// relative to the answer), not arrival time.

const askQ: AskQuestion[] = [{ question: 'Which database?', options: [{ label: 'Postgres' }, { label: 'SQLite' }] }];

const preamble = (ts: number): Line => ({ role: 'assistant', ts, text: 'I need to know which DB to use.' });
const askCard = (ts: number): Line => ({ role: 'tool', ts, tool: 'AskUserQuestion', ask: askQ });
const answer = (ts: number): Line => ({ role: 'user', ts, text: 'Postgres' });
const continuation = (ts: number): Line => ({ role: 'assistant', ts, text: 'Great, using Postgres.' });

const roles = (ls: Line[]) => ls.map((l) => (l.role === 'tool' ? 'ask' : l.text));

describe('orderAskTurns (CCT-338)', () => {
	it('lifts a late preamble + ask card above the answer that preceded them', () => {
		// Receive-time ordering after refetch/reload: the answer was stamped at
		// send time (earlier), the preamble + card arrived later.
		const lines: Line[] = [answer(100), preamble(200), askCard(210), continuation(300)];
		const out = orderAskTurns(lines);
		expect(roles(out)).toEqual(['I need to know which DB to use.', 'ask', 'Postgres', 'Great, using Postgres.']);
	});

	it('handles an ask card without a preamble', () => {
		const lines: Line[] = [answer(100), askCard(210), continuation(300)];
		const out = orderAskTurns(lines);
		expect(roles(out)).toEqual(['ask', 'Postgres', 'Great, using Postgres.']);
	});

	it('is a no-op when order is already causal (the live case)', () => {
		const lines: Line[] = [preamble(100), askCard(110), answer(120), continuation(130)];
		const out = orderAskTurns(lines);
		expect(roles(out)).toEqual(['I need to know which DB to use.', 'ask', 'Postgres', 'Great, using Postgres.']);
	});

	it('is idempotent — re-running keeps the corrected order', () => {
		const lines: Line[] = [answer(100), preamble(200), askCard(210), continuation(300)];
		const once = orderAskTurns(lines);
		const twice = orderAskTurns(once);
		expect(roles(twice)).toEqual(roles(once));
	});

	it('does not move an ask card that has no preceding answer', () => {
		const lines: Line[] = [preamble(100), askCard(110)];
		const out = orderAskTurns(lines);
		expect(roles(out)).toEqual(['I need to know which DB to use.', 'ask']);
	});

	it('reorders multiple ask turns independently', () => {
		const lines: Line[] = [
			answer(100),
			preamble(200),
			askCard(210),
			answer(220),
			preamble(400),
			askCard(410),
			continuation(500)
		];
		const out = orderAskTurns(lines);
		expect(roles(out)).toEqual([
			'I need to know which DB to use.',
			'ask',
			'Postgres',
			'I need to know which DB to use.',
			'ask',
			'Postgres',
			'Great, using Postgres.'
		]);
	});
});
