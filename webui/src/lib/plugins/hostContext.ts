import { browser } from '$app/environment';
import { goto } from '$app/navigation';
import { apiFetch } from '$lib/api';
import { apiBase } from '$lib/config';
import { toasts } from '$lib/toast.svelte';
import type { MeResponse } from '$lib/bindings/MeResponse';
import { pluginSpawn } from './spawnRequest.svelte';
import {
	CCTUI_PLUGIN_API,
	CCTUI_PLUGIN_API_MINOR,
	type HostContext,
	type HostToastTone,
	type HostUser
} from './types';

export function hostUser(me: MeResponse | undefined): HostUser | undefined {
	if (!me?.user_id) return undefined;
	return { id: me.user_id, name: me.user_name ?? me.user_id, isAdmin: me.role === 'admin' };
}

function join(path: string): string {
	return path.startsWith('/') ? path : `/${path}`;
}

/** The context the host sets above every mounted plugin surface. `apiFetch` and
 *  `pluginFetch` carry the user's own cookie session, so a plugin acts with the
 *  user's authority and nothing more. */
export function hostContext(opts: { pluginId: string; user?: HostUser }): HostContext {
	return {
		cctuiApi: CCTUI_PLUGIN_API,
		cctuiApiMinor: CCTUI_PLUGIN_API_MINOR,
		origin: browser ? location.origin : '',
		user: opts.user,
		apiFetch: (path, init) => apiFetch(`${apiBase()}${join(path)}`, init),
		pluginFetch: (path, init) =>
			apiFetch(`${apiBase()}/plugins/${encodeURIComponent(opts.pluginId)}/backend${join(path)}`, init),
		navigate: (path) => void goto(join(path)),
		openSpawn: (req) => pluginSpawn.open(req),
		toast: (message, tone?: HostToastTone) => {
			if (tone === 'error') toasts.error(message);
			else if (tone === 'ok') toasts.ok(message);
			else toasts.info(message);
		}
	};
}
