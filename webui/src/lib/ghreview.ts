import type { MeResponse } from '@bindings/MeResponse';
import type { MintKeyRequest } from '@bindings/MintKeyRequest';
import type { MintKeyResponse } from '@bindings/MintKeyResponse';
import { api } from './api';

// The cctui session rides an HttpOnly cookie (CCT-423) that JS cannot read, and
// gh-review is a separate origin, so the cookie can't authenticate it. Instead
// we mint a scoped bearer for the current user (CCT-603 resolves it against the
// shared DB) and cache it — one key per browser, renewed near expiry, so a
// Review open is not a fresh mint every time.
const CACHE_KEY = 'cctui:ghreview-token';
const TTL_MS = 30 * 24 * 60 * 60 * 1000;
const RENEW_MS = 24 * 60 * 60 * 1000;

interface Cached {
	token: string;
	expiresAt: number;
	userId: string;
}

function readCache(): Cached | null {
	try {
		const raw = localStorage.getItem(CACHE_KEY);
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
		scopes: me.scopes,
		expires_at: new Date(expiresAt).toISOString()
	};
	const res = await api.post<MintKeyResponse>(`/users/${me.user_id}/keys`, body);

	try {
		localStorage.setItem(CACHE_KEY, JSON.stringify({ token: res.key, expiresAt, userId: me.user_id }));
	} catch {
		void 0;
	}
	return res.key;
}

export function clearGhreviewToken(): void {
	try {
		localStorage.removeItem(CACHE_KEY);
	} catch {
		void 0;
	}
}
