import { mount, unmount } from 'svelte';
import type { Presenter } from '@dorsk/journey/runtime';
import WelcomeDeck from './components/organisms/WelcomeDeck.svelte';

export interface DeckCard {
	title: string;
	body: string;
}

export interface DeckDeps {
	/** Every card of the run in progress, in step order. */
	cards: () => DeckCard[];
	/** Records the deck as seen for a user who leaves it part-way: the engine
	 *  clears progress on abort and only writes the done marker on completion. */
	markSeen: () => void;
	fallback: () => Presenter;
}

/** Draws target-less steps as a carousel deck and leaves anchored steps to the
 *  runtime's overlay card. The engine remains the state machine: the deck cannot
 *  move past the step the engine is on, it asks for the next one and redraws. */
export function deckPresenter(deps: DeckDeps): Presenter {
	const state = $state({ cards: [] as DeckCard[], reached: 0 });
	let deck: Record<string, unknown> | null = null;
	let next: (() => void) | null = null;
	let exit: (() => void) | null = null;
	let wanted = 0;

	function close() {
		if (!deck) return;
		unmount(deck, { outro: false });
		deck = null;
		next = null;
		exit = null;
		wanted = 0;
	}

	function open() {
		deck ??= mount(WelcomeDeck, {
			target: document.body,
			props: {
				get cards() {
					return state.cards;
				},
				get reached() {
					return state.reached;
				},
				onindex(index: number) {
					wanted = index;
					if (index > state.reached) next?.();
				},
				onfinish: () => next?.(),
				ondismiss() {
					deps.markSeen();
					exit?.();
				}
			}
		}) as Record<string, unknown>;
	}

	return {
		show(step, el, ctx) {
			if (step.target !== undefined) {
				close();
				deps.fallback().show(step, el, ctx);
				return;
			}
			deps.fallback().hide();
			state.cards = deps.cards();
			state.reached = ctx.index;
			next = ctx.next;
			exit = ctx.exit;
			open();
			if (wanted > ctx.index) ctx.next?.();
		},
		settle(step) {
			if (step.target !== undefined) return deps.fallback().settle(step);
		},
		hide() {
			close();
			deps.fallback().hide();
		},
		message(title, body, onexit, onnext, nextLabel) {
			deps.fallback().message?.(title, body, onexit, onnext, nextLabel);
		},
		moveCursor(el) {
			return deps.fallback().moveCursor?.(el) ?? Promise.resolve();
		},
		ripple(el) {
			deps.fallback().ripple?.(el);
		}
	};
}
