import { expect, test, type Page } from '@playwright/test';
import { session, stubApi, watch } from './tiles.fixture';

function failure(i: number, over: Record<string, unknown> = {}) {
	return {
		session_id: `f0000000-0000-4000-8000-${String(i).padStart(12, '0')}`,
		dispatcher: 'kube',
		name: `nightly ${i}`,
		repo: 'DorskFR/cctui',
		prompt_file: `prompts/job-${i}.md`,
		env_keys: ['CONTEXT_PACK_URL'],
		reason: 'never_registered',
		rerun_blocked: 'env',
		dispatched_at: new Date(Date.now() - i * 60_000).toISOString(),
		...over
	};
}

async function stubFailures(page: Page, list: unknown[]) {
	await page.route('**/api/v1/dispatch-records/failed**', (route) => route.fulfill({ json: list }));
}

test.beforeEach(async ({ page }) => {
	await page.setViewportSize({ width: 1400, height: 900 });
});

test('dispatch failures collapse into one summary row above the list', async ({ page }) => {
	const errors = watch(page);
	await stubApi(page, [session(3)]);
	await stubFailures(page, [failure(1), failure(2), failure(3)]);
	await page.goto('/sessions?view=list');
	const summary = page.locator('[data-journey="failed-dispatch-summary"]');
	await expect(summary).toHaveCount(1);
	await expect(summary).toContainText('3 dispatches failed');
	await expect(summary).toContainText('never started');
	await expect(summary.locator('a[href="/dispatch-failures"]')).toHaveCount(1);
	await expect(page.locator('[data-journey="failed-dispatch"]')).toHaveCount(0);
	await expect(page.getByText('CONTEXT_PACK_URL')).toHaveCount(0);
	expect(errors).toEqual([]);
});

test('a one-session group reads "1 session" and the toolbar has no Show hidden', async ({ page }) => {
	await stubApi(page, [session(3)]);
	await page.goto('/sessions?view=list');
	await page.locator('[data-journey="session"]').first().waitFor();
	await expect(page.locator('[data-journey="section"][data-journey-key="working"]')).toContainText(
		'1 session'
	);
	await expect(page.locator('[data-journey="section"][data-journey-key="working"]')).not.toContainText(
		'1 sessions'
	);
	await expect(page.locator('[data-journey="show-hidden"]')).toHaveCount(0);
	await expect(page.locator('input[placeholder="Search sessions"]')).toHaveCount(1);
});

test('the row name is never cut while the metadata line has room to give', async ({ page }) => {
	const s = {
		...session(3),
		name: 'cctui settings restructure',
		working_dir: '/home/user/Documents/a/very/deep/tree/tsumikit',
		metadata: { git_branch: 'wave/0.30.0-beta.1-design-rework' }
	};
	await stubApi(page, [s]);
	await page.goto('/sessions?view=list');
	const row = page.locator(`[data-session-id="${s.id}"]`);
	await row.waitFor();
	const title = row.locator('[data-journey="title"]');
	const cut = await title.evaluate((el) => el.scrollWidth > el.clientWidth);
	expect(cut).toBe(false);
	await expect(row).toContainText('tsumikit · wave/0.30.0-beta.1-design-rework');
	await expect(row).not.toContainText('/home/user/Documents');
	await expect(row).not.toContainText('Σ');
});

test('/sessions?status=needs-input opens the list filtered to that bucket', async ({ page }) => {
	await stubApi(page, [session(1), session(3)]);
	await page.goto('/sessions?view=list&status=needs-input');
	await expect(page.locator(`[data-session-id="${session(1).id}"]`)).toBeVisible();
	await expect(page.locator(`[data-session-id="${session(3).id}"]`)).toHaveCount(0);
	expect(new URL(page.url()).searchParams.has('status')).toBe(false);
});

test('the failed dispatches page explains each cause once and spans the wide column', async ({ page }) => {
	await stubApi(page, []);
	await stubFailures(page, [failure(1), failure(2), failure(3, { reason: 'crashed', rerun_blocked: null })]);
	await page.goto('/dispatch-failures');
	await expect(page.locator('[data-cause]')).toHaveCount(2);
	await expect(page.getByText(/its env \(CONTEXT_PACK_URL\) is not stored/)).toHaveCount(1);
	await expect(page.locator('a[href="https://github.com/DorskFR/cctui/blob/HEAD/prompts/job-1.md"]')).toHaveCount(1);
	const width = await page
		.locator('[data-journey="dispatch-failures"]')
		.evaluate((el) => el.getBoundingClientRect().width);
	expect(width).toBeGreaterThan(1000);
});

test('the tiles toolbar keeps the page gutter', async ({ page }) => {
	await stubApi(page, [session(3), session(6)]);
	await page.goto('/sessions?view=tiles');
	await page.locator('[data-journey="session-tiles"]').waitFor();
	const left = await page
		.getByRole('heading', { level: 1, name: 'Sessions' })
		.evaluate((el) => el.getBoundingClientRect().left);
	expect(left).toBeGreaterThanOrEqual(12);
});
