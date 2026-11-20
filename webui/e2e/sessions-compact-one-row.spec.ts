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
