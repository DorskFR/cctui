import { expect, test } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

// The text-size control grows only the --fs-* tokens, so every breakpoint on
// the sessions page has to follow the text, not the viewport in rem.
const LARGEST = 1.5;

function loaded(i: number, bucket: string) {
	return {
		...session(i),
		bucket,
		name: 'yubisashi session uncommitted diff',
		working_dir: '/home/dorsk/Documents/cctui',
		git_branch: 'main',
		model: 'claude-opus-5-5',
		effort: 'medium',
		last_message_text: 'The toolbar bug is fixed and tested, the fix lives on a branch of its own.',
		last_message_at: new Date().toISOString(),
		token_usage: {
			tokens_in: 320,
			tokens_out: 99_000,
			cost_usd: 0,
			cache_read_tokens: 29_900_000,
			cache_creation_tokens: 1_500_000
		}
	};
}

const sessions = [loaded(1, 'working'), loaded(2, 'blocked'), loaded(3, 'done')];

for (const view of ['grid', 'list'] as const) {
	for (const width of [412, 520, 600]) {
		test(`largest text keeps the ${view} view inside its cards at ${width}px`, async ({ page }) => {
			const errors = watch(page);
			await page.addInitScript(() => localStorage.setItem('tsumikit-font-scale', 'largest'));
			await stubApi(page, sessions, { settings: { display: { fontScale: LARGEST } } });
			await page.setViewportSize({ width, height: 900 });
			await page.goto(`/sessions?view=${view}`);
			await page.locator('[data-journey="session"]').first().waitFor();

			const report = await page.evaluate(() => {
				const lines = (el: Element) => {
					const r = document.createRange();
					r.selectNodeContents(el);
					return new Set([...r.getClientRects()].map((b) => Math.round(b.top))).size;
				};
				const wrapped = [...document.querySelectorAll('.ssh .sh-title, .ssh .sh-count')]
					.filter((el) => lines(el) > 1)
					.map((el) => el.textContent);
				const spill = [...document.querySelectorAll<HTMLElement>('[data-journey="session"]')].flatMap((card) => {
					const edge = card.getBoundingClientRect().right - parseFloat(getComputedStyle(card).paddingRight) + 1;
					return [...card.querySelectorAll('*')]
						.filter((el) => {
							const b = el.getBoundingClientRect();
							return b.width > 0 && b.right > edge;
						})
						.map((el) => el.className || el.tagName);
				});
				return {
					scale: getComputedStyle(document.documentElement).getPropertyValue('--fs-scale').trim(),
					wrapped,
					spill
				};
			});

			expect(report.scale).toBe(String(LARGEST));
			expect(report.wrapped).toEqual([]);
			expect(report.spill).toEqual([]);
			expect(errors).toEqual([]);
		});
	}
}
