import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { IRStep } from '@dorsk/journey';
import type { Presenter, ShowCtx } from '@dorsk/journey/runtime';
import { type DeckCard, deckPresenter } from './welcomeDeck.svelte';

const CARDS: DeckCard[] = [
	{ title: 'Welcome to cctui', body: 'One control room.' },
	{ title: 'The session list', body: 'Grouped by what it needs.' },
	{ title: 'That is the tour', body: 'Guides live in settings.' }
];

function step(id: string, target?: string): IRStep {
	return { id, target, do: { kind: 'none' }, guide: 'next', timeout: 1000 } as IRStep;
}

function ctx(index: number, over: Partial<ShowCtx> = {}): ShowCtx {
	return {
		index,
		total: CARDS.length,
		action: { kind: 'none' },
		human: true,
		stepped: false,
		next: vi.fn(),
		exit: vi.fn(),
		...over
	} as ShowCtx;
}

function stubPresenter(): Presenter & { show: ReturnType<typeof vi.fn> } {
	return { show: vi.fn(), settle: vi.fn(), hide: vi.fn() } as never;
}

function deps(fallback: Presenter) {
	return { cards: () => CARDS, markSeen: vi.fn(), fallback: () => fallback };
}

function text() {
	return document.body.textContent ?? '';
}

beforeEach(() => {
	document.body.innerHTML = '';
});

describe('deckPresenter', () => {
	it('hands a step that names a target to the overlay presenter', () => {
		const fallback = stubPresenter();
		const presenter = deckPresenter(deps(fallback));
		const anchored = step('tiles', 'tiles');
		const c = ctx(0);
		presenter.show(anchored, null, c);
		flushSync();
		expect(fallback.show).toHaveBeenCalledWith(anchored, null, c);
		expect(text()).not.toContain('Welcome to cctui');
	});

	it('renders a target-less step as the whole deck and hides the overlay', () => {
		const fallback = stubPresenter();
		const presenter = deckPresenter(deps(fallback));
		presenter.show(step('what'), null, ctx(0));
		flushSync();
		expect(fallback.show).not.toHaveBeenCalled();
		expect(fallback.hide).toHaveBeenCalled();
		expect(text()).toContain('Welcome to cctui');
		expect(text()).toContain('One control room.');
	});

	it('advances the engine when the deck asks for a card past the one it is on', () => {
		const presenter = deckPresenter(deps(stubPresenter()));
		const next = vi.fn();
		presenter.show(step('what'), null, ctx(0, { next }));
		flushSync();
		const forward = document.body.querySelectorAll('button');
		expect(forward.length).toBeGreaterThan(0);
		document.body.querySelector<HTMLButtonElement>('[aria-label="Next slide"]')?.click();
		flushSync();
		expect(next).toHaveBeenCalled();
	});

	it('records the deck as seen when the user skips out of it', () => {
		const d = deps(stubPresenter());
		const presenter = deckPresenter(d);
		const exit = vi.fn();
		presenter.show(step('what'), null, ctx(0, { exit }));
		flushSync();
		const skip = [...document.body.querySelectorAll('button')].find(
			(b) => b.textContent?.trim() === 'Skip'
		);
		skip?.click();
		flushSync();
		expect(d.markSeen).toHaveBeenCalled();
		expect(exit).toHaveBeenCalled();
	});

	it('tears the deck down on hide', () => {
		const presenter = deckPresenter(deps(stubPresenter()));
		presenter.show(step('what'), null, ctx(0));
		flushSync();
		presenter.hide();
		flushSync();
		expect(text()).not.toContain('One control room.');
	});
});
