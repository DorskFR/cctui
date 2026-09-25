import { describe, expect, it, vi } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { allFilter } from './filters';
import { buildLines, createLineBuilder, type LineBuildCtx } from './lines';
import { mergeLiveEvent } from './stream.svelte';

const text = (content: string, ts: number, seq: number): AgentEvent => ({
	type: 'text',
	content,
	meta: false,
	kind: null,
	ts,
	message_id: null,
	usage: null,
	seq
});

const transcript = (n: number): AgentEvent[] =>
	Array.from({ length: n }, (_, i) => text(`message ${i}`, 1000 + i, i + 1));

const makeCtx = (renderKey = 'a') => {
	const filter = allFilter(true);
	const renderMarkdown = vi.fn((s: string) => `<p>${s}</p>`);
	const renderCode = vi.fn((t: string) => `<code>${t}</code>`);
	const ctx: LineBuildCtx = {
		visible: (c) => filter[c],
		renderMarkdown,
		renderCode,
		prettyJson: true,
		prettyDiff: true,
		renderKey
	};
	return { ctx, renderMarkdown, renderCode, filter };
};

describe('createLineBuilder', () => {
	it('renders markdown only for an appended event', () => {
		const build = createLineBuilder();
		const { ctx, renderMarkdown } = makeCtx();
		const events = transcript(500);
		build(events, ctx);
		expect(renderMarkdown).toHaveBeenCalledTimes(500);
		renderMarkdown.mockClear();
		const next = [...events, text('fresh', 5000, 501)];
		const lines = build(next, ctx);
		expect(renderMarkdown).toHaveBeenCalledTimes(1);
		expect(renderMarkdown).toHaveBeenCalledWith('fresh');
		expect(lines.at(-1)?.html).toBe('<p>fresh</p>');
	});

	it('produces the same lines as an unmemoized build', () => {
		const build = createLineBuilder();
		const { ctx } = makeCtx();
		const events = transcript(20);
		build(events, ctx);
		expect(build(events, ctx)).toEqual(buildLines(events, ctx));
	});

	it('re-renders everything when the render options change', () => {
		const build = createLineBuilder();
		const a = makeCtx('tables');
		const events = transcript(10);
		build(events, a.ctx);
		const b = makeCtx('no-tables');
		build(events, b.ctx);
		expect(b.renderMarkdown).toHaveBeenCalledTimes(10);
	});

	it('still honours visibility toggles from the cache', () => {
		const build = createLineBuilder();
		const { ctx, filter, renderMarkdown } = makeCtx();
		const events = transcript(5);
		expect(build(events, ctx)).toHaveLength(5);
		filter.assistant = false;
		expect(build(events, ctx)).toHaveLength(0);
		filter.assistant = true;
		renderMarkdown.mockClear();
		expect(build(events, ctx)).toHaveLength(5);
		expect(renderMarkdown).toHaveBeenCalledTimes(5);
	});
});

describe('buildLines duration pass', () => {
	it('stays linear on a long transcript', () => {
		const { ctx } = makeCtx();
		const events = transcript(20000);
		const t0 = performance.now();
		buildLines(events, ctx);
		const small = performance.now() - t0;
		expect(small).toBeLessThan(2000);
	});

	it('measures each assistant line from the previous user or assistant line', () => {
		const { ctx } = makeCtx();
		const lines = buildLines([text('a', 100, 1), text('b', 250, 2)], ctx);
		expect(lines[1].durationMs).toBe(150);
	});
});

describe('mergeLiveEvent', () => {
	it('appends an event past the tail', () => {
		const prev = transcript(3);
		const ev = text('tail', 9000, 10);
		expect(mergeLiveEvent(prev, ev)?.at(-1)).toBe(ev);
	});

	it('inserts an out-of-order event by seq', () => {
		const prev = [text('x', 1, 1), text('z', 3, 3)];
		const ev = text('y', 2, 2);
		expect(mergeLiveEvent(prev, ev)?.map((e) => e.seq)).toEqual([1, 2, 3]);
	});
});
