import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { dictatedPrompt, splitDictated } from './dictatedMark';

describe('dictated mark', () => {
	it('wraps and unwraps the dictated words', () => {
		const sent = dictatedPrompt('fix the login bug');
		expect(sent.startsWith('<cctui-session-context input="dictated">\n')).toBe(true);
		expect(splitDictated(sent)).toEqual({ text: 'fix the login bug', dictated: true });
		expect(splitDictated('fix the login bug')).toEqual({ text: 'fix the login bug', dictated: false });
	});

	it('carries the same note as the server', () => {
		const rust = readFileSync('../crates/cctui-proto/src/injected.rs', 'utf8');
		const raw = rust.match(/const DICTATED_NOTE: &str = "([\s\S]*?)";/)?.[1];
		expect(raw).toBeTruthy();
		const note = raw!.replace(/\\\n\s*/g, '');
		expect(dictatedPrompt('x')).toBe(
			`<cctui-session-context input="dictated">\n${note}\n</cctui-session-context>\n\nx`
		);
	});
});
