import { expect, test } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';
import { settled, waitForDrawerOpen } from './drawer.fixture';

const SESSIONS = Array.from({ length: 5 }, (_, i) => session(i));

const RULES = Array.from({ length: 10 }, (_, i) => ({
	id: `rule-${i}`,
	kind: 'rule',
	name: `rule-${i}`,
	title: `Rule number ${i}`,
	version: 1
}));

const MEMORIES = [
	['Core split', 'core-split-plan-for-the-next-release-train'],
	['Seed users', 'local-seed-users-and-fixtures-for-integration'],
	['Bot runbook', 'bot-autonomy-runbook-with-a-long-slug'],
	['gh', 'gh-cli-image-attachments-and-upload-workarounds'],
	['Infra ticket draft', 'eks-infra-ticket-draft-pending-review'],
	['npm bootstrap', 'npm-publishing-bootstrap-notes'],
	['Release facts', 'migration-facts-for-the-tax-module'],
	['Watch list', 'breakthrough-watch-list-for-later']
].map(([title, name], i) => ({
	id: `mem-${i}`,
	user_id: 'u1',
	kind: 'memory',
	name,
	title,
	body: '',
	scope: 'user',
	scope_ref: null,
	match: {},
	tags: [],
	enabled: true,
	version: 1,
	pending: null,
	created_by_session: null,
	updated_by_session: null,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
}));

test('attach-context rows stay readable at the largest text size', async ({ page }) => {
	await page.addInitScript(() => localStorage.setItem('tsumikit-font-scale', 'largest'));
	await stubApi(page, SESSIONS, { settings: { display: { fontScale: 1.5 } } });
	await page.route(
		(url) => url.pathname === '/api/v1/context',
		(route) => route.fulfill({ json: MEMORIES })
	);
	await page.route(
		(url) => /^\/api\/v1\/sessions\/[^/]+\/context$/.test(url.pathname),
		(route) => route.fulfill({ json: { rules: RULES, memories: [] } })
	);
	await page.setViewportSize({ width: 1400, height: 900 });
	await page.goto('/sessions');
	await page.locator('[data-journey="session"] [data-journey="title"]').first().click();
	await waitForDrawerOpen(page);

	await page.locator('[data-journey="session-context"]').click();
	const field = page.getByRole('combobox', { name: 'Attach context…' });
	await field.click();
	const listbox = page.getByRole('listbox', { name: 'Attach context…' });
	await expect(listbox.getByRole('option')).toHaveCount(MEMORIES.length);
	await settled(page, '[role="listbox"]');

	const r = await listbox.evaluate((lb) => {
		const box = lb.getBoundingClientRect();
		const composer = document.querySelector('[data-journey="composer"]')!.getBoundingClientRect();
		const rows = [...lb.querySelectorAll<HTMLElement>('[role="option"]')].map((o) => {
			const title = o.querySelector<HTMLElement>('.title')!;
			const slug = o.querySelector<HTMLElement>('.slug')!;
			const row = o.querySelector<HTMLElement>('.row')!.getBoundingClientRect();
			const opt = o.getBoundingClientRect();
			return {
				text: title.textContent,
				clipped: o.scrollHeight > o.clientHeight + 1 || row.top < opt.top - 1 || row.bottom > opt.bottom + 1,
				titleCut: title.scrollWidth > title.clientWidth + 1,
				slugBeforeTitle: slug.clientWidth > 0 && title.clientWidth === 0
			};
		});
		return {
			scale: getComputedStyle(document.documentElement).getPropertyValue('--fs-scale').trim(),
			inViewport: box.top >= 0 && box.bottom <= window.innerHeight,
			overComposer: box.bottom > composer.top && box.top < composer.bottom,
			badges: lb.querySelectorAll('[data-kind]').length,
			rows
		};
	});

	expect(r.scale).toBe('1.5');
	expect(r.inViewport).toBe(true);
	expect(r.overComposer).toBe(false);
	expect(r.badges).toBe(0);
	expect(r.rows.filter((row) => row.clipped)).toEqual([]);
	expect(r.rows.filter((row) => row.titleCut)).toEqual([]);
	expect(r.rows.filter((row) => row.slugBeforeTitle)).toEqual([]);
});
