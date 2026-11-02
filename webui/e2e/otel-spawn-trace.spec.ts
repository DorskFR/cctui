import { tmpdir } from 'node:os';
import type { Page } from '@playwright/test';
import { adminToken, expect, test } from './local-token.fixture';
import { SPAN_KIND_SERVER, startSink, type OtlpSink, type SinkSpan } from './otlp-sink';

const API = (process.env.OTEL_E2E_API ?? 'http://localhost:8700').replace(/\/$/, '');
const SINK_PORT = Number(process.env.OTEL_E2E_SINK_PORT ?? 4318);
const CWD = process.env.OTEL_E2E_CWD ?? tmpdir();
const MACHINE = process.env.OTEL_E2E_MACHINE;
const SPAWN_ROUTE = '/api/v1/sessions/spawn';

type Machine = { machine_id: string; name: string; liveness: string };

async function stackUp(): Promise<boolean> {
	try {
		return (await fetch(`${API}/api/v1/version`, { signal: AbortSignal.timeout(3_000) })).ok;
	} catch {
		return false;
	}
}

function descends(span: SinkSpan, ancestor: SinkSpan, byId: Map<string, SinkSpan>): boolean {
	let parent = span.parentSpanId;
	for (let hops = 0; parent && hops < 64; hops++) {
		if (parent === ancestor.spanId) return true;
		parent = byId.get(parent)?.parentSpanId;
	}
	return false;
}

interface Chain {
	launch: SinkSpan;
	server: SinkSpan;
	db: SinkSpan[];
	command: SinkSpan;
	spawn: SinkSpan;
}

function chainIn(trace: SinkSpan[]): Chain | null {
	const byId = new Map(trace.map((s) => [s.spanId, s]));
	const launch = trace.find((s) => s.name === 'ui.spawn_session.launch');
	if (!launch) return null;
	const server = trace.find(
		(s) =>
			s.kind === SPAN_KIND_SERVER &&
			s.attrs['http.route'] === SPAWN_ROUTE &&
			s.attrs['http.request.method'] === 'POST' &&
			descends(s, launch, byId)
	);
	if (!server) return null;
	const db = trace.filter((s) => s.name === 'db.query' && descends(s, server, byId));
	const command = trace.find((s) => s.name === 'daemon.command' && descends(s, server, byId));
	const spawn = command && trace.find((s) => s.name === 'session.spawn' && descends(s, command, byId));
	return db.length && command && spawn ? { launch, server, db, command, spawn } : null;
}

const byTrace = (spans: SinkSpan[]) =>
	spans.reduce((m, s) => m.set(s.traceId, [...(m.get(s.traceId) ?? []), s]), new Map<string, SinkSpan[]>());

function launchTraces(spans: SinkSpan[]): SinkSpan[][] {
	return [...byTrace(spans).values()].filter((t) => t.some((s) => s.name === 'ui.spawn_session.launch'));
}

function dump(spans: SinkSpan[]): string {
	return [...byTrace(spans)]
		.map(([id, t]) => `${id}\n${t.map((s) => `  ${s.service} ${s.name} ${s.spanId}<-${s.parentSpanId ?? '-'}`).join('\n')}`)
		.join('\n');
}

async function onlineMachine(page: Page): Promise<Machine | undefined> {
	const res = await page.request.get('/api/v1/machines/resources');
	expect(res.ok(), `machines/resources answered ${res.status()}`).toBe(true);
	const rows = (await res.json()) as Machine[];
	return rows.find(
		(m) => m.liveness === 'online' && (!MACHINE || m.machine_id === MACHINE || m.name === MACHINE)
	);
}

async function routeBrowser(page: Page, machine: Machine) {
	await page.route('**/config.js', (route) =>
		route.fulfill({
			contentType: 'application/javascript',
			body: `window.CCTUI_CONFIG = { apiBase: '', otel: { endpoint: '/api/v1/otlp' } };`
		})
	);
	await page.route(
		(url) => url.pathname === '/api/v1/machines/resources',
		async (route) => {
			const res = await route.fetch();
			const rows = (await res.json()) as Machine[];
			await route.fulfill({ response: res, json: rows.filter((m) => m.machine_id === machine.machine_id) });
		}
	);
	await page.route(
		(url) => url.pathname === '/api/v1/sessions/recent-dirs',
		(route) => route.fulfill({ json: [CWD] })
	);
}

test('one Launch click yields one trace: webui -> server -> db -> daemon -> session.spawn', async ({ page }) => {
	test.setTimeout(120_000);
	test.skip(!(await stackUp()), `no cctui server at ${API}: run \`make local/demo\` (see docs/tracing.md)`);

	let sink: OtlpSink;
	try {
		sink = await startSink(SINK_PORT);
	} catch (e) {
		test.skip(true, `OTLP sink cannot listen on :${SINK_PORT} (${e}); set OTEL_E2E_SINK_PORT`);
		return;
	}
	let sessionId: string | null | undefined;
	try {
		const login = await page.request.post('/api/v1/auth/login', { data: { token: adminToken() } });
		expect(login.ok(), `login failed with ${login.status()}`).toBe(true);
		const machine = await onlineMachine(page);
		test.skip(!machine, 'no online daemon: enroll one with OTEL_EXPORTER_OTLP_ENDPOINT set (see docs/tracing.md)');
		await routeBrowser(page, machine!);

		await page.goto('/sessions');
		await page.getByRole('button', { name: 'New session' }).first().click();
		const dialog = page.locator('dialog[open]').first();
		const button = dialog.getByRole('button', { name: /^Launch/ });
		await expect(button).toBeEnabled();
		const spawned = page.waitForResponse((r) => new URL(r.url()).pathname === SPAWN_ROUTE);
		await button.click();
		const res = await spawned;
		expect(res.ok(), `spawn answered ${res.status()}: ${await res.text()}`).toBe(true);
		sessionId = ((await res.json()) as { session_id?: string | null }).session_id;

		await expect
			.poll(() => launchTraces(sink.spans).some((t) => chainIn(t)), {
				message: 'no connected launch trace yet',
				timeout: 60_000,
				intervals: [1_000]
			})
			.toBe(true)
			.catch((e) => {
				throw new Error(`${e}\nsink errors: ${sink.errors.join('; ') || 'none'}\nspans:\n${dump(sink.spans)}`);
			});

		const traces = launchTraces(sink.spans);
		expect(traces, `exactly one launch trace\n${dump(sink.spans)}`).toHaveLength(1);
		const chain = chainIn(traces[0])!;
		expect(chain.db.length).toBeGreaterThanOrEqual(1);
		const services = new Set([chain.launch.service, chain.server.service, chain.spawn.service]);
		expect(services.size, `webui, server and daemon services: ${[...services].join(', ')}`).toBe(3);
		expect(sink.errors).toEqual([]);
	} finally {
		if (sessionId) await page.request.post(`/api/v1/sessions/${sessionId}/kill`).catch(() => undefined);
		await sink.close();
	}
});
