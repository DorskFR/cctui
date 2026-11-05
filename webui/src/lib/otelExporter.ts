import type { Attributes, HrTime } from '@opentelemetry/api';
import type { ExportResult } from '@opentelemetry/core';
import type { ReadableSpan, SpanExporter } from '@opentelemetry/sdk-trace-web';

type AnyValue = Record<string, unknown>;
type KeyValue = { key: string; value: AnyValue };

const SUCCESS = 0;
const FAILED = 1;
/** Browsers cap the bodies of all in-flight keepalive requests at 64 KiB. */
const KEEPALIVE_MAX_BYTES = 60 * 1024;

function anyValue(v: unknown): AnyValue {
	if (typeof v === 'string') return { stringValue: v };
	if (typeof v === 'boolean') return { boolValue: v };
	if (typeof v === 'number') return Number.isInteger(v) ? { intValue: String(v) } : { doubleValue: v };
	if (Array.isArray(v)) return { arrayValue: { values: v.map(anyValue) } };
	return v == null ? {} : { stringValue: String(v) };
}

function keyValues(attrs: Attributes | undefined): KeyValue[] {
	return Object.entries(attrs ?? {})
		.filter(([, v]) => v !== undefined)
		.map(([key, v]) => ({ key, value: anyValue(v) }));
}

export function nanos(t: HrTime): string {
	return (BigInt(t[0]) * 1_000_000_000n + BigInt(t[1])).toString();
}

function otlpSpan(s: ReadableSpan) {
	const ctx = s.spanContext();
	return {
		traceId: ctx.traceId,
		spanId: ctx.spanId,
		parentSpanId: s.parentSpanContext?.spanId,
		traceState: ctx.traceState?.serialize(),
		name: s.name,
		kind: s.kind + 1,
		startTimeUnixNano: nanos(s.startTime),
		endTimeUnixNano: nanos(s.endTime),
		attributes: keyValues(s.attributes),
		droppedAttributesCount: s.droppedAttributesCount,
		events: s.events.map((e) => ({
			timeUnixNano: nanos(e.time),
			name: e.name,
			attributes: keyValues(e.attributes),
			droppedAttributesCount: e.droppedAttributesCount ?? 0
		})),
		droppedEventsCount: s.droppedEventsCount,
		links: s.links.map((l) => ({
			traceId: l.context.traceId,
			spanId: l.context.spanId,
			attributes: keyValues(l.attributes),
			droppedAttributesCount: l.droppedAttributesCount ?? 0
		})),
		droppedLinksCount: s.droppedLinksCount,
		status: { code: s.status.code, message: s.status.message }
	};
}

export function otlpJson(spans: ReadableSpan[]) {
	const byResource = new Map<ReadableSpan['resource'], Map<string, ReadableSpan[]>>();
	for (const s of spans) {
		const scopes = byResource.get(s.resource) ?? new Map<string, ReadableSpan[]>();
		byResource.set(s.resource, scopes);
		const scope = s.instrumentationScope;
		const key = JSON.stringify([scope.name, scope.version ?? '']);
		scopes.set(key, [...(scopes.get(key) ?? []), s]);
	}
	return {
		resourceSpans: [...byResource].map(([resource, scopes]) => ({
			resource: { attributes: keyValues(resource.attributes), droppedAttributesCount: 0 },
			scopeSpans: [...scopes.values()].map((group) => ({
				scope: { name: group[0].instrumentationScope.name, version: group[0].instrumentationScope.version },
				spans: group.map(otlpSpan)
			}))
		}))
	};
}

type Fetch = typeof fetch;

function rawFetch(): Fetch {
	const f = globalThis.fetch as Fetch & { __original?: Fetch };
	return f.__original ?? f;
}

/**
 * OTLP/JSON span exporter that posts with the default `cors` request mode.
 *
 * The stock exporter forces `mode: 'same-origin'`; under the SPA's
 * `Referrer-Policy: no-referrer` such a POST carries `Origin: null`, which the
 * server's CSRF guard rightly refuses for cookie-authenticated calls.
 */
export class OtlpJsonExporter implements SpanExporter {
	readonly #url: string;
	readonly #fetch: () => Fetch;
	readonly #pending = new Set<Promise<void>>();
	#stopped = false;

	constructor(url: string, fetchImpl: () => Fetch = rawFetch) {
		this.#url = url;
		this.#fetch = fetchImpl;
	}

	export(spans: ReadableSpan[], done: (result: ExportResult) => void): void {
		if (this.#stopped || spans.length === 0) {
			done({ code: this.#stopped ? FAILED : SUCCESS });
			return;
		}
		const body = new TextEncoder().encode(JSON.stringify(otlpJson(spans)));
		const sent = this.#fetch()(this.#url, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body,
			credentials: 'include',
			keepalive: body.byteLength <= KEEPALIVE_MAX_BYTES
		}).then(
			(res) =>
				done(
					res.ok
						? { code: SUCCESS }
						: { code: FAILED, error: new Error(`OTLP export failed with status ${res.status}`) }
				),
			(error: unknown) => done({ code: FAILED, error: error instanceof Error ? error : new Error(String(error)) })
		);
		this.#pending.add(sent);
		void sent.finally(() => this.#pending.delete(sent));
	}

	async forceFlush(): Promise<void> {
		await Promise.all(this.#pending);
	}

	async shutdown(): Promise<void> {
		this.#stopped = true;
		await this.forceFlush();
	}
}
