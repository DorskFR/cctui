import { expect, test } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

const LEVELS = { smallest: 0.8, normal: 1, large: 1.2, largest: 1.5 } as const;
const MIN_EM = 1.85;
const OUTSIDE = 2;

const parent = { ...session(1), bucket: 'working', name: 'subagent parent' };
const kids = [2, 3, 4, 5, 6].map((i) => ({ ...session(i), parent_id: parent.id, bucket: 'working' }));

for (const [level, scale] of Object.entries(LEVELS)) {
	for (const view of ['list', 'grid'] as const) {
		test(`subagent badge scales with ${level} text and catches near-misses in the ${view} view`, async ({
			page
		}) => {
			const errors = watch(page);
			await page.addInitScript((l) => localStorage.setItem('tsumikit-font-scale', l), level);
			await stubApi(page, [parent, ...kids], { settings: { display: { fontScale: scale } } });
			await page.setViewportSize({ width: 1400, height: 900 });
			await page.goto(`/sessions?view=${view}`);
			await page.locator('[data-journey="session"]').first().waitFor();
			const badge = page.locator('.subagent-badge [data-tsu="Badge"]').first();
			await badge.waitFor();

			const r = await badge.evaluate(
				(b, { minEm, outside }) => {
					const box = b.getBoundingClientRect();
					const font = parseFloat(getComputedStyle(b).fontSize);
					const text = [...b.childNodes]
						.filter((n) => n.nodeType === Node.TEXT_NODE && n.textContent?.trim())
						.flatMap((n) => {
							const range = document.createRange();
							range.selectNodeContents(n);
							return [...range.getClientRects()];
						});
					const cx = box.left + box.width / 2;
					const cy = box.top + box.height / 2;
					const probes = {
						left: [box.left - outside, cy],
						right: [box.right + outside, cy],
						above: [cx, box.top - outside],
						below: [cx, box.bottom + outside]
					} as const;
					const misses = Object.entries(probes)
						.filter(([, [x, y]]) => {
							const el = document.elementFromPoint(x, y);
							return !el || !(el === b || b.contains(el));
						})
						.map(([k]) => k);
					return {
						scale: getComputedStyle(document.documentElement).getPropertyValue('--fs-scale').trim(),
						w: box.width,
						h: box.height,
						min: font * minEm - 0.5,
						digits: text.length,
						overflow: text.filter(
							(t) => t.left < box.left || t.right > box.right || t.top < box.top || t.bottom > box.bottom
						).length,
						misses
					};
				},
				{ minEm: MIN_EM, outside: OUTSIDE }
			);

			expect(r.scale).toBe(String(scale));
			expect(r.w).toBeGreaterThanOrEqual(r.min);
			expect(r.h).toBeGreaterThanOrEqual(r.min);
			expect(r.digits).toBeGreaterThan(0);
			expect(r.overflow).toBe(0);
			expect(r.misses).toEqual([]);

			const before = await badge.getAttribute('aria-expanded');
			const b = (await badge.boundingBox())!;
			await page.mouse.click(b.x - OUTSIDE, b.y + b.height / 2);
			await expect(badge).toHaveAttribute('aria-expanded', before === 'true' ? 'false' : 'true');
			expect(new URL(page.url()).pathname).toBe('/sessions');
			expect(errors).toEqual([]);
		});
	}
}
