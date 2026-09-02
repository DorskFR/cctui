// Drag-to-resize for the panels docked to an edge of the Sessions screen.
// Attached to the thin grip on a panel's inner edge; pointer events cover the
// mouse and touch alike (the grip sets `touch-action: none` so the browser
// never turns the drag into a scroll). The panel keeps its width from the
// settings blob, so the action only reports widths: the caller stores them and
// the layout re-reserves the edge on the next frame.
import { DOCK_MAX_VIEWPORT_SHARE, DOCK_MIN_PX, type DockSide } from './dock';

export interface DockResizeOptions {
	/** Edge the panel is pinned to: decides which way a drag grows it. */
	side: DockSide;
	/** New width in px, at most once per frame while dragging. */
	onwidth: (px: number) => void;
	/** Double-click / double-tap on the grip: back to the default width. */
	onreset?: () => void;
	/** Drag started / ended, for the grip's own active styling. */
	ondrag?: (active: boolean) => void;
}

/** Widest a dragged panel may get on this viewport: a share of the window,
 *  never below the floor. */
export function maxDockWidth(viewportWidth: number): number {
	return Math.max(DOCK_MIN_PX, Math.floor(viewportWidth * DOCK_MAX_VIEWPORT_SHARE));
}

/** Width the grip's panel should take for a pointer that moved `dx` px since
 *  the drag started at `startWidth`. A right-docked panel grows leftwards. */
export function draggedWidth(side: DockSide, startWidth: number, dx: number, viewportWidth: number): number {
	const raw = side === 'left' ? startWidth + dx : startWidth - dx;
	return Math.min(maxDockWidth(viewportWidth), Math.max(DOCK_MIN_PX, Math.round(raw)));
}

export function dockResize(node: HTMLElement, opts: DockResizeOptions) {
	let options = opts;
	let dragging = false;
	let startX = 0;
	let startWidth = 0;
	let frame = 0;
	let pendingWidth: number | null = null;

	const panel = () => node.parentElement ?? node;

	function flush() {
		frame = 0;
		if (pendingWidth === null) return;
		const w = pendingWidth;
		pendingWidth = null;
		options.onwidth(w);
	}

	function onDown(e: PointerEvent) {
		if (e.button !== 0 && e.pointerType === 'mouse') return;
		dragging = true;
		startX = e.clientX;
		startWidth = panel().getBoundingClientRect().width;
		node.setPointerCapture(e.pointerId);
		options.ondrag?.(true);
		document.body.classList.add('dock-resizing');
		e.preventDefault();
	}

	function onMove(e: PointerEvent) {
		if (!dragging) return;
		pendingWidth = draggedWidth(options.side, startWidth, e.clientX - startX, window.innerWidth);
		if (!frame) frame = requestAnimationFrame(flush);
	}

	function onUp(e: PointerEvent) {
		if (!dragging) return;
		dragging = false;
		if (node.hasPointerCapture(e.pointerId)) node.releasePointerCapture(e.pointerId);
		options.ondrag?.(false);
		document.body.classList.remove('dock-resizing');
		if (frame) cancelAnimationFrame(frame);
		flush();
	}

	function onDblClick() {
		options.onreset?.();
	}

	node.addEventListener('pointerdown', onDown);
	node.addEventListener('pointermove', onMove);
	node.addEventListener('pointerup', onUp);
	node.addEventListener('pointercancel', onUp);
	node.addEventListener('dblclick', onDblClick);

	return {
		update(next: DockResizeOptions) {
			options = next;
		},
		destroy() {
			node.removeEventListener('pointerdown', onDown);
			node.removeEventListener('pointermove', onMove);
			node.removeEventListener('pointerup', onUp);
			node.removeEventListener('pointercancel', onUp);
			node.removeEventListener('dblclick', onDblClick);
			if (frame) cancelAnimationFrame(frame);
			document.body.classList.remove('dock-resizing');
		}
	};
}
