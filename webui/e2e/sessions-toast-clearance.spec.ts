import { expect, test, type Page } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';
import { waitForDrawerOpen } from './drawer.fixture';

const SESSIONS = [session(1), session(2)];
const MOBILE = { width: 412, height: 915 };
const toast = (page: Page) => page.locator('[data-tsu="Toaster"] .toast');

async function raise(page: Page, message: string, ctor: 'Error' | 'TypeError' = 'Error') {
	await page.evaluate(
		([msg, kind]) => {
			const error = kind === 'TypeError' ? new TypeError(msg) : new Error(msg);
			window.dispatchEvent(new ErrorEvent('error', { error, message: `${kind}: ${msg}` }));
		},
		[message, ctor] as const
	);
}

async function toastSettled(page: Page) {
	await expect(toast(page).first()).toBeVisible();
	let last = '';
	await expect
		.poll(async () => {
			const box = JSON.stringify(await toast(page).first().boundingBox());
			const same = box === last;
			last = box;
			return same;
		})
		.toBe(true);
}

const hitsAtCentre = (page: Page, selector: string) =>
	page.evaluate((sel) => {
		return [...document.querySelectorAll<HTMLElement>(sel)]
			.filter((el) => el.getClientRects().length > 0)
			.map((el) => {
				const r = el.getBoundingClientRect();
				const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
				return {
					target: !!hit && el.contains(hit),
					toast: !!hit?.closest('[data-tsu="Toaster"]')
				};
			});
	}, selector);

async function openList(page: Page) {
	await stubApi(page, SESSIONS);
	await page.setViewportSize(MOBILE);
	await page.goto('/sessions?view=list');
	await page.locator(`[data-session-id="${SESSIONS[0].id}"]`).waitFor();
}

test('an error toast leaves the bottom nav tappable', async ({ page }) => {
	await openList(page);
	await raise(page, 'boom');
	await toastSettled(page);

	const hits = await hitsAtCentre(page, 'nav [data-journey="nav"]');
	expect(hits.length).toBeGreaterThan(0);
	for (const h of hits) expect(h).toEqual({ target: true, toast: false });
});

test('an error toast leaves the composer Send button tappable', async ({ page }) => {
	await openList(page);
	await page.locator(`[data-session-id="${SESSIONS[0].id}"]`).dispatchEvent('click');
	await waitForDrawerOpen(page);
	await raise(page, 'boom');
	await toastSettled(page);

	const send = page
		.locator('[data-journey="composer"]')
		.getByRole('button', { name: 'Send', exact: true });
	await expect(send.first()).toBeVisible();
	const hits = await hitsAtCentre(page, '[data-journey="composer"] button');
	const sendHit = await send.first().evaluate((el) => {
		const r = el.getBoundingClientRect();
		const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
		return { target: !!hit && el.contains(hit), toast: !!hit?.closest('[data-tsu="Toaster"]') };
	});
	expect(sendHit).toEqual({ target: true, toast: false });
	for (const h of hits) expect(h.toast).toBe(false);

	const composerTop = (await page.locator('[data-journey="composer"]').boundingBox())!.y;
	const toastBox = (await toast(page).first().boundingBox())!;
	expect(toastBox.y + toastBox.height).toBeLessThanOrEqual(composerTop + 1);
});

test('the router scroll capture error is not toasted', async ({ page }) => {
	await openList(page);
	await raise(page, `can't access property "scroll", F[e] is undefined`, 'TypeError');
	await raise(page, 'boom');
	await toastSettled(page);
	await expect(toast(page)).toHaveCount(1);
	await expect(toast(page)).toContainText('boom');
});
