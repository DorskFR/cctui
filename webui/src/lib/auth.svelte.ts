import { browser } from '$app/environment';

const KEY = 'cctui_admin_token';

/** Reactive auth token, persisted to localStorage. */
class Auth {
	token = $state<string>(browser ? (localStorage.getItem(KEY) ?? '') : '');

	get isAuthed(): boolean {
		return this.token.length > 0;
	}

	set(token: string) {
		this.token = token;
		if (browser) localStorage.setItem(KEY, token);
	}

	clear() {
		this.token = '';
		if (browser) localStorage.removeItem(KEY);
	}
}

export const auth = new Auth();
