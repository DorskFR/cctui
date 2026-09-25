function ownSeparator(node: HTMLElement): Element | null {
	for (let a = node.parentElement; a; a = a.parentElement) {
		for (const c of a.children) if (c.getAttribute('role') === 'separator') return c;
	}
	return null;
}

// Pins `node` to its current pixel width while a pointer drags a resize
// separator of an ancestor panel, so the content relayouts once on release
// instead of at every intermediate width.
export function freezeWidthDuringResize(node: HTMLElement): { destroy: () => void } {
	let frozen = false;
	const release = () => {
		if (!frozen) return;
		frozen = false;
		node.style.width = '';
		node.style.flex = '';
		removeEventListener('pointerup', release, true);
		removeEventListener('pointercancel', release, true);
	};
	const onDown = (e: PointerEvent) => {
		const sep = (e.target as Element | null)?.closest?.('[role="separator"]');
		if (!sep || sep !== ownSeparator(node) || frozen) return;
		frozen = true;
		node.style.width = `${node.getBoundingClientRect().width}px`;
		node.style.flex = 'none';
		addEventListener('pointerup', release, true);
		addEventListener('pointercancel', release, true);
	};
	document.addEventListener('pointerdown', onDown, true);
	return {
		destroy() {
			release();
			document.removeEventListener('pointerdown', onDown, true);
		}
	};
}
