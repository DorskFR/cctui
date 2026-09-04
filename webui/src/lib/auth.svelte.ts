import { browser } from '$app/environment';
import { apiBase } from './config';
import { clearCctuiStorage } from './drafts';
import { clearGhreviewToken } from './ghreview';
import { getAssertion, passkeysSupported } from './passkeys';
import type { PasskeyChallenge } from '@bindings/PasskeyChallenge';
import type { PasskeyConfig } from '@bindings/PasskeyConfig';

/**
 * Auth state backed by an `HttpOnly` cookie. The token is set
 * server-side by `POST /api/v1/auth/login` and sent automatically by the browser
 * on same-origin requests and the WS upgrade — it is never readable from JS and
 * never stored in `localStorage` or a URL. We therefore can't inspect the token
 * here; we only track whether the session is authenticated, learned by probing
 * `GET /api/v1/me`.
 */
class Auth {
	/** Whether the cookie session is currently valid. */
	isAuthed = $state<boolean>(false);
	/** True until the initial probe resolves, so the UI need not flash the login
	 *  screen before we know the cookie's state. */
	checking = $state<boolean>(true);

	/** Probe the cookie session on startup. */
	async init(): Promise<void> {
		if (!browser) return;
		try {
			const res = await fetch(`${apiBase()}/me`, { credentials: 'include' });
			this.isAuthed = res.ok;
		} catch {
			this.isAuthed = false;
		} finally {
			this.checking = false;
		}
	}

	/** Exchange a token for the `HttpOnly` cookie. Returns true on success. */
	async login(token: string): Promise<boolean> {
		const res = await fetch(`${apiBase()}/auth/login`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			credentials: 'include',
			body: JSON.stringify({ token })
		});
		this.isAuthed = res.ok;
		return res.ok;
	}

	/** What the login screen may offer before anyone has authenticated: whether
	 *  this server can run a passkey ceremony, whether anyone has enrolled one,
	 *  and whether the read should start on its own. Any failure answers "no
	 *  passkeys", so an older server simply shows the token box. */
	async passkeyConfig(): Promise<PasskeyConfig | null> {
		if (!browser || !passkeysSupported()) return null;
		try {
			const res = await fetch(`${apiBase()}/auth/passkey/config`, { credentials: 'include' });
			if (!res.ok) return null;
			return (await res.json()) as PasskeyConfig;
		} catch {
			return null;
		}
	}

	/** Sign in with a passkey. The assertion is the credential: on success the
	 *  server mints a session key and sets the same `HttpOnly` cookie the token
	 *  login sets, so nothing downstream can tell the two apart.
	 *
	 *  `mediation` picks the interaction — a modal by default, `'conditional'`
	 *  to offer the key from the token field's autocomplete — and `signal`
	 *  cancels a pending conditional request. Throws `PasskeyAborted` when the
	 *  user dismisses the dialog; returns false when the server refuses. */
	async loginWithPasskey(
		mediation?: CredentialMediationRequirement,
		signal?: AbortSignal
	): Promise<boolean> {
		const startRes = await fetch(`${apiBase()}/auth/passkey/login/start`, {
			method: 'POST',
			credentials: 'include'
		});
		if (!startRes.ok) return false;
		const challenge = (await startRes.json()) as PasskeyChallenge;
		const credential = await getAssertion(
			challenge.options as Record<string, unknown>,
			mediation,
			signal
		);
		const finishRes = await fetch(`${apiBase()}/auth/passkey/login/finish`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			credentials: 'include',
			body: JSON.stringify({ challenge_id: challenge.challenge_id, credential })
		});
		this.isAuthed = finishRes.ok;
		return finishRes.ok;
	}

	/** Clear the cookie server-side and drop back to the login screen. */
	async logout(): Promise<void> {
		try {
			await fetch(`${apiBase()}/auth/logout`, { method: 'POST', credentials: 'include' });
		} catch {
			/* clear locally regardless of network outcome */
		}
		this.isAuthed = false;
		clearGhreviewToken();
		clearCctuiStorage();
	}

	/** Called by the API/WS layer on a 401 so the UI returns to login. */
	markLoggedOut(): void {
		this.isAuthed = false;
		clearGhreviewToken();
		clearCctuiStorage();
	}
}

export const auth = new Auth();
