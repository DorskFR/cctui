/** cctui runtime plugin contract, v1. A plugin's `web/index.js` is an ES module
 *  built against the host's `/plugin-runtime/*` shims (see `./vite.ts`) whose
 *  default export is a `CctuiPluginModule`. */
import type { IconName } from '@dorsk/tsumikit';
import type { Component } from 'svelte';

export const CCTUI_PLUGIN_API = 1;
/** Additive members only; the host accepts any minor, a plugin feature-checks. */
export const CCTUI_PLUGIN_API_MINOR = 1;

/** The session a plugin pane is opened next to. */
export interface PluginSession {
	id: string;
	machine_id: string;
	working_dir: string;
	name?: string | null;
}

/** What a pane may do to the conversation composer of its session. */
export interface ComposerBridge {
	/** Insert at the caret (or append, separated by a blank line), keep the
	 *  draft that was already there, focus the textarea. */
	insertText(text: string): void;
	/** Send `text` as a user message to the session now, as if typed and
	 *  submitted; does not touch the current draft. */
	send(text: string): void;
	addFiles(files: File[]): void;
	focus(): void;
}

/** Props the host mounts a `sessionPane` with. `params` come from the message
 *  action that opened the pane (empty when opened from the drawer header). */
export interface PaneProps {
	session: PluginSession;
	composer: ComposerBridge;
	params: Record<string, string>;
	onclose: () => void;
}

/** Props the host mounts a `page` with. The plugin routes on `path` (always
 *  starting with `/`) and calls `navigate` instead of touching `history`, so the
 *  host keeps the SvelteKit URL — `${basePath}${path}` — in sync. */
export interface PageProps {
	basePath: string;
	path: string;
	navigate(path: string): void;
}

/** A conversation message handed to `messageActions`. */
export interface PluginMessage {
	role: string;
	text: string;
}

/** A button the host renders on a message; clicking it opens the plugin's
 *  session pane with `params`. `autoOpen` asks the host to open the pane by
 *  itself, once per (session, params), when this is the newest message. */
export interface MessageAction {
	label: string;
	icon?: IconName;
	params: Record<string, string>;
	open: 'sessionPane';
	autoOpen?: boolean;
}

/** Svelte context the host sets above every mounted pane. */
export const HOST_CONTEXT_KEY = 'cctui:host';

/** The signed-in user, as the host knows them. */
export interface HostUser {
	id: string;
	name: string;
	isAdmin: boolean;
}

export type HostToastTone = 'ok' | 'info' | 'error';

/** What a plugin asks the host to open its New session form pre-filled with. */
export interface HostSpawnRequest {
	prompt: string;
	working_dir?: string;
	machine_id?: string;
}

/** Everything past `{ cctuiApi, origin }` arrived in a later minor, so a plugin
 *  checks for a member before calling it. */
export interface HostContext {
	cctuiApi: number;
	cctuiApiMinor?: number;
	/** The webui origin, what a skill needs as `--parent-origin`. */
	origin: string;
	user?: HostUser;
	/** `fetch` against the host API (`path` is relative to `/api/v1`), with the
	 *  user's own session: a plugin can do exactly what the user can, no more. */
	apiFetch?(path: string, init?: RequestInit): Promise<Response>;
	/** `fetch` against the plugin's own backend through the host proxy
	 *  (`/api/v1/plugins/<id>/backend/<path>`), which signs the user's identity
	 *  upstream. The upstream URL and its secret never reach the browser. */
	pluginFetch?(path: string, init?: RequestInit): Promise<Response>;
	/** Navigate the host SPA to an app path (`/sessions/...`). */
	navigate?(path: string): void;
	/** Open the host's New session form pre-filled. The user still submits it. */
	openSpawn?(req: HostSpawnRequest): void;
	toast?(message: string, tone?: HostToastTone): void;
}

export interface CctuiPluginModule {
	cctuiApi: typeof CCTUI_PLUGIN_API;
	sessionPane?: Component<PaneProps>;
	page?: Component<PageProps>;
	messageActions?: (msg: PluginMessage) => MessageAction[];
}

/** A plugin's `plugin.json`, as far as the browser half cares. */
export interface PluginManifestWeb {
	id: string;
	cctuiApi: number;
	web?: string;
	page?: { title: string; icon?: string };
	/** Global stylesheets, relative to the plugin folder (`web/app.css`). The
	 *  host links each one once per document, before the module is imported. */
	styles?: string[];
}

/** Shared modules a plugin must not bundle, keyed by import specifier; the
 *  value is the stable URL the host serves them from. */
export const PLUGIN_RUNTIME_PATHS: Record<string, string> = {
	svelte: '/plugin-runtime/svelte.js',
	'svelte/internal/client': '/plugin-runtime/svelte-internal-client.js',
	'svelte/internal/disclose-version': '/plugin-runtime/svelte-internal-disclose-version.js',
	'svelte/store': '/plugin-runtime/svelte-store.js',
	'@dorsk/tsumikit': '/plugin-runtime/tsumikit.js'
};

/** What `/plugin-runtime/manifest.json` reports about the host. */
export interface PluginRuntimeManifest {
	cctuiApi: number;
	cctuiApiMinor?: number;
	svelte: string;
	tsumikit: string;
}
