export type PickerKeyAction =
	| { type: 'open' }
	| { type: 'close' }
	| { type: 'select' }
	| { type: 'move'; index: number }
	| { type: 'change'; index: number }
	| { type: 'type'; char: string };

type Keyish = { key: string; altKey?: boolean; ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean };

const printable = (e: Keyish) => e.key.length === 1 && !e.altKey && e.key !== ' ';

/** What the profile picker does with a keydown on its trigger. Closed, the
 *  arrows and Home/End step the selection like a native select and
 *  Enter/Space/Alt+ArrowDown open; open, they move the highlighted row,
 *  Enter/Space pick it and Escape/Tab close. Printable keys feed the
 *  typeahead. Cmd/Ctrl chords are left alone for the form's launch shortcut. */
export function pickerKeyAction(
	e: Keyish,
	{ open, count, index }: { open: boolean; count: number; index: number }
): PickerKeyAction | undefined {
	const { key } = e;
	if (e.ctrlKey || e.metaKey) return undefined;
	const last = count - 1;
	const cur = index >= 0 && index <= last ? index : -1;
	if (!open) {
		if (e.altKey) return key === 'ArrowDown' ? { type: 'open' } : undefined;
		if (key === 'Enter' || key === ' ') return { type: 'open' };
		if (count <= 0) return undefined;
		if (key === 'ArrowDown') return { type: 'change', index: cur < 0 ? 0 : Math.min(cur + 1, last) };
		if (key === 'ArrowUp') return { type: 'change', index: cur < 0 ? last : Math.max(cur - 1, 0) };
		if (key === 'Home') return { type: 'change', index: 0 };
		if (key === 'End') return { type: 'change', index: last };
		return printable(e) ? { type: 'type', char: key } : undefined;
	}
	if (key === 'Escape' || key === 'Tab') return { type: 'close' };
	if (e.altKey) return key === 'ArrowUp' ? { type: 'select' } : undefined;
	if (key === 'Enter' || key === ' ') return cur >= 0 ? { type: 'select' } : { type: 'close' };
	if (count <= 0) return undefined;
	if (key === 'ArrowDown') return { type: 'move', index: cur < 0 ? 0 : Math.min(cur + 1, last) };
	if (key === 'ArrowUp') return { type: 'move', index: cur < 0 ? last : Math.max(cur - 1, 0) };
	if (key === 'Home') return { type: 'move', index: 0 };
	if (key === 'End') return { type: 'move', index: last };
	return printable(e) ? { type: 'type', char: key } : undefined;
}

/** First label starting with `query` (any case), searching after `from` and
 *  wrapping; a repeated single letter cycles through its matches. */
export function typeaheadIndex(labels: readonly string[], query: string, from: number): number | undefined {
	if (!query || !labels.length) return undefined;
	const q = query.toLocaleLowerCase();
	const repeat = [...q].every((c) => c === q[0]);
	const needle = repeat ? q[0] : q;
	const start = repeat || q.length === 1 ? from + 1 : Math.max(from, 0);
	for (let k = 0; k < labels.length; k++) {
		const i = (((start + k) % labels.length) + labels.length) % labels.length;
		if (labels[i].toLocaleLowerCase().startsWith(needle)) return i;
	}
	return undefined;
}
