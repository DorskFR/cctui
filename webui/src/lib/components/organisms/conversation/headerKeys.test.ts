import { describe, expect, it, vi } from 'vitest';
import { isMac } from '#lib/platform.ts';
import { headerKeyAction, runHeaderKey, type HeaderKeyState } from './headerKeys';

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

describe('runHeaderKey', () => {
	const withPrevent = (e: KeyboardEvent) => {
		const preventDefault = vi.fn();
		return { e: { ...e, preventDefault } as unknown as KeyboardEvent, preventDefault };
	};
	const handlers = (consumed = false) => ({
		onsearch: vi.fn(),
		onstoparchive: vi.fn(),
		onescape: vi.fn(() => consumed),
		onescapeaction: vi.fn()
	});

	it('opens search and archives on their chords', () => {
		const h = handlers();
		const f = withPrevent(chord('f'));
		runHeaderKey(f.e, state(), h);
		expect(h.onsearch).toHaveBeenCalledOnce();
		expect(f.preventDefault).toHaveBeenCalled();
		runHeaderKey(withPrevent(chord('e')).e, state(), h);
		expect(h.onstoparchive).toHaveBeenCalledOnce();
	});

	it('gives the find bar first refusal on Escape', () => {
		const h = handlers(true);
		const esc = withPrevent(escapeKey());
		runHeaderKey(esc.e, state(), h);
		expect(esc.preventDefault).toHaveBeenCalled();
		expect(h.onescapeaction).not.toHaveBeenCalled();
	});

	it('runs the escape action when nothing consumed it', () => {
		const h = handlers(false);
		runHeaderKey(withPrevent(escapeKey()).e, state(), h);
		expect(h.onescapeaction).toHaveBeenCalledOnce();
	});
});
