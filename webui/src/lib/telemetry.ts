import type { Attributes, Span, Tracer } from '@opentelemetry/api';
import { apiOrigin, otelConfig, type OtelConfig } from './config';

type Otel = typeof import('@opentelemetry/api');

let otel: { api: Otel; tracer: Tracer } | null = null;
let starting: Promise<void> | null = null;

export function tracesUrl(endpoint: string): string {
	const base = endpoint.replace(/\/+$/, '');
	return base.endsWith('/v1/traces') ? base : `${base}/v1/traces`;
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

const URL_ATTRS = ['url.full', 'http.url'] as const;

/** Drops the query and fragment: search routes carry user text in `?q=`. */
export function redactUrl(raw: string): string {
	try {
		const u = new URL(raw, window.location.href);
		return `${u.origin}${u.pathname}`;
	} catch {
		return raw.split(/[?#]/, 1)[0];
	}
}

export function redactSpanUrls(span: Span): void {
	const attrs = (span as unknown as { attributes?: Attributes }).attributes;
	for (const key of URL_ATTRS) {
		const v = attrs?.[key];
		if (typeof v === 'string') span.setAttribute(key, redactUrl(v));
	}
}

/** Starts browser tracing when `CCTUI_CONFIG.otel` is set; otherwise loads nothing. */
export function initTelemetry(): Promise<void> {
	const cfg = otelConfig();
	if (!cfg) return Promise.resolve();
	starting ??= start(cfg).catch((e) => console.warn('telemetry disabled:', e));
	return starting;
}

async function start(cfg: OtelConfig) {
	const [api, web, core, resources, exporter, instrumentation, fetchInstr] = await Promise.all([
		import('@opentelemetry/api'),
		import('@opentelemetry/sdk-trace-web'),
		import('@opentelemetry/core'),
		import('@opentelemetry/resources'),
		import('./otelExporter'),
		import('@opentelemetry/instrumentation'),
		import('@opentelemetry/instrumentation-fetch')
	]);
	const url = new URL(tracesUrl(cfg.endpoint), window.location.href).toString();
	const provider = new web.WebTracerProvider({
		resource: resources.resourceFromAttributes({
			'service.name': cfg.serviceName ?? 'cctui-webui',
			'service.version': __CLIENT_VERSION__
		}),
		sampler: new web.ParentBasedSampler({ root: new web.TraceIdRatioBasedSampler(cfg.sampleRatio ?? 1) }),
		spanProcessors: [new web.BatchSpanProcessor(new exporter.OtlpJsonExporter(url))]
	});
	provider.register({ propagator: new core.W3CTraceContextPropagator() });
	instrumentation.registerInstrumentations({
		tracerProvider: provider,
		instrumentations: [
			new fetchInstr.FetchInstrumentation({
				propagateTraceHeaderCorsUrls: [new RegExp(`^${escapeRe(apiOrigin())}/`)],
				ignoreUrls: [url, /\/(stream|sse)(\?|$)/],
				clearTimingResources: true,
				applyCustomAttributesOnSpan: redactSpanUrls
			})
		]
	});
	otel = { api, tracer: api.trace.getTracer('cctui-webui') };
}

/** Runs `fn` inside an active span so the fetches it issues become children.
 *  The context only survives synchronous code: requests must be started before
 *  `fn`'s first `await`. Without telemetry this is just `fn()`. */
export function withUserSpan<T>(name: string, attrs: Attributes, fn: () => T): T {
	if (!otel) return fn();
	const { api, tracer } = otel;
	return tracer.startActiveSpan(name, { attributes: attrs }, (span) => {
		const fail = (e: unknown) => {
			span.setStatus({ code: api.SpanStatusCode.ERROR });
			span.end();
			throw e;
		};
		let out: T;
		try {
			out = fn();
		} catch (e) {
			return fail(e);
		}
		if (out instanceof Promise) {
			return out.then((v) => {
				span.end();
				return v;
			}, fail) as T;
		}
		span.end();
		return out;
	});
}
