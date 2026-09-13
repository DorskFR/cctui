import type { Interaction } from '@dorsk/journey';
import type { Overlay, Presenter, ShowCtx } from '@dorsk/journey/runtime';
import { m } from './paraglide/messages';

/** The runtime's toast speaks only for `press`; every other interaction draws a
 *  card whose sole control is Exit, which reads as a dead end. */
const HINTS: Partial<Record<Interaction['kind'], () => string>> = {
	click: () => m.journey_do_click(),
	dblclick: () => m.journey_do_click(),
	fill: () => m.journey_do_fill(),
	select: () => m.journey_do_select(),
	check: () => m.journey_do_check(),
	hover: () => m.journey_do_hover()
};

export function actionHint(ctx: Pick<ShowCtx, 'action' | 'next'>): string | undefined {
	if (ctx.next) return undefined;
	return HINTS[ctx.action.kind]?.();
}

/** `inner.show` hides the toast, so the hint has to be written after it. */
export function withActionHint(inner: Presenter, overlay: Overlay): Presenter {
	return {
		...inner,
		show(step, el, ctx) {
			inner.show(step, el, ctx);
			const hint = actionHint(ctx);
			const toast = overlay.parts.toast;
			if (hint === undefined) return;
			toast.textContent = hint;
			toast.hidden = false;
			overlay.layout();
		},
		settle: (step) => inner.settle(step),
		hide: () => inner.hide(),
		message: inner.message ? (...args) => inner.message?.(...args) : undefined,
		moveCursor: inner.moveCursor ? (el) => inner.moveCursor?.(el) ?? Promise.resolve() : undefined,
		ripple: inner.ripple ? (el) => inner.ripple?.(el) : undefined
	};
}
