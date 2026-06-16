// Swallow the spurious backdrop "click" that closes a tsumikit Modal when a
// text-selection drag starts inside the sheet and the pointer is released over
// the dialog's empty backdrop (e.g. selecting text in the spawn prompt field
// and overshooting the modal). The browser fires a `click` on the nearest
// common ancestor of mousedown/mouseup — which is the <dialog> backdrop — and
// tsumikit's own handler reads that as "clicked outside, close me".
//
// We only let the close through when the gesture also *started* on the backdrop
// (a genuine outside click). The action attaches to any node inside the dialog;
// it finds the dialog itself and guards its click handler in the capture phase
// so tsumikit's bubble-phase handler never runs for these stray clicks.
export function dialogBackdropGuard(node: HTMLElement) {
	const dialog = node.closest('dialog');
	if (!dialog) return;

	let downTarget: EventTarget | null = null;
	const onDown = (e: PointerEvent) => {
		downTarget = e.target;
	};
	const onClick = (e: MouseEvent) => {
		// A real backdrop click has both press and release on the dialog itself.
		// If the press landed on inner content (a drag-select), keep the modal open.
		if (e.target === dialog && downTarget && downTarget !== dialog) {
			e.stopImmediatePropagation();
		}
	};

	document.addEventListener('pointerdown', onDown, true);
	dialog.addEventListener('click', onClick, true);

	return {
		destroy() {
			document.removeEventListener('pointerdown', onDown, true);
			dialog.removeEventListener('click', onClick, true);
		}
	};
}
