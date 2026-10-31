// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { h, tracked } = vi.hoisted(() => {
	const h = {
		loaded: [] as string[],
		fetchConfig: null as null | { ignoreUrls: (string | RegExp)[]; propagateTraceHeaderCorsUrls: RegExp[] }
	};
	const tracked = (name: string) => async (importOriginal: () => Promise<unknown>) => {
		h.loaded.push(name);
		return importOriginal();
	};
	return { h, tracked };
});

vi.mock('@opentelemetry/api', tracked('@opentelemetry/api'));
vi.mock('@opentelemetry/sdk-trace-web', tracked('@opentelemetry/sdk-trace-web'));
vi.mock('@opentelemetry/core', tracked('@opentelemetry/core'));
vi.mock('@opentelemetry/resources', tracked('@opentelemetry/resources'));
vi.mock('@opentelemetry/exporter-trace-otlp-http', () => {
	h.loaded.push('@opentelemetry/exporter-trace-otlp-http');
	return {
		OTLPTraceExporter: class {
			export(_spans: unknown[], done: (r: { code: number }) => void) {
				done({ code: 0 });
			}
			shutdown() {
				return Promise.resolve();
			}
			forceFlush() {
				return Promise.resolve();
			}
		}
	};
});
vi.mock('@opentelemetry/instrumentation', () => {
	h.loaded.push('@opentelemetry/instrumentation');
	return { registerInstrumentations: () => () => {} };
});
vi.mock('@opentelemetry/instrumentation-fetch', () => {
	h.loaded.push('@opentelemetry/instrumentation-fetch');
	return {
		FetchInstrumentation: class {
			constructor(cfg: NonNullable<typeof h.fetchConfig>) {
				h.fetchConfig = cfg;
			}
		}
	};
});

vi.stubGlobal('__CLIENT_VERSION__', '0.0.0-test');

const setConfig = (cfg: Record<string, unknown> | undefined) => Object.assign(window, { CCTUI_CONFIG: cfg });

beforeEach(() => {
	vi.resetModules();
	h.loaded = [];
	h.fetchConfig = null;
});

afterEach(() => {
	delete window.CCTUI_CONFIG;
});

describe('initTelemetry', () => {
	it('loads no OpenTelemetry module without CCTUI_CONFIG.otel', async () => {
		setConfig({ apiBase: 'https://api.test' });
		const { initTelemetry } = await import('./telemetry');
		await initTelemetry();
		expect(h.loaded).toEqual([]);
	});

	it('loads nothing when otel has no endpoint', async () => {
		setConfig({ otel: { serviceName: 'x' } });
		const { initTelemetry } = await import('./telemetry');
		await initTelemetry();
		expect(h.loaded).toEqual([]);
	});

	it('wires fetch propagation to the API origin and ignores the exporter URL', async () => {
		setConfig({ apiBase: 'https://api.test', otel: { endpoint: 'https://otel.test/' } });
		const { initTelemetry } = await import('./telemetry');
		await initTelemetry();
		expect(h.loaded).toContain('@opentelemetry/instrumentation-fetch');
		const cfg = h.fetchConfig!;
		expect(cfg.ignoreUrls).toContain('https://otel.test/v1/traces');
		expect(cfg.propagateTraceHeaderCorsUrls.some((re) => re.test('https://api.test/api/v1/sessions/spawn'))).toBe(
			true
		);
		expect(cfg.propagateTraceHeaderCorsUrls.some((re) => re.test('https://api.test.evil/x'))).toBe(false);
	});
});

describe('tracesUrl', () => {
	it('appends /v1/traces once', async () => {
		const { tracesUrl } = await import('./telemetry');
		expect(tracesUrl('https://otel.test')).toBe('https://otel.test/v1/traces');
		expect(tracesUrl('https://otel.test/v1/traces/')).toBe('https://otel.test/v1/traces');
	});
});

describe('withUserSpan', () => {
	it('runs fn and returns its value without telemetry', async () => {
		const { withUserSpan } = await import('./telemetry');
		expect(withUserSpan('ui.test', {}, () => 42)).toBe(42);
		await expect(withUserSpan('ui.test', {}, async () => 'ok')).resolves.toBe('ok');
		expect(h.loaded).toEqual([]);
	});

	it('runs fn inside an active recording span and rethrows its failure', async () => {
		setConfig({ otel: { endpoint: 'https://otel.test' } });
		const { initTelemetry, withUserSpan } = await import('./telemetry');
		const { trace } = await import('@opentelemetry/api');
		await initTelemetry();
		const inside = await withUserSpan('ui.test', { 'cctui.k': 'v' }, async () => trace.getActiveSpan()?.isRecording());
		expect(inside).toBe(true);
		await expect(
			withUserSpan('ui.fail', {}, async () => {
				throw new Error('boom');
			})
		).rejects.toThrow('boom');
		expect(() =>
			withUserSpan('ui.sync', {}, () => {
				throw new Error('sync boom');
			})
		).toThrow('sync boom');
	});
});
