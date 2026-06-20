import { apiBase } from './config';
import { auth } from './auth.svelte';

export class ApiError extends Error {
	status: number;
	constructor(status: number, message: string) {
		super(message);
		this.status = status;
		this.name = 'ApiError';
	}
}

type Method = 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE';

interface RequestOpts {
	method?: Method;
	path: string;
	body?: unknown;
	/** when set, send as `?key=value`; undefined values are dropped */
	query?: Record<string, string | number | boolean | undefined>;
}

function buildUrl(path: string, query?: RequestOpts['query']): string {
	const url = new URL(`${apiBase()}${path}`);
	if (query) {
		for (const [k, v] of Object.entries(query)) {
			if (v !== undefined) url.searchParams.set(k, String(v));
		}
	}
	return url.toString();
}

async function request<T>({ method = 'GET', path, body, query }: RequestOpts): Promise<T> {
	const headers = new Headers();
	if (body !== undefined) headers.set('Content-Type', 'application/json');

	// Auth rides the `HttpOnly` cookie (CCT-423); `credentials: 'include'` makes
	// the browser attach it (works same-origin without CORS credential config).
	const res = await fetch(buildUrl(path, query), {
		method,
		headers,
		credentials: 'include',
		body: body !== undefined ? JSON.stringify(body) : undefined
	});

	if (res.status === 401) {
		auth.markLoggedOut();
		throw new ApiError(401, 'Unauthorized');
	}
	if (!res.ok) {
		let msg = `${res.status} ${res.statusText}`;
		try {
			const j = await res.json();
			if (j?.error) msg = j.error;
		} catch {
			/* non-JSON error body */
		}
		throw new ApiError(res.status, msg);
	}

	if (res.status === 204) return undefined as T;
	const text = await res.text();
	if (!text) return undefined as T;
	return JSON.parse(text) as T;
}

/** POST a `multipart/form-data` body (CCT-203 file uploads). The browser sets
 *  the `Content-Type` boundary itself, so we must NOT set it here. Shares the
 *  auth + error handling of {@link request}. */
async function postForm<T>(path: string, form: FormData): Promise<T> {
	const headers = new Headers();

	const res = await fetch(buildUrl(path), {
		method: 'POST',
		headers,
		credentials: 'include',
		body: form
	});

	if (res.status === 401) {
		auth.markLoggedOut();
		throw new ApiError(401, 'Unauthorized');
	}
	if (!res.ok) {
		let msg = `${res.status} ${res.statusText}`;
		try {
			const j = await res.json();
			if (j?.error) msg = j.error;
		} catch {
			/* non-JSON error body */
		}
		throw new ApiError(res.status, msg);
	}
	if (res.status === 204) return undefined as T;
	const text = await res.text();
	if (!text) return undefined as T;
	return JSON.parse(text) as T;
}

export const api = {
	get: <T>(path: string, query?: RequestOpts['query']) => request<T>({ path, query }),
	post: <T>(path: string, body?: unknown) => request<T>({ method: 'POST', path, body }),
	postForm,
	patch: <T>(path: string, body?: unknown) => request<T>({ method: 'PATCH', path, body }),
	put: <T>(path: string, body?: unknown) => request<T>({ method: 'PUT', path, body }),
	del: <T>(path: string) => request<T>({ method: 'DELETE', path })
};
