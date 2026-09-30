import { describe, expect, it } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import {
	applyMention,
	filterMentions,
	findTrigger,
	mentionToken,
	mentionableSessions,
	moveSelection
} from '$lib/mention/mention';
import { parityFixture } from './fixtures';

type RawSession = {
	id: string;
	status: string;
	name: string | null;
	working_dir: string | null;
	machine_name: string | null;
};

type Fixture = {
	findTrigger: { text: string; caret: number; out: { start: number; query: string } | null }[];
	mentionableSessions: { sessions: RawSession[]; exclude_id: string | null; out: string[] }[];
	filterMentions: { sessions: RawSession[]; query: string; out: string[] }[];
	mentionToken: { id: string; name: string | null; out: string }[];
	applyMention: {
		text: string;
		caret: number;
		id: string;
		name: string | null;
		out: { text: string; caret: number };
	}[];
	moveSelection: { index: number; delta: 1 | -1; length: number; out: number }[];
};

const fx = parityFixture<Fixture>('mention');

const asSessions = (raw: RawSession[]): SessionListItem[] => raw as unknown as SessionListItem[];

describe('mention parity fixtures', () => {
	it('findTrigger', () => {
		for (const c of fx.findTrigger)
			expect(findTrigger(c.text, c.caret), JSON.stringify(c)).toEqual(c.out);
	});
	it('mentionableSessions', () => {
		for (const c of fx.mentionableSessions)
			expect(
				mentionableSessions(asSessions(c.sessions), c.exclude_id).map((s) => s.id),
				JSON.stringify(c.exclude_id)
			).toEqual(c.out);
	});
	it('filterMentions', () => {
		for (const c of fx.filterMentions)
			expect(filterMentions(asSessions(c.sessions), c.query).map((s) => s.id), c.query).toEqual(c.out);
	});
	it('mentionToken', () => {
		for (const c of fx.mentionToken)
			expect(mentionToken({ id: c.id, name: c.name }), JSON.stringify(c)).toBe(c.out);
	});
	it('applyMention', () => {
		for (const c of fx.applyMention) {
			const trigger = findTrigger(c.text, c.caret);
			expect(trigger).not.toBeNull();
			expect(
				applyMention(c.text, c.caret, trigger!, { id: c.id, name: c.name }),
				JSON.stringify(c)
			).toEqual(c.out);
		}
	});
	it('moveSelection', () => {
		for (const c of fx.moveSelection)
			expect(moveSelection(c.index, c.delta, c.length), JSON.stringify(c)).toBe(c.out);
	});
});
