import { apiBase } from './config';
import { auth } from './auth.svelte';
import { net } from './netstats.svelte';

export class ApiError extends Error {
	status: number;
	constructor(status: number, message: string) {
		super(message);
		this.status = status;
		this.name = 'ApiError';
	}
}

type Method = 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE';

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
	const { method = 'GET', body, query, keepalive, token } = opts;
	const headers = new Headers();
	if (body !== undefined) headers.set('Content-Type', 'application/json');
	if (token) headers.set('Authorization', `Bearer ${token}`);
	const url = buildUrl(base, path, query);
	const cached = method === 'GET' ? revalidation.get(url) : undefined;
	if (cached) headers.set('If-None-Match', cached.etag);

	// Auth rides the `HttpOnly` cookie; `credentials: 'include'` makes
	// the browser attach it (works same-origin without CORS credential config).
	const res = await fetch(url, {
		method,
		headers,
		credentials: token ? 'omit' : 'include',
		keepalive,
		body: body !== undefined ? JSON.stringify(body) : undefined
	});

	return handle<T>(res, url, !token);
}

/** Raw cookie-authed fetch for callers that branch on the `Response` itself. */
export function apiFetch(url: string, init: RequestInit = {}): Promise<Response> {
	return fetch(url, { ...init, credentials: 'include' });
}

/** Fetch a same-origin file/blob href; the caller inspects status and body. */
export function apiBlob(href: string): Promise<Response> {
	return apiFetch(href);
}

/** POST a `multipart/form-data` body (file uploads). The browser sets
 *  the `Content-Type` boundary itself, so we must NOT set it here. Shares the
 *  auth + error handling of {@link request}. */
async function postForm<T>(path: string, form: FormData): Promise<T> {
	const res = await apiFetch(buildUrl(apiBase(), path), { method: 'POST', body: form });
	return handle<T>(res);
}

export const api = {
	get: <T>(path: string, query?: RequestOpts['query']) => request<T>(apiBase(), path, { query }),
	post: <T>(path: string, body?: unknown) => request<T>(apiBase(), path, { method: 'POST', body }),
	postForm,
	patch: <T>(path: string, body?: unknown) =>
		request<T>(apiBase(), path, { method: 'PATCH', body }),
	put: <T>(path: string, body?: unknown, opts?: { keepalive?: boolean }) =>
		request<T>(apiBase(), path, { method: 'PUT', body, keepalive: opts?.keepalive }),
	del: <T>(path: string) => request<T>(apiBase(), path, { method: 'DELETE' })
};
