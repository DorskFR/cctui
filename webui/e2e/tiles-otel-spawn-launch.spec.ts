import { expect, test, type Page, type Request } from '@playwright/test';
import { session, stubApi } from './tiles.fixture';

const PROMPT = 'otel-e2e-prompt-must-not-leak-5f1c';
const SPAWNED = session(0);

interface OtlpSpan {
	name: string;
	traceId: string;
	spanId: string;
	parentSpanId?: string;
}

const hex = (id: string) =>
	/^[0-9a-f]+$/i.test(id) ? id.toLowerCase() : Buffer.from(id, 'base64').toString('hex');

function spansOf(bodies: string[]): OtlpSpan[] {
	return bodies.flatMap((b) =>
		(JSON.parse(b).resourceSpans ?? []).flatMap((rs: { scopeSpans?: { spans?: OtlpSpan[] }[] }) =>
			(rs.scopeSpans ?? []).flatMap((ss) =>
				(ss.spans ?? []).map((s) => ({
					...s,
					traceId: hex(s.traceId),
					spanId: hex(s.spanId),
					parentSpanId: s.parentSpanId ? hex(s.parentSpanId) : undefined
				}))
			)
		)
	);
}

async function stubSpawn(page: Page, otel: boolean) {
	await stubApi(page, [SPAWNED]);
	if (otel) {
		await page.route('**/config.js', (route) =>
			route.fulfill({
				contentType: 'application/javascript',
				body: `window.CCTUI_CONFIG = { apiBase: '', otel: { endpoint: '/otlp' } };`
			})
		);
	}
	await page.route(
		(url) => url.pathname === '/api/v1/machines/resources',
		(route) =>
			route.fulfill({
				json: [
					{
						machine_id: SPAWNED.machine_id,
						name: SPAWNED.machine_name,
						display_name: SPAWNED.machine_name,
						hue: null,
						liveness: 'online',
						last_seen_at: SPAWNED.last_heartbeat,
						resources: null,
						updated_at: null
					}
				]
			})
	);
	await page.route(
		(url) => url.pathname === '/api/v1/sessions/recent-dirs',
		(route) => route.fulfill({ json: [SPAWNED.working_dir] })
	);
	await page.route(
		(url) => url.pathname === '/api/v1/sessions/spawn',
		(route) => route.fulfill({ json: { command_id: 'cmd-otel-1', status: 'dispatched', session_id: SPAWNED.id } })
	);
	const otlp: string[] = [];
	await page.route('**/otlp/v1/traces', (route) => {
		otlp.push(route.request().postData() ?? '');
		return route.fulfill({ json: {} });
	});
	return otlp;
}

async function launch(page: Page): Promise<Request> {
	await page.goto('/sessions');
	await page.getByRole('button', { name: 'New session' }).first().click();
	const dialog = page.locator('dialog[open]').first();
	await dialog.getByPlaceholder(/^Initial prompt/).fill(PROMPT);
	const button = dialog.getByRole('button', { name: /^Launch/ });
	await expect(button).toBeEnabled();
	const spawn = page.waitForRequest((r) => new URL(r.url()).pathname === '/api/v1/sessions/spawn');
	await button.click();
	return spawn;
}

test('Launch starts a browser trace that the spawn request continues', async ({ page }) => {
	test.setTimeout(60_000);
	const otlp = await stubSpawn(page, true);
	const req = await launch(page);

	const traceparent = (await req.allHeaders())['traceparent'];
	expect(traceparent).toMatch(/^00-[0-9a-f]{32}-[0-9a-f]{16}-0[01]$/);
	const [, traceId, parentId] = traceparent.split('-');

	await expect
		.poll(() => spansOf(otlp).some((s) => s.name === 'ui.spawn_session.launch'), { timeout: 30_000 })
		.toBe(true);
	const spans = spansOf(otlp);
	const root = spans.find((s) => s.name === 'ui.spawn_session.launch')!;
	expect(root.traceId).toBe(traceId);
	const fetchSpan = spans.find((s) => s.spanId === parentId);
	expect(fetchSpan, 'the span named by traceparent was exported').toBeDefined();
	expect(fetchSpan!.traceId).toBe(traceId);
	expect(fetchSpan!.parentSpanId).toBe(root.spanId);

	const exported = otlp.join('\n');
	expect(exported).toContain('cctui.spawn.mode');
	expect(exported).not.toContain(PROMPT);
});

test('without CCTUI_CONFIG.otel the spawn request carries no trace context', async ({ page }) => {
	const otlp = await stubSpawn(page, false);
	const req = await launch(page);
	expect((await req.allHeaders())['traceparent']).toBeUndefined();
	expect(otlp).toEqual([]);
});
