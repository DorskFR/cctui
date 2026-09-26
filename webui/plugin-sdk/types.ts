/** cctui runtime plugin contract, v1. A plugin's `web/index.js` is an ES module
 *  built against the host's `/plugin-runtime/*` shims (see `./vite.ts`) whose
 *  default export is a `CctuiPluginModule`. */
import type { IconName } from '@dorsk/tsumikit';
import type { Component } from 'svelte';

export const CCTUI_PLUGIN_API = 1;

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

export interface HostContext {
	cctuiApi: number;
	/** The webui origin, what a skill needs as `--parent-origin`. */
	origin: string;
}

export interface CctuiPluginModule {
	cctuiApi: typeof CCTUI_PLUGIN_API;
	sessionPane?: Component<PaneProps>;
	messageActions?: (msg: PluginMessage) => MessageAction[];
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
	svelte: string;
	tsumikit: string;
}
