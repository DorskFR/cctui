import { createServer, type Server } from 'node:http';
import { gunzipSync } from 'node:zlib';

export type AttrValue = string | number | boolean;

export interface SinkSpan {
	traceId: string;
	spanId: string;
	parentSpanId?: string;
	name: string;
	kind: number;
	service: string;
	attrs: Record<string, AttrValue>;
}

export const SPAN_KIND_SERVER = 2;

type Field = { no: number; wire: number; int: bigint; bytes: Uint8Array };

function varint(buf: Uint8Array, at: number): [bigint, number] {
	let out = 0n;
	let shift = 0n;
	for (let i = at; i < buf.length; i++) {
		out |= BigInt(buf[i] & 0x7f) << shift;
		if ((buf[i] & 0x80) === 0) return [out, i + 1];
		shift += 7n;
	}
	throw new Error('truncated varint');
}

function* fields(buf: Uint8Array): Generator<Field> {
	const none = new Uint8Array(0);
	let i = 0;
	while (i < buf.length) {
		const [key, next] = varint(buf, i);
		i = next;
		const no = Number(key >> 3n);
		const wire = Number(key & 7n);
		if (wire === 0) {
			const [int, after] = varint(buf, i);
			i = after;
			yield { no, wire, int, bytes: none };
		} else if (wire === 1 || wire === 5) {
			const size = wire === 1 ? 8 : 4;
			yield { no, wire, int: 0n, bytes: buf.subarray(i, i + size) };
			i += size;
		} else if (wire === 2) {
			const [len, after] = varint(buf, i);
			const end = after + Number(len);
			yield { no, wire, int: 0n, bytes: buf.subarray(after, end) };
			i = end;
		} else {
			throw new Error(`unsupported protobuf wire type ${wire}`);
		}
	}
}

const utf8 = new TextDecoder();
const hex = (b: Uint8Array) => Buffer.from(b).toString('hex');

function anyValue(buf: Uint8Array): AttrValue | undefined {
	for (const f of fields(buf)) {
		if (f.no === 1) return utf8.decode(f.bytes);
		if (f.no === 2) return f.int !== 0n;
		if (f.no === 3) return Number(BigInt.asIntN(64, f.int));
		if (f.no === 4) return Buffer.from(f.bytes).readDoubleLE(0);
	}
	return undefined;
}

function keyValue(buf: Uint8Array): [string, AttrValue | undefined] {
	let key = '';
	let value: AttrValue | undefined;
	for (const f of fields(buf)) {
		if (f.no === 1) key = utf8.decode(f.bytes);
		else if (f.no === 2) value = anyValue(f.bytes);
	}
	return [key, value];
}

function attrsOf(list: Uint8Array[]): Record<string, AttrValue> {
	const out: Record<string, AttrValue> = {};
	for (const kv of list) {
		const [k, v] = keyValue(kv);
		if (v !== undefined) out[k] = v;
	}
	return out;
}

function protoSpan(buf: Uint8Array, service: string): SinkSpan {
	const span: SinkSpan = { traceId: '', spanId: '', name: '', kind: 0, service, attrs: {} };
	const attrs: Uint8Array[] = [];
	for (const f of fields(buf)) {
		if (f.no === 1) span.traceId = hex(f.bytes);
		else if (f.no === 2) span.spanId = hex(f.bytes);
		else if (f.no === 4 && f.bytes.length) span.parentSpanId = hex(f.bytes);
		else if (f.no === 5) span.name = utf8.decode(f.bytes);
		else if (f.no === 6) span.kind = Number(f.int);
		else if (f.no === 9) attrs.push(f.bytes);
	}
	span.attrs = attrsOf(attrs);
	return span;
}

export function decodeProtobuf(buf: Uint8Array): SinkSpan[] {
	const out: SinkSpan[] = [];
	for (const rs of fields(buf)) {
		if (rs.no !== 1) continue;
		const resourceAttrs: Uint8Array[] = [];
		const scopes: Uint8Array[] = [];
		for (const f of fields(rs.bytes)) {
			if (f.no === 1) for (const a of fields(f.bytes)) if (a.no === 1) resourceAttrs.push(a.bytes);
			if (f.no === 2) scopes.push(f.bytes);
		}
		const service = String(attrsOf(resourceAttrs)['service.name'] ?? '');
		for (const scope of scopes) {
			for (const f of fields(scope)) if (f.no === 2) out.push(protoSpan(f.bytes, service));
		}
	}
	return out;
}

type JsonValue = { stringValue?: string; boolValue?: boolean; intValue?: string | number; doubleValue?: number };
type JsonAttr = { key: string; value?: JsonValue };
type JsonSpan = {
	traceId: string;
	spanId: string;
	parentSpanId?: string;
	name: string;
	kind?: number;
	attributes?: JsonAttr[];
};
type JsonRequest = {
	resourceSpans?: { resource?: { attributes?: JsonAttr[] }; scopeSpans?: { spans?: JsonSpan[] }[] }[];
};

const jsonId = (id: string) =>
	/^[0-9a-f]*$/i.test(id) ? id.toLowerCase() : Buffer.from(id, 'base64').toString('hex');

function jsonAttrs(list: JsonAttr[] = []): Record<string, AttrValue> {
	const out: Record<string, AttrValue> = {};
	for (const { key, value } of list) {
		if (!value) continue;
		const v =
			value.stringValue ??
			value.boolValue ??
			(value.intValue === undefined ? value.doubleValue : Number(value.intValue));
		if (v !== undefined) out[key] = v;
	}
	return out;
}

export function decodeJson(text: string): SinkSpan[] {
	const req = JSON.parse(text) as JsonRequest;
	return (req.resourceSpans ?? []).flatMap((rs) => {
		const service = String(jsonAttrs(rs.resource?.attributes)['service.name'] ?? '');
		return (rs.scopeSpans ?? []).flatMap((ss) =>
			(ss.spans ?? []).map((s) => ({
				traceId: jsonId(s.traceId),
				spanId: jsonId(s.spanId),
				parentSpanId: s.parentSpanId ? jsonId(s.parentSpanId) : undefined,
				name: s.name,
				kind: s.kind ?? 0,
				service,
				attrs: jsonAttrs(s.attributes)
			}))
		);
	});
}

export function decodeBody(body: Buffer, contentType = '', encoding = ''): SinkSpan[] {
	const raw = encoding.includes('gzip') ? gunzipSync(body) : body;
	return contentType.includes('json') ? decodeJson(raw.toString('utf8')) : decodeProtobuf(raw);
}

export interface OtlpSink {
	spans: SinkSpan[];
	errors: string[];
	close(): Promise<void>;
}

export function startSink(port: number, host = '0.0.0.0'): Promise<OtlpSink> {
	const spans: SinkSpan[] = [];
	const errors: string[] = [];
	const server: Server = createServer((req, res) => {
		const chunks: Buffer[] = [];
		req.on('data', (c: Buffer) => chunks.push(c));
		req.on('end', () => {
			const type = String(req.headers['content-type'] ?? '');
			if (req.method !== 'POST' || !req.url?.startsWith('/v1/traces')) {
				res.writeHead(404).end();
				return;
			}
			try {
				spans.push(...decodeBody(Buffer.concat(chunks), type, String(req.headers['content-encoding'] ?? '')));
			} catch (e) {
				errors.push(String(e));
			}
			if (type.includes('json')) res.writeHead(200, { 'content-type': 'application/json' }).end('{}');
			else res.writeHead(200, { 'content-type': 'application/x-protobuf' }).end();
		});
	});
	return new Promise((resolve, reject) => {
		server.once('error', reject);
		server.listen(port, host, () =>
			resolve({
				spans,
				errors,
				close: () => new Promise<void>((done) => server.close(() => done()))
			})
		);
	});
}
