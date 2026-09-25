import type { MeResponse } from '@bindings/MeResponse';
import type { MintKeyRequest } from '@bindings/MintKeyRequest';
import type { MintKeyResponse } from '@bindings/MintKeyResponse';
import { api, ApiError, request } from './api';
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

export async function listGhreviewAccounts(): Promise<GhreviewAccount[]> {
	const base = ghreviewUrl();
	if (!base) return [];
	const token = await ensureGhreviewToken();
	const body = await request<{ items?: GhreviewAccount[] }>(base, '/v1/accounts', { token });
	return body.items ?? [];
}

export interface GhreviewPullPayload {
	number?: number;
	html_url?: string;
	state?: string;
	draft?: boolean;
	head?: { ref?: string };
}

export async function listGhreviewPulls(
	owner: string,
	repo: string,
	account?: string
): Promise<GhreviewPullPayload[]> {
	const base = ghreviewUrl();
	if (!base) return [];
	const token = await ensureGhreviewToken();
	const seg = encodeURIComponent;
	const body = await request<{ items?: { payload?: GhreviewPullPayload }[] }>(
		base,
		`/v1/repos/${seg(owner)}/${seg(repo)}/pulls`,
		{ token, query: { account, limit: 100 } }
	);
	return (body.items ?? []).map((i) => i.payload ?? {});
}

export async function addGhreviewAccount(pat: string, login?: string): Promise<GhreviewAccount> {
	const base = ghreviewUrl();
	if (!base) throw new Error('review backend not configured');
	const token = await ensureGhreviewToken();
	return request<GhreviewAccount>(base, '/v1/accounts', {
		method: 'POST',
		token,
		body: login ? { token: pat, login } : { token: pat }
	});
}

export async function removeGhreviewAccount(id: string): Promise<void> {
	const base = ghreviewUrl();
	if (!base) return;
	const token = await ensureGhreviewToken();
	try {
		await request<void>(base, `/v1/accounts/${id}`, { method: 'DELETE', token });
	} catch (e) {
		if (!(e instanceof ApiError && e.status === 404)) throw e;
	}
}
