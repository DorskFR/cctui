import { expect, test, type Page } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const SECRET = 'hunter2-xyz';
const SOURCE = { ...session(0), name: 'redact source', bucket: 'done' };

async function open(page: Page, conversation: unknown[] = []) {
	await stubApi(page, [SOURCE], { conversation: () => conversation });
	const puts: { key: string; body: string }[] = [];
	await page.route(
		(url) => url.pathname.startsWith('/api/v1/drafts/'),
		(route) => {
			const req = route.request();
			if (req.method() === 'PUT') {
				const key = decodeURIComponent(new URL(req.url()).pathname.split('/').pop() ?? '');
				puts.push({ key, body: req.postData() ?? '' });
			}
			return route.fulfill({ json: {} });
		}
	);
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
	return puts;
}

const message = (page: Page) => page.locator('[data-journey="composer"] textarea').first();
const chips = (page: Page) => page.locator('[data-journey="composer"] [data-testid="redacted-chip"]');

test.describe('redacted tags', () => {
	test.use({ viewport: { width: 1536, height: 900 } });

	test('a closed span shows as a masked chip that reveals locally on click', async ({ page }) => {
		await open(page);
		await message(page).fill(`login with <redacted>${SECRET}</redacted> please`);

		await expect(chips(page)).toHaveCount(1);
		await expect(chips(page)).toHaveText('••••••');
		await expect(chips(page)).not.toContainText(SECRET);

		await chips(page).click();
		await expect(chips(page)).toHaveText(SECRET);
		await chips(page).click();
		await expect(chips(page)).toHaveText('••••••');
	});

	test('Ctrl+Shift+R wraps the selection in tags', async ({ page }) => {
		await open(page);
		const box = message(page);
		await box.fill(`token ${SECRET} ok`);
		await box.evaluate((el: HTMLTextAreaElement, s) => {
			const at = el.value.indexOf(s);
			el.focus();
			el.setSelectionRange(at, at + s.length);
		}, SECRET);

		await page.keyboard.press('Control+Shift+R');

		await expect(box).toHaveValue(`token <redacted>${SECRET}</redacted> ok`);
		await expect(chips(page)).toHaveCount(1);
	});

	test('an unclosed tag warns inline until it is closed', async ({ page }) => {
		await open(page);
		const warning = page.locator('[data-journey="composer"] [data-testid="redacted-unclosed"]');
		await message(page).fill(`x <redacted>${SECRET}`);
		await expect(warning).toBeVisible();

		await message(page).fill(`x <redacted>${SECRET}</redacted>`);
		await expect(warning).toHaveCount(0);
	});

	test('the optimistic bubble, synced draft and history carry only the masked form', async ({ page }) => {
		const puts = await open(page);
		await message(page).fill(`deploy now <redacted>${SECRET}</redacted>`);
		await expect
			.poll(() => puts.some((p) => p.key.startsWith('cctui_draft_')), { timeout: 5_000 })
			.toBe(true);
		await message(page).press('Enter');

		const bubble = page.locator('.bubble.user', { hasText: 'deploy now' }).last();
		await expect(bubble).toBeVisible();
		await expect(bubble).not.toContainText(SECRET);
		await expect(bubble.locator('[data-redacted="user"]')).toHaveCount(1);

		await expect
			.poll(() => puts.some((p) => p.key.startsWith('cctui_history_')), { timeout: 5_000 })
			.toBe(true);
		for (const p of puts) {
			expect(p.body).not.toContain(SECRET);
			if (/^cctui_(draft|history)_/.test(p.key)) expect(p.body).toContain('[REDACTED:user]');
		}
	});

	test('the transcript renders [REDACTED:user] as a pill hidden by you', async ({ page }) => {
		const now = Date.now();
		await open(page, [
			{ type: 'text', content: '▷ User: my key is [REDACTED:user] thanks', meta: false, ts: now - 2000, seq: 1 },
			{ type: 'text', content: 'Got it, using [REDACTED:user].', meta: false, ts: now - 1000, seq: 2 }
		]);

		const pills = page.locator('.bubble [data-redacted="user"]');
		await expect(pills).toHaveCount(2);
		await expect(pills.first()).toHaveAttribute('title', 'hidden by you');
		await expect(page.locator('.bubble', { hasText: '[REDACTED:user]' })).toHaveCount(0);
	});
});
