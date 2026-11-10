import { apiBase } from './config';
import { auth } from './auth.svelte';
import { backendHealth, isNetworkError, isOutageStatus } from './backendHealth.svelte';
import { net } from './netstats.svelte';
import { routeAvailable } from './features.svelte';
import { ROUTES } from '@bindings/routes';
import type { Feature } from '@bindings/Feature';

export class ApiError extends Error {
	status: number;
	constructor(status: number, message: string) {
		super(message);
		this.status = status;
		this.name = 'ApiError';
	}
}

/** A request to a route of a feature the server turned off, refused before it is sent.
 *  It carries the 404 the server would answer, so callers that treat a missing
 *  route as absent keep doing so. */
export class FeatureOffError extends ApiError {
	feature: Feature;
	constructor(feature: Feature) {
		super(404, `feature off: ${feature}`);
		this.feature = feature;
		this.name = 'FeatureOffError';
	}
}

type Method = 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE';

const ROUTE_MATCHERS = ROUTES.map((r) => ({
	method: r.method as string,
	feature: r.feature as Feature | null,
	pattern: new RegExp(
		`^${r.path
			.replace(/[.+?^$()|[\]\\]/g, '\\$&')
			.replace(/\{\*\w+\}/g, '.+')
			.replace(/\{\w+\}/g, '[^/]+')}$`
	),
	literal: r.path.replace(/\{[^}]*\}/g, '').length
}));

/** The feature owning `method path` (a path under the api prefix), `null` for
 *  core and for paths outside the route table. A literal segment wins over a
 *  placeholder: `/sessions/search` is not `/sessions/{id}`. */
export function routeFeature(method: string, path: string): Feature | null {
	const bare = path.split(/[?#]/, 1)[0];
	let best: (typeof ROUTE_MATCHERS)[number] | null = null;
	for (const m of ROUTE_MATCHERS) {
		if (m.method !== method || !m.pattern.test(bare)) continue;
		if (!best || m.literal > best.literal) best = m;
	}
	return best?.feature ?? null;
}

function apiPathOf(url: string): string | null {
	try {
		const target = new URL(url, globalThis.location?.href);
		const base = new URL(apiBase(), globalThis.location?.href);
		if (target.origin !== base.origin) return null;
		const prefix = base.pathname.replace(/\/$/, '');
		return target.pathname.startsWith(`${prefix}/`) ? target.pathname.slice(prefix.length) : null;
	} catch {
		return null;
	}
}

function refuseOffFeature(url: string, method: string): void {
	const path = apiPathOf(url);
	if (path === null) return;
	const feature = routeFeature(method.toUpperCase(), path);
	if (feature && !routeAvailable({ feature })) throw new FeatureOffError(feature);
}

export interface RequestOpts {
	method?: Method;
	body?: unknown;
	/** when set, send as `?key=value`; undefined values are dropped */
	query?: Record<string, string | number | boolean | undefined>;
	/** let the request outlive the document (unload-time flushes) */
	keepalive?: boolean;
	/** Bearer for a non-cctui backend: no cctui cookie is sent, and its 401 does
	 *  not end the cctui session. */
	token?: string;
	signal?: AbortSignal;
}

function buildUrl(base: string, path: string, query?: RequestOpts['query']): string {
	const url = new URL(`${base}${path}`, globalThis.location?.href);
	if (query) {
		for (const [k, v] of Object.entries(query)) {
			if (v !== undefined) url.searchParams.set(k, String(v));
		}
	}
	return url.toString();
}

// Fetch-layer revalidation: the gateway's compressor strips `ETag`, so the
// server mirrors it as `x-etag` and we do the If-None-Match round-trip
// ourselves, replaying the cached body on 304 — transparent to callers.
const revalidation = new Map<string, { etag: string; text: string }>();
const REVALIDATION_MAX = 24;

function rememberEtag(url: string, etag: string, text: string) {
	revalidation.delete(url);
	revalidation.set(url, { etag, text });
	if (revalidation.size > REVALIDATION_MAX) {
		const oldest = revalidation.keys().next().value;
		if (oldest !== undefined) revalidation.delete(oldest);
	}
}

function observe(res: Response) {
	if (isOutageStatus(res.status)) backendHealth.reportFailure();
	else if (res.ok || res.status === 304) backendHealth.reportOk();
}

async function cctuiFetch(url: string, init: RequestInit): Promise<Response> {
	refuseOffFeature(url, init.method ?? 'GET');
	try {
		const res = await fetch(url, init);
		observe(res);
		return res;
	} catch (e) {
		if (isNetworkError(e)) backendHealth.reportFailure();
		throw e;
	}
}

async function handle<T>(res: Response, url: string = res.url, session = true): Promise<T> {
	if (res.status === 401 && session) {
		auth.markLoggedOut();
		throw new ApiError(401, 'Unauthorized');
	}
	if (res.status === 304) {
		net.recordApi(url, 0);
		const cached = revalidation.get(url);
		if (cached) return JSON.parse(cached.text) as T;
		throw new ApiError(304, 'Not modified');
	}
	if (!res.ok) {
		let msg = `${res.status} ${res.statusText}`;
		try {
			const err = (await res.json())?.error;
			if (typeof err === 'string' && err) msg = err;
			else if (typeof err?.message === 'string') msg = err.message;
		} catch {
			/* non-JSON error body */
		}
		throw new ApiError(res.status, msg);
	}
	if (res.status === 204) return undefined as T;
	const text = await res.text();
	net.recordApi(url, text.length);
	const etag = res.headers.get('x-etag') ?? res.headers.get('etag');
	if (etag && text) rememberEtag(url, etag, text);
	if (!text) return undefined as T;
	return JSON.parse(text) as T;
}

export function errMessage(e: unknown): string {
	if (e instanceof Error) return e.message;
	return typeof e === 'string' ? e : String(e);
}

export async function request<T>(base: string, path: string, opts: RequestOpts = {}): Promise<T> {
	const { method = 'GET', body, query, keepalive, token, signal } = opts;
	const headers = new Headers();
	if (body !== undefined) headers.set('Content-Type', 'application/json');
	if (token) headers.set('Authorization', `Bearer ${token}`);
	const url = buildUrl(base, path, query);
	const cached = method === 'GET' ? revalidation.get(url) : undefined;
	if (cached) headers.set('If-None-Match', cached.etag);

	// Auth rides the `HttpOnly` cookie; `credentials: 'include'` makes
	// the browser attach it (works same-origin without CORS credential config).
	const res = await (token ? fetch : cctuiFetch)(url, {
		method,
		headers,
		credentials: token ? 'omit' : 'include',
		keepalive,
		signal,
		body: body !== undefined ? JSON.stringify(body) : undefined
	});

	return handle<T>(res, url, !token);
}

/** Raw cookie-authed fetch for callers that branch on the `Response` itself. */
export function apiFetch(url: string, init: RequestInit = {}): Promise<Response> {
	return cctuiFetch(url, { ...init, credentials: 'include' });
}

/** Fetch a same-origin file/blob href; the caller inspects status and body. */
export function apiBlob(href: string): Promise<Response> {
	return apiFetch(href);
}

/** Send a `multipart/form-data` body (file uploads). The browser sets
 *  the `Content-Type` boundary itself, so we must NOT set it here. Shares the
 *  auth + error handling of {@link request}. */
async function sendForm<T>(method: 'POST' | 'PUT', path: string, form: FormData): Promise<T> {
	const res = await apiFetch(buildUrl(apiBase(), path), { method, body: form });
	return handle<T>(res);
}
/** POST with no body, reading the answer as a `Blob` (audio samples). */
async function postBlob(path: string): Promise<Blob> {
	const res = await apiFetch(buildUrl(apiBase(), path), { method: 'POST' });
	if (!res.ok) await handle(res);
	return res.blob();
}
/** POST a JSON body, reading the answer as a `Blob` (synthesized speech). */
async function postBlobJson(path: string, body: unknown): Promise<Blob> {
	const res = await apiFetch(buildUrl(apiBase(), path), {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(body)
	});
	if (!res.ok) await handle(res);
	return res.blob();
}
const postForm = <T>(path: string, form: FormData) => sendForm<T>('POST', path, form);
const putForm = <T>(path: string, form: FormData) => sendForm<T>('PUT', path, form);

export const api = {
	get: <T>(path: string, query?: RequestOpts['query'], signal?: AbortSignal) =>
		request<T>(apiBase(), path, { query, signal }),
	post: <T>(path: string, body?: unknown) => request<T>(apiBase(), path, { method: 'POST', body }),
	postBlob,
	postBlobJson,
	postForm,
	putForm,
	patch: <T>(path: string, body?: unknown) =>
		request<T>(apiBase(), path, { method: 'PATCH', body }),
	put: <T>(path: string, body?: unknown, opts?: { keepalive?: boolean }) =>
		request<T>(apiBase(), path, { method: 'PUT', body, keepalive: opts?.keepalive }),
	del: <T>(path: string) => request<T>(apiBase(), path, { method: 'DELETE' })
};
