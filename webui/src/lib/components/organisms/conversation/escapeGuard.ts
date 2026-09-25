/** Nested dialogs and the rename input take Escape for themselves; keep it
 *  from reaching the panel's document-level close handler. */
export function guardEscape(e: KeyboardEvent): void {
	if (e.key !== 'Escape') return;
	const target = e.target as Element | null;
	const own = (e.currentTarget as Element).closest('[role="dialog"]');
	if (target instanceof HTMLInputElement || target?.closest('[role="dialog"]') !== own) {
		e.preventDefault();
	}
}
