import { describe, expect, it } from 'vitest';
import { pickerKeyAction, typeaheadIndex } from './profilePickerKeys';

const closed = (key: string, index: number, mods = {}) =>
	pickerKeyAction({ key, ...mods }, { open: false, count: 3, index });
const opened = (key: string, index: number, mods = {}) =>
	pickerKeyAction({ key, ...mods }, { open: true, count: 3, index });

describe('pickerKeyAction', () => {
	it('closed, ↑/↓ step the selection and stop at the ends; Home/End jump', () => {
		expect(closed('ArrowDown', 0)).toEqual({ type: 'change', index: 1 });
		expect(closed('ArrowDown', 2)).toEqual({ type: 'change', index: 2 });
		expect(closed('ArrowUp', 1)).toEqual({ type: 'change', index: 0 });
		expect(closed('ArrowUp', 0)).toEqual({ type: 'change', index: 0 });
		expect(closed('ArrowDown', -1)).toEqual({ type: 'change', index: 0 });
		expect(closed('ArrowUp', -1)).toEqual({ type: 'change', index: 2 });
		expect(closed('Home', 2)).toEqual({ type: 'change', index: 0 });
		expect(closed('End', 0)).toEqual({ type: 'change', index: 2 });
	});

	it('closed, Enter, Space and Alt+↓ open; Cmd/Ctrl chords are left to the form', () => {
		expect(closed('Enter', 0)).toEqual({ type: 'open' });
		expect(closed(' ', 0)).toEqual({ type: 'open' });
		expect(closed('ArrowDown', 0, { altKey: true })).toEqual({ type: 'open' });
		expect(closed('Enter', 0, { metaKey: true })).toBeUndefined();
		expect(closed('Enter', 0, { ctrlKey: true })).toBeUndefined();
	});

	it('open, ↑/↓ move the highlight, Enter picks it, Escape and Tab close', () => {
		expect(opened('ArrowDown', 1)).toEqual({ type: 'move', index: 2 });
		expect(opened('ArrowUp', 1)).toEqual({ type: 'move', index: 0 });
		expect(opened('End', 0)).toEqual({ type: 'move', index: 2 });
		expect(opened('Enter', 1)).toEqual({ type: 'select' });
		expect(opened('Enter', -1)).toEqual({ type: 'close' });
		expect(opened('Escape', 1)).toEqual({ type: 'close' });
		expect(opened('Tab', 1)).toEqual({ type: 'close' });
	});

	it('feeds printable keys to the typeahead and ignores the rest', () => {
		expect(closed('q', 0)).toEqual({ type: 'type', char: 'q' });
		expect(opened('q', 0)).toEqual({ type: 'type', char: 'q' });
		expect(closed('Shift', 0)).toBeUndefined();
		expect(pickerKeyAction({ key: 'ArrowDown' }, { open: false, count: 0, index: -1 })).toBeUndefined();
	});
});

describe('typeaheadIndex', () => {
	const labels = ['Alpha', 'Beta', 'Bravo', 'Charlie'];
	it('finds the next label after the current one, wrapping', () => {
		expect(typeaheadIndex(labels, 'b', 0)).toBe(1);
		expect(typeaheadIndex(labels, 'b', 1)).toBe(2);
		expect(typeaheadIndex(labels, 'bb', 2)).toBe(1);
		expect(typeaheadIndex(labels, 'a', 3)).toBe(0);
	});
	it('matches a longer prefix from the current label', () => {
		expect(typeaheadIndex(labels, 'br', 1)).toBe(2);
		expect(typeaheadIndex(labels, 'CH', 0)).toBe(3);
		expect(typeaheadIndex(labels, 'z', 0)).toBeUndefined();
	});
});
