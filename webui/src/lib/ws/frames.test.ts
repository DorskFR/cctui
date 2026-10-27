import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { lastMessageExcerpt, streamListPatch } from './frames';

describe('lastMessageExcerpt', () => {
	it('previews a spoken prompt as its utterance', () => {
		const spoken =
			'▷ User: <cctui-session-context voice="spoken">\nThe user spoke this message aloud.\n</cctui-session-context>\n\nwhy is the sky blue';
		expect(lastMessageExcerpt(spoken)).toBe('why is the sky blue');
	});

	it('keeps a prompt that is nothing but a wrapper, and plain prose', () => {
		const only = '<cctui-session-context>\nctx\n</cctui-session-context>';
		expect(lastMessageExcerpt(only)).toBe('<cctui-session-context> ctx </cctui-session-context>');
		expect(lastMessageExcerpt('▷ User: line one\n\n line two')).toBe('line one line two');
	});
});

describe('streamListPatch', () => {
	const text = (content: string, kind: string | null = null, meta = false) =>
		({ type: 'text', content, meta, kind, ts: 1000 }) as unknown as AgentEvent;

	it('patches the row with a typed user turn', () => {
		expect(streamListPatch('s', text('▷ User: hello  there'))).toEqual({
			session_id: 's',
			last_message_text: 'hello there',
			last_message_at: new Date(1000).toISOString()
		});
		const reply = { type: 'reply', content: 'hi', ts: 1000 } as unknown as AgentEvent;
		expect(streamListPatch('s', reply)?.last_message_text).toBe('hi');
	});

	it('never previews an annotation, marker or queue op', () => {
		for (const ev of [
			text('turn_duration:3752', 'turn_annotation', true),
			text('stop_hook_summary', 'turn_annotation', true),
			text('file_history:snapshot', 'turn_annotation', true),
			text('· mode: normal', 'system_marker', true),
			text('next prompt', 'queue_op', true),
			text('▷ User: queued', 'queue_op', true)
		])
			expect(streamListPatch('s', ev)).toBeNull();
	});

	it('leaves assistant text and non-text events alone', () => {
		expect(streamListPatch('s', text('assistant prose'))).toBeNull();
		expect(streamListPatch('s', { type: 'turn_end', ts: 1000 } as unknown as AgentEvent)).toBeNull();
	});
});
