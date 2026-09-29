import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { allFilter } from './filters';
import { atHeadOfTranscript, headHiddenByFilter } from './headHidden';
import type { LineBuildCtx } from './lines';
import type { MsgCategory } from './types';

const ctx = (overrides: Partial<Record<MsgCategory, boolean>> = {}): LineBuildCtx => {
	const filter = { ...allFilter(true), ...overrides };
	return {
		visible: (c) => filter[c],
		renderMarkdown: (s) => `<p>${s}</p>`,
		renderCode: (text) => `<code>${text}</code>`,
		prettyJson: true,
		prettyDiff: true
	};
};

const USER_PREFIX = '▷ User:';

const text = (content: string, ts: number): AgentEvent =>
	({
		type: 'text',
		content,
		meta: false,
		kind: null,
		ts,
		message_id: null,
		usage: null,
		seq: ts
	}) as unknown as AgentEvent;

const user = (body: string, ts: number): AgentEvent => text(`${USER_PREFIX} ${body}`, ts);
const harness = (ts: number): AgentEvent =>
	user('<system-reminder>context rehydrated</system-reminder>', ts);

describe('headHiddenByFilter', () => {
	it('counts the rows the System filter hides at the head', () => {
		const events = [harness(1), harness(2), text('you are right, my list was stale', 3)];
		const hidden = headHiddenByFilter(events, ctx({ system: false }));
		expect(hidden.count).toBe(2);
		expect(hidden.categories).toEqual(['system']);
	});

	it('is empty when the oldest row is visible', () => {
		const events = [user('take over please', 1), harness(2)];
		expect(headHiddenByFilter(events, ctx({ system: false })).count).toBe(0);
	});

	it('only counts rows before the first visible one', () => {
		const events = [harness(1), text('on it', 2), harness(3)];
		expect(headHiddenByFilter(events, ctx({ system: false })).count).toBe(1);
	});

	it('reports every category needed to reveal the head', () => {
		const events = [harness(1), user('take over please', 2), text('on it', 3)];
		const hidden = headHiddenByFilter(events, ctx({ system: false, user: false }));
		expect(hidden.count).toBe(2);
		expect([...hidden.categories].sort()).toEqual(['system', 'user']);
	});

	it('is empty on an empty transcript', () => {
		expect(headHiddenByFilter([], ctx({ system: false }))).toEqual({
			count: 0,
			categories: []
		});
	});
});

describe('atHeadOfTranscript', () => {
	it('is true only when no chunk and no page remain', () => {
		expect(atHeadOfTranscript(0, false)).toBe(true);
		expect(atHeadOfTranscript(0, true)).toBe(false);
		expect(atHeadOfTranscript(12, false)).toBe(false);
	});
});
