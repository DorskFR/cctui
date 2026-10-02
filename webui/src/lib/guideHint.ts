import type { IR, Interaction } from '@dorsk/journey';
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

/** A guide step always offers Next. A fill would end on the first keystroke, so a
 *  guide only points at the field and lets the user type; the book still fills. */
export function guided(ir: IR): IR {
	return {
		...ir,
		steps: ir.steps.map((step) =>
			step.do.kind === 'fill' ? { ...step, do: { kind: 'none' }, guide: 'next' } : { ...step, guide: 'next' }
		)
	};
}

/** The runtime ignores Enter while focus is in a field, which strands a step that
 *  points at one: typing is done, and Next may sit behind a modal. Enter in the
 *  step's own single-line field is the user saying so. */
export function withFieldEnter(inner: Presenter): Presenter {
	let detach: (() => void) | null = null;
	const off = () => {
		detach?.();
		detach = null;
	};
	return {
		show(step, el, ctx) {
			off();
			inner.show(step, el, ctx);
			const next = ctx.next;
			if (!el || !next || !ctx.human) return;
			const onKey = (e: KeyboardEvent) => {
				if (e.key !== 'Enter' || e.isComposing || e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
				const t = e.target;
				if (!(t instanceof HTMLInputElement) || !(t === el || el.contains(t))) return;
				e.preventDefault();
				e.stopPropagation();
				off();
				next();
			};
			window.addEventListener('keydown', onKey, true);
			detach = () => window.removeEventListener('keydown', onKey, true);
		},
		settle: (step) => inner.settle(step),
		hide() {
			off();
			inner.hide();
		},
		message: inner.message ? (...args) => inner.message?.(...args) : undefined,
		moveCursor: inner.moveCursor ? (el) => inner.moveCursor?.(el) ?? Promise.resolve() : undefined,
		ripple: inner.ripple ? (el) => inner.ripple?.(el) : undefined
	};
}
