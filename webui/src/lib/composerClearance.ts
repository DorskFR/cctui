import type { Attachment } from 'svelte/attachments';

const clearances = new Map<Element, number>();

function publish(): void {
	const max = Math.max(0, ...clearances.values());
	const root = document.documentElement.style;
	if (max > 0) root.setProperty('--composer-clear', `${max}px`);
	else root.removeProperty('--composer-clear');
}

/** How far a composer reaches up from the viewport bottom; one in the upper
 *  half of the screen (a top tile) does not hold the toasts up. */
export function clearanceOf(rect: Pick<DOMRect, 'top' | 'bottom' | 'height'>, viewportH: number): number {
	if (rect.height <= 0 || rect.bottom < viewportH / 2) return 0;
	return Math.max(0, Math.round(viewportH - rect.top));
}

/** Publishes `--composer-clear` on the document element: the largest clearance
 *  among mounted composers, so the toast stack can sit above them. */
export const composerClearance: Attachment<HTMLElement> = (el) => {
	const measure = () => {
		clearances.set(el, clearanceOf(el.getBoundingClientRect(), innerHeight));
		publish();
	};
	measure();
	const ro = new ResizeObserver(measure);
	ro.observe(el);
	const events = ['resize', 'transitionend', 'animationend'] as const;
	for (const ev of events) addEventListener(ev, measure);
	return () => {
		ro.disconnect();
		for (const ev of events) removeEventListener(ev, measure);
		clearances.delete(el);
		publish();
	};
};
