import { expect, test, type Page } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';
import { waitForDrawerOpen } from './drawer.fixture';

const SESSIONS = [session(1), session(2), session(3)];
const [A, B] = SESSIONS;
const MOBILE = { width: 412, height: 900 };

const path = (page: Page) => new URL(page.url()).pathname;
const row = (page: Page, id: string) => page.locator(`[data-session-id="${id}"]`);
const drawerHeader = (page: Page) => page.locator('[data-journey="header"]');

async function openList(page: Page) {
	await stubApi(page, SESSIONS);
	await page.setViewportSize(MOBILE);
	await page.goto('/sessions?view=list');
	await row(page, A.id).waitFor();
	await expect.poll(() => path(page)).toBe('/sessions');
}

async function open(page: Page, id: string) {
	// The full-width mobile drawer covers the rows, so fire the row's handler.
	await row(page, id).dispatchEvent('click');
	await expect.poll(() => path(page)).toBe(`/sessions/${id}`);
	await waitForDrawerOpen(page);
}

test('Back from a session switched to from another lands on the list', async ({ page }) => {
	const errors = watch(page);
	await openList(page);
	await row(page, A.id).locator('[data-journey="title"]').click();
	await expect.poll(() => path(page)).toBe(`/sessions/${A.id}`);
	await waitForDrawerOpen(page);
	await open(page, B.id);

	await page.goBack();
	await expect.poll(() => path(page)).toBe('/sessions');
	await expect(drawerHeader(page)).toHaveCount(0);
	await expect(row(page, A.id)).toBeVisible();

	await page.goBack();
	await expect.poll(() => page.url()).toBe('about:blank');
	expect(errors).toEqual([]);
});

test('Back after a session closes does not reopen it', async ({ page }) => {
	await openList(page);
	await open(page, A.id);
	await drawerHeader(page).getByRole('button', { name: 'Back', exact: true }).click();
	await expect.poll(() => path(page)).toBe('/sessions');
	await expect(drawerHeader(page)).toHaveCount(0);

	await page.goBack();
	await expect.poll(() => page.url()).toBe('about:blank');
});

test('Forward then Back returns to the list, never the same id twice', async ({ page }) => {
	await openList(page);
	await open(page, A.id);
	await page.goBack();
	await expect.poll(() => path(page)).toBe('/sessions');
	await page.goForward();
	await expect.poll(() => path(page)).toBe(`/sessions/${A.id}`);
	await waitForDrawerOpen(page);

	await page.goBack();
	await expect.poll(() => path(page)).toBe('/sessions');
	await expect(drawerHeader(page)).toHaveCount(0);
	expect(await page.evaluate(() => history.length)).toBe(3);
});
