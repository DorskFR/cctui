import { type Locator, type Page } from '@playwright/test';

export const WIDE = { width: 1280, height: 900 };
export const NARROW = { width: 360, height: 800 };
export const TINY = { width: 320, height: 640 };

export async function openDrawer(page: Page, viewport: { width: number; height: number }) {
	await page.setViewportSize(viewport);
	await page.goto('/sessions');
	await page.locator('[data-journey="search"]').waitFor();
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await page.locator('[data-journey="composer"]').waitFor();
	await page.locator('[data-journey="header"]').waitFor();
}

export const pageOverflow = (page: Page) =>
	page.evaluate(() => {
		const el = document.scrollingElement as Element;
		return el.scrollWidth - window.innerWidth;
	});

export async function rects(scope: Locator) {
	const out: { label: string; x: number; y: number; width: number; height: number }[] = [];
	const n = await scope.count();
	for (let i = 0; i < n; i++) {
		const el = scope.nth(i);
		if (!(await el.isVisible())) continue;
		const box = await el.boundingBox();
		if (!box || box.width === 0 || box.height === 0) continue;
		const label = await el.evaluate((e) => e.getAttribute('aria-label') || e.className.toString());
		out.push({ label, ...box });
	}
	return out;
}
