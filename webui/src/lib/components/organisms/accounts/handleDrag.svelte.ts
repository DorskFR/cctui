import type { AccountPoolView } from '@bindings/AccountPoolView';
import { ACCOUNT_DRAG_MIME } from './pools.logic';
import { accountDrag, isTouchPointer, poolZoneAt } from './drag.svelte';

// Touch / pen: no HTML5 drag. A short hold on the handle arms the drag (a
// quick swipe still scrolls), then the finger carries the card and drops it
// on whichever pool zone is under it on release.
const HOLD_MS = 120;
const SLOP_PX = 8;

/** Drag-handle handlers for an account card: HTML5 drag for mouse, a
 * hold-to-lift pointer drag for touch and pen. */
export function accountHandleDrag(opts: {
	accountId: () => string;
	pool: () => AccountPoolView | null;
	pools: () => AccountPoolView[];
	onmovepool: () => ((pool: AccountPoolView | null) => void) | undefined;
}) {
	let touchDragging = $state(false);
	let holdTimer: ReturnType<typeof setTimeout> | undefined;
	let origin = { x: 0, y: 0 };

	function dragStart(e: DragEvent) {
		if (!e.dataTransfer) return;
		e.dataTransfer.setData(ACCOUNT_DRAG_MIME, opts.accountId());
		e.dataTransfer.effectAllowed = 'move';
		accountDrag.accountId = opts.accountId();
	}
	function dragEnd() {
		accountDrag.accountId = '';
	}
	function endTouchDrag() {
		clearTimeout(holdTimer);
		holdTimer = undefined;
		touchDragging = false;
		accountDrag.accountId = '';
		accountDrag.overId = '';
	}
	function pointerDown(e: PointerEvent) {
		if (!isTouchPointer(e)) return;
		e.preventDefault();
		try {
			(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		} catch {
			/* synthetic pointer: nothing to capture */
		}
		origin = { x: e.clientX, y: e.clientY };
		holdTimer = setTimeout(() => {
			touchDragging = true;
			accountDrag.accountId = opts.accountId();
			navigator.vibrate?.(10);
		}, HOLD_MS);
	}
	function pointerMove(e: PointerEvent) {
		if (!isTouchPointer(e)) return;
		if (touchDragging) {
			accountDrag.overId = poolZoneAt(e.clientX, e.clientY);
			return;
		}
		if (holdTimer && Math.hypot(e.clientX - origin.x, e.clientY - origin.y) > SLOP_PX) {
			clearTimeout(holdTimer);
			holdTimer = undefined;
		}
	}
	function pointerUp(e: PointerEvent) {
		if (!isTouchPointer(e)) return;
		const was = touchDragging;
		const target = was ? poolZoneAt(e.clientX, e.clientY) : '';
		endTouchDrag();
		if (!was) return;
		const to = opts.pools().find((p) => p.id === target);
		if (to && to.id !== opts.pool()?.id) opts.onmovepool()?.(to);
	}
	function pointerCancel(e: PointerEvent) {
		if (!isTouchPointer(e)) return;
		endTouchDrag();
	}

	return {
		get touchDragging() {
			return touchDragging;
		},
		dragStart,
		dragEnd,
		pointerDown,
		pointerMove,
		pointerUp,
		pointerCancel
	};
}
