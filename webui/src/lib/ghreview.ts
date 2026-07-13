import type { MeResponse } from '@bindings/MeResponse';
import type { MintKeyRequest } from '@bindings/MintKeyRequest';
import type { MintKeyResponse } from '@bindings/MintKeyResponse';
import { api } from './api';
import { ghreviewUrl } from './config';

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

// ConnectorInfo never exposes the credential's login; gh-review keys accounts by
// login, so the connector→login mapping must be cached to resolve deletes.
const LOGIN_MAP_KEY = 'cctui:ghreview-connector-logins';

interface GhreviewAccount {
	id: string;
	login: string;
}

function readLoginMap(): Record<string, string> {
	try {
		const raw = localStorage.getItem(LOGIN_MAP_KEY);
		if (!raw) return {};
		const map = JSON.parse(raw) as unknown;
		return map && typeof map === 'object' ? (map as Record<string, string>) : {};
	} catch {
		return {};
	}
}

function writeLoginMap(map: Record<string, string>): void {
	try {
		localStorage.setItem(LOGIN_MAP_KEY, JSON.stringify(map));
	} catch {
		void 0;
	}
}

function rememberConnectorLogin(connectorId: string, login: string): void {
	const map = readLoginMap();
	map[connectorId] = login;
	writeLoginMap(map);
}

function forgetConnectorLogin(connectorId: string): string | null {
	const map = readLoginMap();
	const login = map[connectorId] ?? null;
	if (login !== null) {
		delete map[connectorId];
		writeLoginMap(map);
	}
	return login;
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

export async function provisionGhreviewAccount(
	connectorId: string,
	pat: string
): Promise<string | null> {
	const base = ghreviewUrl();
	if (!base) return null;
	const token = await ensureGhreviewToken();
	const res = await fetch(`${base}/v1/accounts`, {
		method: 'POST',
		headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
		body: JSON.stringify({ token: pat })
	});
	if (!res.ok) throw new Error(await ghreviewError(res));
	const account = (await res.json()) as GhreviewAccount;
	rememberConnectorLogin(connectorId, account.login);
	return account.login;
}

export async function deprovisionGhreviewAccount(connectorId: string): Promise<void> {
	const base = ghreviewUrl();
	const login = forgetConnectorLogin(connectorId);
	if (!base || !login) return;
	const token = await ensureGhreviewToken();
	const listRes = await fetch(`${base}/v1/accounts`, {
		headers: { authorization: `Bearer ${token}` }
	});
	if (!listRes.ok) throw new Error(await ghreviewError(listRes));
	const body = (await listRes.json()) as { items?: GhreviewAccount[] };
	const match = (body.items ?? []).find((a) => a.login === login);
	if (!match) return;
	const delRes = await fetch(`${base}/v1/accounts/${match.id}`, {
		method: 'DELETE',
		headers: { authorization: `Bearer ${token}` }
	});
	if (!delRes.ok && delRes.status !== 404) throw new Error(await ghreviewError(delRes));
}
