/**
 * Svelte action: grow a <textarea> with its content up to its CSS max-height,
 * then scroll *inside* the textarea. Pass the bound value so it re-measures on
 * programmatic changes (drafts loading, clearing, etc.).
 *
 * Two things keep it well-behaved in a scrollable modal (CCT-405):
 *  - the explicit height is clamped to the element's computed `max-height`, so
 *    the field never grows past its cap and pushes the surrounding controls out
 *    of view;
 *  - the nearest scrollable ancestor's scrollTop is restored around the
 *    measure, so toggling `height: auto` doesn't make the modal jump to the
 *    caret on every keystroke.
 *
 * If the user has manually dragged the textarea taller (resize: vertical), that
 * larger height is respected — auto-grow only ever raises the height.
 *
 * Usage: <textarea use:autoresize={value} ...></textarea>
 */
function scrollParent(el: HTMLElement): HTMLElement | null {
	let p = el.parentElement;
	while (p) {
		const oy = getComputedStyle(p).overflowY;
		if ((oy === 'auto' || oy === 'scroll') && p.scrollHeight > p.clientHeight) return p;
		p = p.parentElement;
	}
	return null;
}

export function autoresize(node: HTMLTextAreaElement, _value?: string) {
	// Honour a height the user dragged to via resize: vertical — never shrink it.
	let userMin = 0;
	const remember = () => {
		userMin = node.offsetHeight;
	};

	const resize = () => {
		const sc = scrollParent(node);
		const prevTop = sc ? sc.scrollTop : 0;
		node.style.height = 'auto';
		const max = parseFloat(getComputedStyle(node).maxHeight);
		let h = node.scrollHeight;
		if (Number.isFinite(max)) h = Math.min(h, max);
		if (userMin) h = Math.max(h, userMin);
		node.style.height = `${h}px`;
		if (sc) sc.scrollTop = prevTop;
	};

	resize();
	node.addEventListener('input', resize);
	// A manual drag fires no input event — capture the new floor on pointer up.
	node.addEventListener('pointerup', remember);
	return {
		update() {
			// re-measure when the bound value changes from outside (e.g. cleared)
			resize();
		},
		destroy() {
			node.removeEventListener('input', resize);
			node.removeEventListener('pointerup', remember);
		}
	};
}
