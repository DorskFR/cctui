import { describe, expect, it } from 'vitest';
import { HistoryNav } from './historyNav';

function setup(list: string[], initial = '') {
	let value = initial;
	let caret = initial.length;
	const el = {
		get selectionStart() {
			return caret;
		},
		get selectionEnd() {
			return caret;
		}
	} as HTMLTextAreaElement;
	const nav = new HistoryNav({
		list: () => list,
		value: () => value,
		setValue: (v) => {
			value = v;
			caret = v.length;
		},
		el: () => el
	});
	return {
		nav,
		get value() {
			return value;
		},
		caretTo(at: number) {
			caret = at;
		},
		key(name: string) {
			let prevented = false;
			const e = {
				key: name,
				preventDefault: () => {
					prevented = true;
				}
			} as unknown as KeyboardEvent;
			const handled = nav.handleKey(e);
			return { handled, prevented };
		}
	};
}

describe('HistoryNav', () => {
	it('walks back through history newest-first from the caret start', () => {
		const t = setup(['old', 'mid', 'new']);
		t.caretTo(0);

		expect(t.key('ArrowUp')).toEqual({ handled: true, prevented: true });
		expect(t.value).toBe('new');
		t.key('ArrowUp');
		expect(t.value).toBe('mid');
		t.key('ArrowUp');
		expect(t.value).toBe('old');
		t.key('ArrowUp');
		expect(t.value).toBe('old');
	});

	it('does not recall mid-text', () => {
		const t = setup(['old', 'new'], 'typing here');
		t.caretTo(4);

		expect(t.key('ArrowUp')).toEqual({ handled: false, prevented: false });
		expect(t.value).toBe('typing here');
	});

	it('restores the stashed draft on the way forward', () => {
		const t = setup(['old', 'new'], 'in progress');
		t.caretTo(0);

		t.key('ArrowUp');
		t.key('ArrowUp');
		expect(t.value).toBe('old');

		t.key('ArrowDown');
		expect(t.value).toBe('new');
		t.key('ArrowDown');
		expect(t.value).toBe('in progress');
		expect(t.nav.browsing).toBe(false);
	});

	it('ignores ArrowDown when not browsing', () => {
		const t = setup(['old'], 'draft');
		expect(t.key('ArrowDown')).toEqual({ handled: false, prevented: false });
		expect(t.value).toBe('draft');
	});

	it('leaves an empty history alone', () => {
		const t = setup([], '');
		t.caretTo(0);
		t.key('ArrowUp');
		expect(t.value).toBe('');
		expect(t.nav.browsing).toBe(false);
	});

	it('recall() stashes the draft so ArrowDown still returns to it', () => {
		const t = setup(['old', 'new'], 'draft');
		t.nav.recall('old');
		expect(t.value).toBe('old');
		expect(t.nav.browsing).toBe(true);

		t.key('ArrowDown');
		expect(t.value).toBe('new');
		t.key('ArrowDown');
		expect(t.value).toBe('draft');
	});

	it('reset() drops the cursor without touching the value', () => {
		const t = setup(['old', 'new'], 'draft');
		t.caretTo(0);
		t.key('ArrowUp');
		t.nav.reset();
		expect(t.nav.browsing).toBe(false);
		expect(t.value).toBe('new');
	});
});
