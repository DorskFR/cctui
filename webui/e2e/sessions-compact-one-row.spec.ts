import { expect, test } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

function loaded(i: number) {
	return {
		...session(i),
		name: 'hmm server deployment infra rollout and follow-up checks',
		model: 'claude-opus-5-5',
		effort: 'high',
		last_message_text: 'The deployment is rolled out and the endpoints answer on every node.',
		last_message_at: new Date().toISOString(),
		token_usage: {
			tokens_in: 320,
			tokens_out: 99_000,
			cost_usd: 0,
			cache_read_tokens: 57_900_000,
			cache_creation_tokens: 1_700_000
		}
	};
}

const sessions = [loaded(1), loaded(2), loaded(3)];

for (const width of [472, 400, 320]) {
	test(`a compact session card stays on one row at ${width}px`, async ({ page }) => {
		const errors = watch(page);
		await stubApi(page, sessions, { settings: { sessionList: { density: 'compact' } } });
		await page.setViewportSize({ width: 1000, height: 900 });
		await page.goto('/sessions?view=list');
		await page.locator('[data-journey="session"]').first().waitFor();

		const cards = await page.evaluate((w) => {
			for (const wrap of document.querySelectorAll<HTMLElement>('.sc-wrap.compact')) wrap.style.width = `${w}px`;
			const shown = (el: Element | null) => {
				if (!el) return null;
				const b = el.getBoundingClientRect();
				return b.width > 0 && b.height > 0 ? b : null;
			};
			return [...document.querySelectorAll<HTMLElement>('.sc-wrap.compact [data-journey="session"]')].map((card) => {
				const box = card.getBoundingClientRect();
				const edge = box.right - parseFloat(getComputedStyle(card).paddingRight) + 1;
				const parts = ['.l1', '.l2', '.m1']
					.map((sel) => shown(card.querySelector(sel)))
					.filter((b): b is DOMRect => b !== null)
					.map((b) => Math.round(b.top + b.height / 2));
				const title = shown(card.querySelector('.title'));
				const time = shown(card.querySelector('.time'));
				const spill = [...card.querySelectorAll('*')]
					.filter((el) => {
						if (el.getAttribute('aria-hidden') === 'true' && getComputedStyle(el).position === 'absolute')
							return false;
						const b = el.getBoundingClientRect();
						return b.width > 0 && b.right > edge;
					})
					.map((el) => el.className || el.tagName);
				return {
					width: Math.round(card.closest('.sc-wrap')?.getBoundingClientRect().width ?? 0),
					rowSpread: Math.max(...parts) - Math.min(...parts),
					titleWidth: Math.round(title?.width ?? 0),
					timeInside: !!time && time.right <= edge,
					spill
				};
			});
		}, width);

		expect(cards.length).toBe(sessions.length);
		for (const c of cards) {
			expect(c.width).toBe(width);
			expect(c.rowSpread).toBeLessThanOrEqual(4);
			expect(c.titleWidth).toBeGreaterThanOrEqual(60);
			expect(c.timeInside).toBe(true);
			expect(c.spill).toEqual([]);
		}
		expect(errors).toEqual([]);
	});
}

function labelled(i: number) {
	return {
		...loaded(i),
		machine_name: 'dev1',
		account_name: 'pumpkin',
		labels: [
			{ id: 'l1', name: 'cctui', color: '#a855f7' },
			{ id: 'l2', name: 'design-wave', color: '#22c55e' }
		]
	};
}
const parent = labelled(1);
const withSubagents = [
	parent,
	...Array.from({ length: 22 }, (_, k) => ({ ...session(100 + k), parent_id: parent.id, name: `agent ${k}` }))
];

for (const width of [1148, 900, 700, 560, 372]) {
	test(`a compact row keeps its label chips whole beside the next item at ${width}px`, async ({ page }) => {
		const errors = watch(page);
		await stubApi(page, withSubagents, { settings: { sessionList: { density: 'compact' } } });
		await page.setViewportSize({ width: 1600, height: 900 });
		await page.goto('/sessions?view=list');
		await page.locator('[data-journey="session"] .l1 .labels').first().waitFor();

		const row = await page.evaluate((w) => {
			for (const wrap of document.querySelectorAll<HTMLElement>('.sc-wrap.compact')) wrap.style.width = `${w}px`;
			const card = document.querySelector<HTMLElement>('[data-journey="session"]:has(.l1 .labels)')!;
			const box = (el: Element) => el.getBoundingClientRect();
			const shown = (b: DOMRect) => b.width > 0 && b.height > 0;
			const labels = card.querySelector<HTMLElement>('.l1 > .labels')!;
			const chips = [...labels.querySelectorAll('.name, .dots')].map(box).filter(shown);
			const next = [...card.querySelectorAll('.l2 .who > *, .l2 > :not(.who), .m1 > *')].map(box).filter(shown);
			const lb = box(labels);
			const meets = (a: DOMRect, b: DOMRect) =>
				a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom && b.top < a.bottom;
			return {
				chips: chips.length,
				clipped: labels.scrollWidth > labels.clientWidth + 1 || chips.some((c) => c.right > lb.right + 0.5),
				overlaps: next.filter((b) => meets(lb, b)).length,
				title: Math.round(box(card.querySelector('.title')!).width)
			};
		}, width);

		expect(row.chips).toBeGreaterThan(0);
		expect(row.clipped).toBe(false);
		expect(row.overlaps).toBe(0);
		expect(row.title).toBeGreaterThanOrEqual(width >= 1100 ? 200 : 60);
		expect(errors).toEqual([]);
	});
}
