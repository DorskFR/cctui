import { describe, expect, it } from 'vitest';
import { ScrollController } from './scroll.svelte';

// Mutations must go through `c.scroller`: the field is `$state`, and its proxy
// caches values read off the raw object.
type ScrollerStub = { scrollHeight: number; clientHeight: number; scrollTop: number };

function attach(c: ScrollController, init: ScrollerStub): ScrollerStub {
	c.scroller = { ...init } as unknown as HTMLElement;
	return c.scroller as unknown as ScrollerStub;
}

describe('ScrollController layout-induced scrolls', () => {
	it('re-pins when the viewport shrinks (brief strip expands) while stuck', () => {
		const c = new ScrollController();
		const el = attach(c, { scrollHeight: 2000, clientHeight: 800, scrollTop: 1200 });
		c.resetForSession();
		c.onScroll();
		expect(c.stuck).toBe(true);

		// The strip takes 200px of viewport height; the browser fires a scroll
		// event with the old scrollTop.
		el.clientHeight = 600;
		c.onScroll();
		expect(c.stuck).toBe(true);
		expect(el.scrollTop).toBe(el.scrollHeight);
	});

	it('a shrink cannot unstick even when the resulting position is far from the bottom', () => {
		const c = new ScrollController();
		const el = attach(c, { scrollHeight: 5000, clientHeight: 800, scrollTop: 4200 });
		c.resetForSession();
		c.onScroll();
		el.clientHeight = 400;
		el.scrollTop = 100;
		c.onScroll();
		expect(c.stuck).toBe(true);
	});

	it('collapsing the strip again (viewport grows back) keeps the bottom pin', () => {
		const c = new ScrollController();
		const el = attach(c, { scrollHeight: 2000, clientHeight: 600, scrollTop: 1400 });
		c.resetForSession();
		c.onScroll();
		el.clientHeight = 800;
		el.scrollTop = 1200;
		c.onScroll();
		expect(c.stuck).toBe(true);
	});

	it('a genuine user scroll-up still unsticks', () => {
		const c = new ScrollController();
		const el = attach(c, { scrollHeight: 2000, clientHeight: 800, scrollTop: 1200 });
		c.resetForSession();
		c.onScroll();
		c.markUserScroll();
		el.scrollTop = 100;
		c.onScroll();
		expect(c.stuck).toBe(false);
	});

	it('unstick() drops the bottom pin', () => {
		const c = new ScrollController();
		attach(c, { scrollHeight: 2000, clientHeight: 800, scrollTop: 1200 });
		c.unstick();
		expect(c.stuck).toBe(false);
	});
});

describe('ScrollController.centerOnSeq', () => {
	it('returns false when the seq is not mounted', () => {
		const c = new ScrollController();
		c.scroller = { querySelector: () => null } as unknown as HTMLElement;
		expect(c.centerOnSeq(42)).toBe(false);
	});

	it('centres the line, unsticks, and flashes it', () => {
		const classes = new Set<string>();
		const node = {
			offsetTop: 1000,
			clientHeight: 100,
			classList: {
				add: (c: string) => classes.add(c),
				remove: (c: string) => classes.delete(c)
			},
			offsetWidth: 0
		};
		const raw = {
			offsetTop: 0,
			clientHeight: 500,
			scrollTop: 0,
			scrollHeight: 4000,
			querySelector: (sel: string) => (sel === '[data-seq="42"]' ? node : null)
		} as unknown as HTMLElement;
		const c = new ScrollController();
		c.scroller = raw;
		const el = c.scroller as HTMLElement;
		expect(c.centerOnSeq(42)).toBe(true);
		expect(c.stuck).toBe(false);
		// 1000 - (500 - 100) / 2
		expect(el.scrollTop).toBe(800);
		expect(classes.has('line-focus')).toBe(true);
	});
});
