import { beforeEach, describe, expect, it, vi } from 'vitest';
import { SearchHitStepper } from './searchHits.svelte';

function scrollerWith(hitCount: number): HTMLElement {
	const el = document.createElement('div');
	el.innerHTML = Array.from(
		{ length: hitCount },
		(_, i) => `<div class="line"><mark class="search-hit">hit${i}</mark></div>`
	).join('');
	return el;
}

beforeEach(() => {
	// happy-dom has no scrollIntoView on elements by default.
	Element.prototype.scrollIntoView = vi.fn();
});

describe('SearchHitStepper', () => {
	it('counts the rendered hits and steps forward in order', () => {
		const el = scrollerWith(3);
		const s = new SearchHitStepper({ scroller: () => el, loadOlder: () => {} });

		s.refresh();
		expect(s.count).toBe(3);
		expect(s.index).toBe(-1);

		s.next();
		expect(s.index).toBe(0);
		s.next();
		s.next();
		expect(s.index).toBe(2);
		// Clamps at the newest hit rather than wrapping.
		s.next();
		expect(s.index).toBe(2);
	});

	it('marks only the current hit', () => {
		const el = scrollerWith(3);
		const s = new SearchHitStepper({ scroller: () => el, loadOlder: () => {} });
		s.next();
		s.next();
		const marked = [...el.querySelectorAll('mark.hit-current')];
		expect(marked).toHaveLength(1);
		expect(marked[0].textContent).toBe('hit1');
	});

	it('steps back and clamps at the oldest hit when nothing older loads', async () => {
		const el = scrollerWith(2);
		const loadOlder = vi.fn();
		const s = new SearchHitStepper({ scroller: () => el, loadOlder });

		s.next();
		await s.prev();
		expect(loadOlder).toHaveBeenCalledTimes(1);
		expect(s.index).toBe(0);
	});

	it('loads older lines and keeps stepping into them', async () => {
		let el = scrollerWith(2);
		const loadOlder = vi.fn(() => {
			el = scrollerWith(5);
		});
		const s = new SearchHitStepper({ scroller: () => el, loadOlder });

		s.next();
		expect(s.index).toBe(0);
		// At the oldest rendered hit: three older hits appear above it, so the
		// step lands on the newest of those, not back on hit 0.
		await s.prev();
		expect(loadOlder).toHaveBeenCalledTimes(1);
		expect(s.count).toBe(5);
		expect(s.index).toBe(2);
	});

	it('re-anchors the index on the same hit element when older lines prepend', () => {
		const el = document.createElement('div');
		el.innerHTML = '<mark class="search-hit">a</mark><mark class="search-hit">b</mark>';
		const s = new SearchHitStepper({ scroller: () => el, loadOlder: () => {} });
		s.next();
		s.next();
		expect(s.index).toBe(1);

		el.insertAdjacentHTML('afterbegin', '<mark class="search-hit">older</mark>');
		s.refresh();
		expect(s.count).toBe(3);
		expect(s.index).toBe(2);
	});

	it('does nothing when there are no hits', () => {
		const s = new SearchHitStepper({ scroller: () => undefined, loadOlder: () => {} });
		s.refresh();
		s.next();
		expect(s.count).toBe(0);
		expect(s.index).toBe(-1);
	});
});
