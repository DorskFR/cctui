import { describe, expect, it, vi } from 'vitest';
import type { ReadableSpan } from '@opentelemetry/sdk-trace-web';
import { OtlpJsonExporter, nanos, otlpJson } from './otelExporter';

const resource = { attributes: { 'service.name': 'cctui-webui', 'service.version': '1.2.3' } };

function span(name: string, over: Partial<Record<string, unknown>> = {}): ReadableSpan {
	return {
		name,
		kind: 2,
		spanContext: () => ({ traceId: 'a'.repeat(32), spanId: 'b'.repeat(16), traceFlags: 1 }),
		parentSpanContext: { traceId: 'a'.repeat(32), spanId: 'c'.repeat(16), traceFlags: 1 },
		startTime: [1, 5],
		endTime: [2, 0],
		status: { code: 2, message: 'boom' },
		attributes: { 'http.request.method': 'POST', 'http.response.status_code': 403, ratio: 0.5, ok: false },
		links: [],
		events: [{ time: [1, 10], name: 'ev', attributes: { k: ['x', 'y'] } }],
		duration: [1, 0],
		ended: true,
		resource,
		instrumentationScope: { name: 'fetch', version: '0.1' },
		droppedAttributesCount: 0,
		droppedEventsCount: 0,
		droppedLinksCount: 0,
		...over
	} as unknown as ReadableSpan;
}

type Call = { url: string; init: RequestInit };

function fakeFetch(status: number) {
	const calls: Call[] = [];
	const impl = (async (url: string, init: RequestInit) => {
		calls.push({ url, init });
		return new Response(null, { status });
	}) as unknown as typeof fetch;
	return { calls, impl: () => impl };
}

describe('otlpJson', () => {
	it('encodes spans as OTLP/JSON grouped by resource and scope', () => {
		const out = otlpJson([span('a'), span('b'), span('c', { instrumentationScope: { name: 'ui' } })]);
		expect(out.resourceSpans).toHaveLength(1);
		const rs = out.resourceSpans[0];
		expect(rs.resource.attributes).toContainEqual({ key: 'service.name', value: { stringValue: 'cctui-webui' } });
		expect(rs.scopeSpans.map((s) => [s.scope.name, s.spans.length])).toEqual([
			['fetch', 2],
			['ui', 1]
		]);
		const s = rs.scopeSpans[0].spans[0];
		expect(s).toMatchObject({
			traceId: 'a'.repeat(32),
			spanId: 'b'.repeat(16),
			parentSpanId: 'c'.repeat(16),
			kind: 3,
			startTimeUnixNano: '1000000005',
			endTimeUnixNano: '2000000000',
			status: { code: 2, message: 'boom' }
		});
		expect(s.attributes).toEqual([
			{ key: 'http.request.method', value: { stringValue: 'POST' } },
			{ key: 'http.response.status_code', value: { intValue: '403' } },
			{ key: 'ratio', value: { doubleValue: 0.5 } },
			{ key: 'ok', value: { boolValue: false } }
		]);
		expect(s.events[0].attributes).toEqual([
			{ key: 'k', value: { arrayValue: { values: [{ stringValue: 'x' }, { stringValue: 'y' }] } } }
		]);
	});

	it('keeps nanosecond precision beyond 2^53', () => {
		expect(nanos([1_700_000_000, 123_456_789])).toBe('1700000000123456789');
	});
});

describe('OtlpJsonExporter', () => {
	it('posts in the default cors mode with the cookie so the browser sends the real Origin', async () => {
		const { calls, impl } = fakeFetch(200);
		const done = vi.fn();
		const exporter = new OtlpJsonExporter('https://cctui.test/api/v1/otlp/v1/traces', impl);
		exporter.export([span('a')], done);
		await exporter.forceFlush();
		expect(done).toHaveBeenCalledWith({ code: 0 });
		expect(calls).toHaveLength(1);
		const { url, init } = calls[0];
		expect(url).toBe('https://cctui.test/api/v1/otlp/v1/traces');
		expect(init.mode).toBeUndefined();
		expect(init.credentials).toBe('include');
		expect(init.method).toBe('POST');
		expect(init.keepalive).toBe(true);
		expect((init.headers as Record<string, string>)['Content-Type']).toBe('application/json');
		const body = JSON.parse(new TextDecoder().decode(init.body as Uint8Array));
		expect(body.resourceSpans[0].scopeSpans[0].spans[0].name).toBe('a');
	});

	it('reports a rejected export as failed', async () => {
		const { impl } = fakeFetch(403);
		const done = vi.fn();
		const exporter = new OtlpJsonExporter('https://cctui.test/x', impl);
		exporter.export([span('a')], done);
		await exporter.forceFlush();
		expect(done.mock.calls[0][0].code).toBe(1);
	});

	it('drops keepalive for bodies over the browser budget', async () => {
		const { calls, impl } = fakeFetch(200);
		const exporter = new OtlpJsonExporter('https://cctui.test/x', impl);
		exporter.export([span('big', { attributes: { blob: 'x'.repeat(70 * 1024) } })], () => {});
		await exporter.forceFlush();
		expect(calls[0].init.keepalive).toBe(false);
	});

	it('refuses exports after shutdown', async () => {
		const { calls, impl } = fakeFetch(200);
		const done = vi.fn();
		const exporter = new OtlpJsonExporter('https://cctui.test/x', impl);
		await exporter.shutdown();
		exporter.export([span('a')], done);
		expect(done).toHaveBeenCalledWith({ code: 1 });
		expect(calls).toHaveLength(0);
	});
});
