import { describe, expect, it } from 'vitest';
import { lastMessageExcerpt } from './frames';

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
