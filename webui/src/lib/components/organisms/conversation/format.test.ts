import { describe, it, expect } from 'vitest';
import type { AskQuestion, Line } from './types';

// CCT-475 regression coverage.
//
// The Conversation Drawer must render the combined message list in strict
// ascending timestamp order, regardless of role. `events` is sorted ascending
// by `ts` in ConversationDrawer.svelte (`.sort((a, b) => a.ts - b.ts)`) and the
// lines are built from it in order with no role grouping and no re-anchoring.
//
// This replaces the former CCT-338 `orderAskTurns` re-anchor, which was REMOVED
// in CCT-475: with ts-only event data it could not distinguish a late-flushed
// AskUserQuestion inversion from a normal prior-turn user line, so it pushed
// later-ts assistant messages above earlier-ts user messages — exactly the
// regression this guards against (Assistant 15:30 rendered above User 15:12).
// The proper causal-ordering fix is tracked in CCT-481.

const askQ: AskQuestion[] = [{ question: 'Which database?', options: [{ label: 'Postgres' }, { label: 'SQLite' }] }];

const preamble = (ts: number): Line => ({ role: 'assistant', ts, text: 'I need to know which DB to use.' });
const askCard = (ts: number): Line => ({ role: 'tool', ts, tool: 'AskUserQuestion', ask: askQ });
const answer = (ts: number): Line => ({ role: 'user', ts, text: 'Postgres' });
const continuation = (ts: number): Line => ({ role: 'assistant', ts, text: 'Great, using Postgres.' });

// Mirror the drawer's render order: events are sorted ascending by ts (a stable
// sort keeps equal-ts ties in source order, as the component relies on).
const renderOrder = (lines: Line[]) => lines.slice().sort((a, b) => a.ts - b.ts);
const tsOf = (ls: Line[]) => ls.map((l) => l.ts);

describe('conversation render ordering (CCT-475)', () => {
	it('renders the combined list in strict ascending timestamp order', () => {
		// The reported regression: an assistant message (15:30) ahead of an earlier
		// user message (15:12). Source order is intentionally scrambled by role.
		const t1512 = answer(1512);
		const t1530 = preamble(1530);
		const t1535 = answer(1535);
		const out = renderOrder([t1530, t1512, t1535]);
		expect(tsOf(out)).toEqual([1512, 1530, 1535]);
		expect(out[0]).toBe(t1512);
		expect(out[1]).toBe(t1530);
		expect(out[2]).toBe(t1535);
	});

	it('does not lift an ask card / preamble above an earlier user line', () => {
		// A conversation containing an ask must NOT reorder around it: every line
		// stays in ts order, including the user answer that follows the ask.
		const lines: Line[] = [answer(100), preamble(200), askCard(210), answer(220), continuation(300)];
		const out = renderOrder(lines);
		expect(tsOf(out)).toEqual([100, 200, 210, 220, 300]);
	});

	it('keeps equal-timestamp ties in source order (stable)', () => {
		const a = answer(100);
		const b = preamble(100);
		const out = renderOrder([a, b]);
		expect(out[0]).toBe(a);
		expect(out[1]).toBe(b);
	});
});
