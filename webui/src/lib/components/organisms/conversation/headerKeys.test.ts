import { describe, expect, it } from 'vitest';
import { isMac } from '$lib/platform';
import { headerKeyAction, type HeaderKeyState } from './headerKeys';

const key = (init: Partial<KeyboardEvent>) => init as KeyboardEvent;
const escapeKey = () => key({ key: 'Escape', altKey: false, shiftKey: false });
const chord = (k: string) =>
	key({ key: k, altKey: false, shiftKey: false, metaKey: isMac(), ctrlKey: !isMac() });

const state = (over: Partial<HeaderKeyState> = {}): HeaderKeyState => ({
	active: true,
	renaming: false,
	archived: false,
	canSearch: true,
	archiveShortcut: true,
	...over
});

describe('headerKeyAction', () => {
	it('answers the chords on the active header', () => {
		expect(headerKeyAction(escapeKey(), state())).toBe('escape');
		expect(headerKeyAction(chord('e'), state())).toBe('archive');
		expect(headerKeyAction(chord('f'), state())).toBe('search');
	});

	it('is silent on an inactive header', () => {
		const inactive = state({ active: false });
		expect(headerKeyAction(escapeKey(), inactive)).toBeNull();
		expect(headerKeyAction(chord('e'), inactive)).toBeNull();
		expect(headerKeyAction(chord('f'), inactive)).toBeNull();
	});

	it('dispatches once across four mounted tiles', () => {
		const tileStates = ['a', 'b', 'c', 'd'].map((id) => ({ id, s: state({ active: id === 'c' }) }));
		const fired = tileStates.filter((t) => headerKeyAction(escapeKey(), t.s) === 'escape');
		expect(fired.map((t) => t.id)).toEqual(['c']);
	});

	it('takes no chord while renaming, nor archive on an archived session', () => {
		expect(headerKeyAction(escapeKey(), state({ renaming: true }))).toBeNull();
		expect(headerKeyAction(chord('e'), state({ renaming: true }))).toBeNull();
		expect(headerKeyAction(chord('e'), state({ archived: true }))).toBeNull();
		expect(headerKeyAction(chord('e'), state({ archiveShortcut: false }))).toBeNull();
		expect(headerKeyAction(chord('f'), state({ canSearch: false }))).toBeNull();
	});
});
