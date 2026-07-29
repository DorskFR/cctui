import type { MeResponse } from '@bindings/MeResponse';
import type { MintKeyRequest } from '@bindings/MintKeyRequest';
import type { MintKeyResponse } from '@bindings/MintKeyResponse';
import { api } from './api';
import { ghreviewUrl } from './config';

const CACHE_KEY = 'cctui:ghreview-token';
// gh-review consults no cctui scope (only maps the bearer to a user), so grant
// the floor `read`: a leaked token can then do nothing else on the cctui API.
const GHREVIEW_SCOPES = ['read'];
const TTL_MS = 60 * 60 * 1000;
const RENEW_MS = 15 * 60 * 1000;

interface Cached {
	token: string;
	expiresAt: number;
	userId: string;
}

function readCache(): Cached | null {
	try {
		const raw = sessionStorage.getItem(CACHE_KEY);
		if (!raw) return null;
		const c = JSON.parse(raw) as Cached;
		if (typeof c.token !== 'string' || typeof c.expiresAt !== 'number') return null;
		return c;
	} catch {
		return null;
	}
}

export async function ensureGhreviewToken(): Promise<string> {
	const me = await api.get<MeResponse>('/me');
	if (!me.user_id) throw new Error('no cctui user for gh-review token');

	const cached = readCache();
	if (cached && cached.userId === me.user_id && cached.expiresAt - Date.now() > RENEW_MS) {
		return cached.token;
	}

	const expiresAt = Date.now() + TTL_MS;
	const body: MintKeyRequest = {
		label: 'gh-review (embedded review center)',
		scopes: GHREVIEW_SCOPES,
		expires_at: new Date(expiresAt).toISOString()
	};
	const res = await api.post<MintKeyResponse>(`/users/${me.user_id}/keys`, body);

	try {
		sessionStorage.setItem(
			CACHE_KEY,
			JSON.stringify({ token: res.key, expiresAt, userId: me.user_id })
		);
	} catch {
		void 0;
	}
	return res.key;
}

export function clearGhreviewToken(): void {
	try {
		sessionStorage.removeItem(CACHE_KEY);
		localStorage.removeItem(CACHE_KEY);
	} catch {
		void 0;
	}
}

export interface GhreviewAccount {
	id: string;
	login: string;
	created_at: string | null;
}

async function ghreviewError(res: Response): Promise<string> {
	try {
		const body = (await res.json()) as { error?: { message?: string } };
		if (body?.error?.message) return body.error.message;
	} catch {
		void 0;
	}
	return `gh-review responded ${res.status}`;
}

export async function listGhreviewAccounts(): Promise<GhreviewAccount[]> {
	const base = ghreviewUrl();
	if (!base) return [];
	const token = await ensureGhreviewToken();
	const res = await fetch(`${base}/v1/accounts`, {
		headers: { authorization: `Bearer ${token}` }
	});
	if (!res.ok) throw new Error(await ghreviewError(res));
	const body = (await res.json()) as { items?: GhreviewAccount[] };
	return body.items ?? [];
}

export async function addGhreviewAccount(pat: string, login?: string): Promise<GhreviewAccount> {
	const base = ghreviewUrl();
	if (!base) throw new Error('review backend not configured');
	const token = await ensureGhreviewToken();
	const res = await fetch(`${base}/v1/accounts`, {
		method: 'POST',
		headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
		body: JSON.stringify(login ? { token: pat, login } : { token: pat })
	});
	if (!res.ok) throw new Error(await ghreviewError(res));
	return (await res.json()) as GhreviewAccount;
}

export async function removeGhreviewAccount(id: string): Promise<void> {
	const base = ghreviewUrl();
	if (!base) return;
	const token = await ensureGhreviewToken();
	const res = await fetch(`${base}/v1/accounts/${id}`, {
		method: 'DELETE',
		headers: { authorization: `Bearer ${token}` }
	});
	if (!res.ok && res.status !== 404) throw new Error(await ghreviewError(res));
}
