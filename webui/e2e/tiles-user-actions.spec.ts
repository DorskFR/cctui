import { expect, test, type Page } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { session, stubApi, watch } from './tiles.fixture';

const S = session(1);
const KINDS = ['action', 'decision', 'input'] as const;

const stamp = (i: number) => new Date(Date.UTC(2026, 9, 1, 10, i)).toISOString();

const ACTIONS = Array.from({ length: 30 }, (_, i) => {
	const status = i < 12 ? 'open' : i < 24 ? 'done' : 'dropped';
	return {
		id: `ua-${i}`,
		title:
			i % 4 === 0
				? `Rotate the staging deploy token and paste the new value into the vault entry (${i})`
				: `Review item ${i}`,
		detail: i % 3 === 0 ? `Steps:\n\n1. Open **settings**\n2. Copy \`token-${i}\`\n3. Confirm` : null,
		kind: KINDS[i % 3],
		blocking: status === 'open' && i % 5 === 0,
		status,
		note: status === 'open' ? null : `resolved ${i}`,
		created_at: stamp(i),
		resolved_at: status === 'open' ? null : stamp(60 + i),
		resolved_by: status === 'open' ? null : 'user'
	};
});

const OPEN = ACTIONS.filter((a) => a.status === 'open');

async function open(page: Page, width: number) {
	await stubApi(page, [S], {
		conversation: () => [{ type: 'text', content: 'Working on it.', meta: false, ts: Date.now(), seq: 0 }]
	});
	await page.route('**/api/v1/sessions/*/user-actions', (route) =>
		route.fulfill({ json: { session_id: S.id, items: ACTIONS } })
	);
	await page.setViewportSize({ width, height: 900 });
	await page.goto(`/sessions/${S.id}`);
	const card = page.locator('[data-tsu="Card"]').filter({ hasText: 'Needs you' });
	await card.waitFor();
	await card.scrollIntoViewIfNeeded();
	return card;
}

for (const [label, width] of [
	['desktop', 1400],
	['mobile', 390]
] as const) {
	test(`user-actions card at ${label} width: left-aligned, compact, resolved items folded`, async ({ page }) => {
		const errors = watch(page);
		const card = await open(page, width);

		const shots = process.env.UA_SHOTS;
		if (shots) {
			mkdirSync(shots, { recursive: true });
			await page.screenshot({ path: join(shots, `${label}.png`) });
			await card.screenshot({ path: join(shots, `${label}-card.png`) });
		}

		await expect(card.locator('[data-ua-row]')).toHaveCount(8);
		await expect(card.getByRole('button', { name: `Show ${OPEN.length - 8} more` })).toBeVisible();
		await expect(card.locator('[data-ua-row][data-blocking]')).toHaveCount(
			OPEN.filter((a) => a.blocking).length
		);
		await expect(card).not.toContainText(/\b(do|decide|tell)\b/);
		for (const d of await card.locator('.ua-detail').all()) await expect(d).toBeHidden();
		await expect(card.getByText('Review item 13')).toBeHidden();
		await expect(card.getByRole('button', { name: /12 done · 6 dropped/ })).toHaveAttribute(
			'aria-expanded',
			'false'
		);

		const box = (await card.boundingBox())!;
		const lefts = await card
			.locator('[data-ua-row] [data-ua-title]')
			.evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().left)));
		expect(new Set(lefts).size).toBe(1);
		expect(lefts[0] - box.x).toBeLessThan(64);
		const align = await card
			.locator('[data-ua-title]')
			.first()
			.evaluate((e) => getComputedStyle(e).textAlign);
		expect(['start', 'left']).toContain(align);

		await card.getByRole('button', { name: /12 done · 6 dropped/ }).click();
		await expect(card.getByText('Review item 13')).toBeVisible();
		expect(errors).toEqual([]);
	});
}
