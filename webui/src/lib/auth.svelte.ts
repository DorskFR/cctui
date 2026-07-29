import { browser } from '$app/environment';
import { apiBase } from './config';
import { clearCctuiStorage } from './drafts';
import { clearGhreviewToken } from './ghreview';

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
