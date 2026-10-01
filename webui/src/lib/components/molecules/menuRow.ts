// Row chrome for the sessions ⋯ menu. Inline strings, not a class: its rows are
// three different controls in three components, and a style prop is the only way
// to reach a child component's root without `:global`.
const BASE = [
	'width: 100%',
	'justify-content: flex-start',
	'gap: var(--sp-2)',
	'min-height: 2.25rem',
	'padding: var(--sp-1) var(--sp-2)',
	'border-radius: var(--r-sm)',
	'font-size: var(--fs-sm)'
].join('; ');

export const MENU_ROW = BASE;

/** Leading icon column, so every row's label starts at the same edge. */
export const MENU_ROW_ICON =
	'display: inline-flex; align-items: center; justify-content: center; flex: none; width: 1.125rem';
