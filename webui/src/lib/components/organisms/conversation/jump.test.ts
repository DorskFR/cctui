import { describe, expect, it, vi } from 'vitest';
import { createSeqJumper, type SeqJumpDeps } from './jump';

/** A stub transcript: `fetched` grows leftward as older pages land, and only
 *  the last `renderLimit` entries count as mounted. */
function stubScroller(opts: {
	all: number[];
	fetchedFrom: number;
	renderLimit: number;
	chunk?: number;
}) {
	const chunk = opts.chunk ?? 3;
	const state = {
		fetchedFrom: opts.fetchedFrom,
		renderLimit: opts.renderLimit,
		stuck: true,
		centered: null as number | null,
		flashed: [] as number[],
		fetches: 0,
		grows: 0
	};
	const fetched = () => opts.all.slice(state.fetchedFrom);
	const deps: SeqJumpDeps = {
		hasSeq: (seq) => fetched().includes(seq),
		isRendered: (seq) => {
			const list = fetched();
			const i = list.indexOf(seq);
			return i >= 0 && i >= list.length - state.renderLimit;
		},
		growRender: () => {
			state.grows++;
			state.renderLimit += chunk;
		},
		canFetchOlder: () => state.fetchedFrom > 0,
		fetchOlder: async () => {
			state.fetches++;
			state.fetchedFrom = Math.max(0, state.fetchedFrom - chunk);
		},
		centerOnSeq: (seq) => {
			if (!deps.isRendered(seq)) return false;
			state.centered = seq;
			state.flashed.push(seq);
			return true;
		},
		unstick: () => {
			state.stuck = false;
		},
		settle: async () => {}
	};
	return { state, deps };
}

describe('ensureSeqVisible', () => {
	it('scrolls straight to a seq already inside the rendered window', async () => {
		const { state, deps } = stubScroller({
			all: [1, 2, 3, 4, 5, 6],
			fetchedFrom: 0,
			renderLimit: 6
		});
		const { ensureSeqVisible } = createSeqJumper(deps);
		expect(await ensureSeqVisible(5)).toBe(true);
		expect(state.centered).toBe(5);
		expect(state.grows).toBe(0);
		expect(state.fetches).toBe(0);
	});

	it('grows the render window for a loaded-but-unrendered seq', async () => {
		const { state, deps } = stubScroller({
			all: [1, 2, 3, 4, 5, 6, 7, 8, 9],
			fetchedFrom: 0,
			renderLimit: 2
		});
		const { ensureSeqVisible } = createSeqJumper(deps);
		expect(await ensureSeqVisible(1)).toBe(true);
		expect(state.centered).toBe(1);
		expect(state.grows).toBeGreaterThan(0);
		expect(state.fetches).toBe(0);
	});

	it('fetches older pages for a seq above the fetched window, then centres and flashes', async () => {
		const { state, deps } = stubScroller({
			all: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
			fetchedFrom: 9,
			renderLimit: 3
		});
		const { ensureSeqVisible } = createSeqJumper(deps);
		expect(await ensureSeqVisible(2)).toBe(true);
		expect(state.fetches).toBeGreaterThan(0);
		expect(state.grows).toBeGreaterThan(0);
		expect(state.centered).toBe(2);
		expect(state.flashed).toEqual([2]);
	});

	it('unsticks the scroll controller before scrolling', async () => {
		const { state, deps } = stubScroller({ all: [1, 2, 3], fetchedFrom: 0, renderLimit: 3 });
		const { ensureSeqVisible } = createSeqJumper(deps);
		await ensureSeqVisible(2);
		expect(state.stuck).toBe(false);
	});

	it('gives up when the seq is unreachable rather than looping', async () => {
		const { state, deps } = stubScroller({ all: [5, 6, 7], fetchedFrom: 0, renderLimit: 3 });
		const { ensureSeqVisible } = createSeqJumper(deps);
		expect(await ensureSeqVisible(99)).toBe(false);
		expect(state.centered).toBe(null);
	});

	it('retries once when the line is not mounted on the first frame', async () => {
		let mounted = false;
		const deps: SeqJumpDeps = {
			hasSeq: () => true,
			isRendered: () => true,
			growRender: () => {},
			canFetchOlder: () => false,
			fetchOlder: async () => {},
			centerOnSeq: vi.fn(() => mounted),
			unstick: () => {},
			settle: async () => {
				mounted = true;
			}
		};
		const { ensureSeqVisible } = createSeqJumper(deps);
		expect(await ensureSeqVisible(3)).toBe(true);
		expect(deps.centerOnSeq).toHaveBeenCalledTimes(2);
	});
});
