import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { defaultFilter } from './filters';
import { mergeEventSources } from './format';
import { buildLines, type LineBuildCtx } from './lines';
import { mergeLiveEvent } from './stream.svelte';
import { voiceTranscriptLanded } from './voiceMessage';
import fixture from './spokenTurns.fixture.json';

const ctx = (): LineBuildCtx => {
	const filter = defaultFilter();
	return {
		visible: (c) => filter[c],
		renderMarkdown: (s) => `<p>${s}</p>`,
		renderCode: (text) => `<code>${text}</code>`,
		prettyJson: true,
		prettyDiff: true
	};
};

const rows = fixture as unknown as AgentEvent[];
const UTTERANCE = 'Tell me in four or five sentences why the sky is blue.';
const SPOKEN = rows.find((r) => r.seq === 198934431) as AgentEvent & { type: 'text' };
const QUEUED = rows.find((r) => r.seq === 198934428) as AgentEvent & { type: 'text' };

const build = (events: AgentEvent[]) => buildLines(mergeEventSources(events, [], []), ctx());

describe('spoken turns in the thread', () => {
	it('renders each of the identical spoken turns once, as a voice message', () => {
		const lines = build(rows);
		const users = lines.filter((l) => l.role === 'user' && l.seq !== 198934401);
		expect(users.map((l) => l.seq)).toEqual([
			198934431, 198934477, 198934801, 198934915, 198934943
		]);
		expect(users.every((l) => l.voice && l.text === UTTERANCE)).toBe(true);
	});

	it('keeps every reply, even two with identical text', () => {
		const lines = build(rows);
		expect(lines.filter((l) => l.role === 'assistant').map((l) => l.seq)).toEqual([
			198934408, 198934447, 198934478, 198934900, 198934919, 198934948
		]);
	});

	it('leaves no queued bubble and no context markup', () => {
		const lines = build(rows);
		expect(lines.filter((l) => l.queued)).toEqual([]);
		for (const l of lines) {
			expect(l.text ?? '').not.toContain('<cctui-session-context');
			expect(l.html ?? '').not.toContain('cctui-session-context');
		}
		expect(lines.find((l) => l.seq === 198934431)?.queuedAt).toBe(QUEUED.ts);
	});

	it('shows the queued spoken prompt without markup until its user line lands', () => {
		const lines = build([QUEUED]);
		expect(lines).toHaveLength(1);
		expect(lines[0].queued).toBe(true);
		expect(lines[0].text).toBe(UTTERANCE);
	});

	it('appends a live spoken turn that repeats an earlier one verbatim', () => {
		const live = { ...SPOKEN, seq: 198935000, ts: SPOKEN.ts + 1, line_id: 'live-line' } as AgentEvent;
		const merged = mergeLiveEvent(rows, live);
		expect(merged).toHaveLength(rows.length + 1);
		expect(mergeLiveEvent(merged, { ...live, seq: 198935001 } as AgentEvent)).toBe(merged);
		const lines = buildLines(merged ?? [], ctx());
		expect(lines.filter((l) => l.role === 'user' && l.voice)).toHaveLength(6);
		expect(voiceTranscriptLanded(lines, UTTERANCE)).toBe(true);
	});

	it('renders two identical live spoken turns in a row, each once', () => {
		const { line_id: _, ...shape } = SPOKEN as AgentEvent & { line_id?: string };
		const live = (seq: number) => ({ ...shape, seq, ts: SPOKEN.ts + seq }) as AgentEvent;
		const answer = {
			type: 'text',
			content: 'Because air scatters blue light.',
			meta: false,
			message_id: 'msg_live',
			seq: 198935001,
			ts: SPOKEN.ts + 198935001
		} as AgentEvent;
		const stream = [live(198935000), answer, live(198935002)];
		let merged: AgentEvent[] | undefined = rows;
		for (const ev of stream) merged = mergeLiveEvent(merged, ev);
		expect(merged).toHaveLength(rows.length + 3);
		expect(mergeLiveEvent(merged, live(198935002))).toBe(merged);
		const spoken = (events: AgentEvent[], buffer: AgentEvent[]) =>
			buildLines(mergeEventSources(events, [], buffer), ctx())
				.filter((l) => l.role === 'user' && l.voice)
				.map((l) => l.seq);
		const expected = [198934431, 198934477, 198934801, 198934915, 198934943, 198935000, 198935002];
		expect(spoken(merged ?? [], [])).toEqual(expected);
		expect(spoken(merged ?? [], stream)).toEqual(expected);
		expect(spoken(rows, stream)).toEqual(expected);
	});
});
