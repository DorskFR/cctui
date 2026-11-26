import { MediaQuery } from 'svelte/reactivity';

/** A phone with its keyboard up: the layout viewport shrinks with it. */
const short = new MediaQuery('(max-width: 959px) and (max-height: 640px)');

const COMPOSER = '[data-journey="composer"]';
const PANE = '[data-journey="conversation"]';

/** Tracks focus inside this pane's composer, so the chrome above the
 *  transcript can fold away while the user types on a short viewport. */
export class ComposerFocus {
	focused = $state(false);

	get collapse(): boolean {
		return this.focused && short.current;
	}

	/** Listens on the pane holding `node`; returns the cleanup. */
	attach = (node: HTMLElement): (() => void) => {
		const root: HTMLElement | Document = node.closest<HTMLElement>(PANE) ?? document;
		const inComposer = (t: EventTarget | null) =>
			t instanceof Element && !!t.closest(COMPOSER) && root.contains(t);
		// Removing a focused node fires focusout synchronously, inside Svelte's
		// own update, where writing state throws: defer the write.
		const set = (v: boolean) => queueMicrotask(() => (this.focused = v));
		const onIn = (e: Event) => set(inComposer(e.target));
		const onOut = (e: FocusEvent) => {
			if (!inComposer(e.relatedTarget)) set(false);
		};
		root.addEventListener('focusin', onIn);
		root.addEventListener('focusout', onOut as EventListener);
		this.focused = inComposer(document.activeElement);
		return () => {
			root.removeEventListener('focusin', onIn);
			root.removeEventListener('focusout', onOut as EventListener);
		};
	};
}
