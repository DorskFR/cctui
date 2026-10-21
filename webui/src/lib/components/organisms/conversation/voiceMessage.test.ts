import { describe, expect, it } from 'vitest';
import { voiceTranscriptLanded } from './voiceMessage';
import type { Line } from './types';

const user = (text: string, voice = false): Line => ({
	role: 'user',
	ts: 1,
	html: `<p>${text}</p>`,
	text,
	...(voice ? { voice } : {})
});
const assistant = (text: string): Line => ({ role: 'assistant', ts: 2, html: '', text });

describe('voiceTranscriptLanded', () => {
	it('is false while the spoken turn has not reached the conversation', () => {
		expect(voiceTranscriptLanded([user('earlier', true)], 'what time is it')).toBe(false);
	});

	it('is true once the latest user turn is that spoken transcript', () => {
		const lines = [user('what time is it', true), assistant('noon')];
		expect(voiceTranscriptLanded(lines, '  what  time is it ')).toBe(true);
	});

	it('ignores a typed turn with the same words', () => {
		expect(voiceTranscriptLanded([user('what time is it')], 'what time is it')).toBe(false);
	});

	it('only looks at the latest user turn', () => {
		const lines = [user('what time is it', true), user('typed after')];
		expect(voiceTranscriptLanded(lines, 'what time is it')).toBe(false);
	});

	it('is false for an empty transcript', () => {
		expect(voiceTranscriptLanded([user('', true)], '')).toBe(false);
	});
});
