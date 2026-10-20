import { describe, expect, it } from 'vitest';
import { parseCctuiverseLinked, parsePeerMessage } from './format';

describe('parsePeerMessage with a remote envelope', () => {
	const remote =
		'<cross-session-message from="remote:7b1e" from-name="alice-lane (remote)" origin="remote" n="a1b2c3d4e5f6">\nhello from afar\n</cross-session-message>';

	it('reads the sender label and marks it remote', () => {
		expect(parsePeerMessage(remote)).toEqual({
			from: 'alice-lane (remote)',
			room: undefined,
			remote: true,
			body: 'hello from afar'
		});
	});

	it('accepts the harness preamble before a remote envelope', () => {
		const peer = parsePeerMessage(`Another session sent a message:\n${remote}`);
		expect(peer?.remote).toBe(true);
	});

	it('treats a remote: address without origin as remote', () => {
		const peer = parsePeerMessage(
			'<cross-session-message from="remote:7b1e">hi</cross-session-message>'
		);
		expect(peer?.remote).toBe(true);
		expect(peer?.from).toBe('remote:7b1e');
	});

	it('keeps a local peer message local', () => {
		const peer = parsePeerMessage(
			'<cross-session-message from="s-1" from-name="lane-a">ping</cross-session-message>'
		);
		expect(peer).toEqual({ from: 'lane-a', room: undefined, remote: false, body: 'ping' });
	});

	it('ignores a remote envelope quoted mid-message by a human', () => {
		expect(parsePeerMessage(`look at this:\n${remote}`)).toBeNull();
	});
});

describe('parseCctuiverseLinked', () => {
	const notice =
		'<cctuiverse-linked peer="bob-lane" id="remote:7b1e">\nYou are now linked with "bob-lane".\n</cctuiverse-linked>';

	it('reads the peer label', () => {
		expect(parseCctuiverseLinked(notice)).toEqual({ peer: 'bob-lane' });
	});

	it('is not a room-join notice or a plain turn', () => {
		expect(parseCctuiverseLinked('<cctui-room-joined room="x">hi</cctui-room-joined>')).toBeNull();
		expect(parseCctuiverseLinked('hello')).toBeNull();
	});

	it('ignores a notice quoted after human text', () => {
		expect(parseCctuiverseLinked(`fyi\n${notice}`)).toBeNull();
	});
});
