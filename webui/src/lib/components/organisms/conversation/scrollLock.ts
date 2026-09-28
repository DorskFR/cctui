/** Freeze the document's own scroller, returning the undo. The gutter
 *  `scrollbar-gutter: stable` reserves stays reserved, so emptying it shifts
 *  nothing sideways; the previous inline value is restored verbatim rather than
 *  cleared, so a nested lock can't leak `hidden`. */
export function lockDocumentScroll(): () => void {
	const el = document.documentElement;
	const previous = el.style.overflowY;
	el.style.overflowY = 'hidden';
	return () => {
		el.style.overflowY = previous;
	};
}
