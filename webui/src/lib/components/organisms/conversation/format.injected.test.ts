import { describe, expect, it } from 'vitest';
import { CCTUI_TAGS, LEGACY_TAGS, injectedSummary, parsePeerMessage, splitInjected } from './format';

const NEW_FOLLOWUP =
	'<cctui-session-context>\nx\n</cctui-session-context>\n\n' +
	'<cctui-previous-conversation session="a" id="s1">\nbrief\n</cctui-previous-conversation>\n\nContinue from this.';
const LEGACY_FOLLOWUP =
	'<session-context>\nx\n</session-context>\n\n' +
	'<previous-conversation session="a" id="s1">\n**User:**\n\nq\n</previous-conversation>\n\n===\n\nNow add tests.';

describe('splitInjected', () => {
	const cases: [string, string, [string, string][], string][] = [
		[
			'leading new name',
			'<cctui-session-context>\nsession: a\n</cctui-session-context>\n\nfix the bug',
			[['session-context', CCTUI_TAGS.sessionContext]],
			'fix the bug'
		],
		[
			'leading legacy name',
			'<session-context>\ncwd: /w\n</session-context>\n\nfix the bug',
			[['session-context', LEGACY_TAGS.sessionContext]],
			'fix the bug'
		],
		[
			'quoted mid-text stays prose',
			'look at this:\n<session-context>\ncwd: /w\n</session-context>',
			[],
			'look at this:\n<session-context>\ncwd: /w\n</session-context>'
		],
		[
			'legacy follow-up with === separator',
			LEGACY_FOLLOWUP,
			[
				['session-context', LEGACY_TAGS.sessionContext],
				['previous-conversation', LEGACY_TAGS.previousConversation]
			],
			'Now add tests.'
		],
		[
			'new follow-up without separator',
			NEW_FOLLOWUP,
			[
				['session-context', CCTUI_TAGS.sessionContext],
				['previous-conversation', CCTUI_TAGS.previousConversation]
			],
			'Continue from this.'
		],
		[
			'no prose',
			'<cctui-session-context>\nx\n</cctui-session-context>\n',
			[['session-context', CCTUI_TAGS.sessionContext]],
			''
		],
		['plain prose', 'hello', [], 'hello'],
		[
			'an unclosed wrapper is prose',
			'<session-context>\nnever closed',
			[],
			'<session-context>\nnever closed'
		],
		[
			'a lookalike tag is prose',
			'<session-contextual>x</session-contextual> hi',
			[],
			'<session-contextual>x</session-contextual> hi'
		],
		['a leading === alone is prose', '===\nheading', [], '===\nheading'],
		[
			'peer envelopes are not context',
			'<cctui-peer-message from="s1">\nhi\n</cctui-peer-message>',
			[],
			'<cctui-peer-message from="s1">\nhi\n</cctui-peer-message>'
		]
	];

	it.each(cases)('%s', (_name, content, want, prose) => {
		const got = splitInjected(content);
		expect(got.blocks.map((b) => [b.kind, b.tag])).toEqual(want);
		expect(got.prose).toBe(prose);
	});

	it('reads the bodies and entity-decoded attributes', () => {
		const { blocks } = splitInjected(
			'<cctui-previous-conversation session="my &quot;run&quot; &amp; co" id="s1">\nb\n</cctui-previous-conversation>'
		);
		expect(blocks[0].attrs).toEqual({ session: 'my "run" & co', id: 's1' });
		expect(blocks[0].body).toBe('b');
		expect(splitInjected(LEGACY_FOLLOWUP).blocks[1].body).toBe('**User:**\n\nq');
	});

	it('keeps a nested legacy follow-up as one block', () => {
		const content =
			'<previous-conversation session="b" id="s2">\n**User:**\n\n' +
			'<previous-conversation session="a" id="s1">\nold\n</previous-conversation>\n\n===\n\nmore\n' +
			'</previous-conversation>\n\n===\n\nask';
		const { blocks, prose } = splitInjected(content);
		expect(blocks).toHaveLength(1);
		expect(blocks[0].body.endsWith('more')).toBe(true);
		expect(blocks[0].attrs.id).toBe('s2');
		expect(prose).toBe('ask');
	});
});

describe('peer wrappers under either name', () => {
	it('reads the namespaced and the legacy tag alike', () => {
		for (const tag of [CCTUI_TAGS.peerMessage, LEGACY_TAGS.peerMessage]) {
			const peer = parsePeerMessage(`<${tag} from="s1" from-name="lane-a">\nhi\n</${tag}>`);
			expect(peer).toMatchObject({ from: 'lane-a', body: 'hi', remote: false });
		}
	});

	it('does not close a namespaced envelope on the legacy tag', () => {
		expect(parsePeerMessage('<cctui-peer-message from="s1">hi</cross-session-message>')).toBeNull();
	});
});

describe('injectedSummary', () => {
	it('takes the first meaningful line and counts the non-blank ones', () => {
		expect(injectedSummary('\n**User:**\n\nfix it\n')).toEqual({ first: 'User:', lines: 2 });
		expect(injectedSummary('<rules>\n# Rules\nbe nice')).toEqual({ first: 'Rules', lines: 3 });
	});
});
