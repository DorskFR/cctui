import { expect, test, type Page } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const SECRET = 'hunter2-xyz';
const SOURCE = { ...session(0), name: 'mask source', bucket: 'done' };

async function open(page: Page) {
	await stubApi(page, [SOURCE]);
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

const box = (page: Page) => page.locator('[data-journey="composer"] textarea').first();
const mirror = (page: Page) => page.locator('[data-journey="composer"] [data-tsu="TextareaMirror"]');
const masked = (page: Page) => mirror(page).locator('.masked');
const caret = (page: Page) =>
	box(page).evaluate((el: HTMLTextAreaElement) => [el.selectionStart, el.selectionEnd]);

test.describe('composer masks <redacted> spans while typing', () => {
	test.use({ viewport: { width: 1536, height: 900 } });

	test('a typed span shows masked, the field keeps the raw text', async ({ page }) => {
		await open(page);
		await box(page).click();
		await expect(mirror(page)).toHaveCount(0);

		const raw = `pw <redacted>${SECRET}</redacted> ok`;
		await page.keyboard.type(raw);

		await expect(box(page)).toHaveValue(raw);
		await expect(box(page)).toHaveAttribute('aria-label', 'Message');
		await expect(masked(page)).toHaveCount(1);
		await expect(masked(page)).toHaveText(SECRET);
		await expect(mirror(page)).toHaveAttribute('aria-hidden', 'true');
		expect(await masked(page).evaluate((el) => getComputedStyle(el).color)).toBe('rgba(0, 0, 0, 0)');
		expect(await box(page).evaluate((el) => getComputedStyle(el).color)).toBe('rgba(0, 0, 0, 0)');
		const plain = mirror(page).locator('span:not(.masked):not(.tail)');
		await expect(plain.first()).toHaveText('pw <redacted>');
	});

	test('an unclosed tag is not masked', async ({ page }) => {
		await open(page);
		await box(page).click();
		await page.keyboard.type(`x <redacted>${SECRET}`);
		await expect(masked(page)).toHaveCount(0);
	});

	test('caret, selection, paste and IME behave like a plain textarea', async ({ page, context }) => {
		await context.grantPermissions(['clipboard-read', 'clipboard-write']);
		await open(page);
		const raw = `a <redacted>${SECRET}</redacted> z`;
		await box(page).click();
		await page.keyboard.type(raw);

		await page.keyboard.press('End');
		for (let i = 0; i < 3; i++) await page.keyboard.press('ArrowLeft');
		expect(await caret(page)).toEqual([raw.length - 3, raw.length - 3]);

		await page.keyboard.press('Shift+Home');
		expect(await caret(page)).toEqual([0, raw.length - 3]);

		await page.keyboard.press('End');
		await page.evaluate(() => navigator.clipboard.writeText(' <redacted>pasted</redacted>'));
		await page.keyboard.press('Control+V');
		const pasted = `${raw} <redacted>pasted</redacted>`;
		await expect(box(page)).toHaveValue(pasted);
		await expect(masked(page)).toHaveCount(2);
		await expect(masked(page).nth(1)).toHaveText('pasted');

		const cdp = await context.newCDPSession(page);
		await cdp.send('Input.imeSetComposition', { text: 'に', selectionStart: 1, selectionEnd: 1 });
		await cdp.send('Input.insertText', { text: '日本' });
		await expect(box(page)).toHaveValue(`${pasted}日本`);
		await expect(masked(page)).toHaveCount(2);
	});

	test('a chip reveals its span in the field', async ({ page }) => {
		await open(page);
		await box(page).fill(`<redacted>${SECRET}</redacted> and <redacted>b2</redacted>`);
		const chips = page.locator('[data-journey="composer"] [data-testid="redacted-chip"]');
		await expect(masked(page)).toHaveCount(2);

		await chips.first().click();
		await expect(masked(page)).toHaveCount(1);
		await expect(masked(page)).toHaveText('b2');
		await expect(chips.first()).not.toContainText(SECRET);

		await chips.first().click();
		await expect(masked(page)).toHaveCount(2);
	});

	test('the sent text is the raw value, masked only by the existing send path', async ({ page }) => {
		const puts = await open(page);
		const raw = `ship it <redacted>${SECRET}</redacted> now`;
		await box(page).click();
		await page.keyboard.type(raw);
		await expect(masked(page)).toHaveCount(1);
		await expect(box(page)).toHaveValue(raw);

		await box(page).press('Enter');
		await expect(box(page)).toHaveValue('');
		await expect(mirror(page)).toHaveCount(0);

		await expect
			.poll(() => puts.some((p) => p.key.startsWith('cctui_history_')), { timeout: 5_000 })
			.toBe(true);
		const history = puts.filter((p) => p.key.startsWith('cctui_history_')).at(-1)!;
		expect(history.body).toContain('ship it [REDACTED:user] now');
		expect(history.body).not.toContain(SECRET);
	});
});
