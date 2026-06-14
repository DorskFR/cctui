// Close a popover/menu when a pointer or focus lands outside the node it's
// attached to. Shared by the toolbar filter molecules (SectionFilter,
// LabelFilter) so each doesn't re-implement the same listener.
export function clickOutside(node: HTMLElement, onOutside: () => void) {
	const handler = (e: Event) => {
		if (!node.contains(e.target as Node)) onOutside();
	};
	document.addEventListener('pointerdown', handler, true);
	return {
		destroy() {
			document.removeEventListener('pointerdown', handler, true);
		}
	};
}
