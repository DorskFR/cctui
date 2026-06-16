// Session-action hook. Collects the thin "do X to this session" wrappers:
// rename, archive, interrupt, resume, in-place model switch, auto-approve
// toggle, and the export / copy actions. Side-effects (toasts, lifecycle) live
// here; the header/composer stay presentational and call these via callbacks.
// Follows the ForkController / ScrollController pattern used elsewhere in this
// drawer.
import type { SessionListItem } from '@bindings/SessionListItem';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { ViewOpts } from './types';
import { toasts } from '$lib/toast.svelte';
import { ws } from '$lib/ws.svelte';
import { clearSessionStorage } from '$lib/drafts';
import { downloadConversationHtml, conversationToMarkdown } from '$lib/export';

// The slice of useSessionActions() this hook drives. Kept structural so the
// drawer can pass the query mutations straight through.
export interface SessionActionApi {
	rename: (id: string, name: string) => Promise<unknown>;
	archive: (id: string) => Promise<unknown>;
	interrupt: (id: string) => Promise<{ command_id: string }>;
	resume: (id: string) => Promise<unknown>;
	setModel: (id: string, model: string, effort: string) => Promise<unknown>;
	setAutoApprove: (id: string, want: boolean) => Promise<unknown>;
}

export interface SessionActionsOpts {
	id: () => string;
	session: () => SessionListItem;
	// Merged history+live stream + the active view toggles — export/copy honor
	// the current filters, so they read these reactively at call time.
	events: () => AgentEvent[];
	view: () => ViewOpts;
	actions: SessionActionApi;
	// Close the drawer (archive / resume both dismiss it on success).
	onclose: () => void;
}

export class SessionActions {
	#opts: SessionActionsOpts;

	constructor(opts: SessionActionsOpts) {
		this.#opts = opts;
	}

	rename = async (name: string) => {
		try {
			await this.#opts.actions.rename(this.#opts.id(), name);
			toasts.ok('Renamed');
		} catch (e) {
			toasts.err((e as Error).message);
		}
	};

	archive = async () => {
		const s = this.#opts.session();
		try {
			await this.#opts.actions.archive(this.#opts.id());
			// Wipe this session's local composer state (gone once archived, CCT-162)
			// and stop tracking any in-flight/failed sends so auto-retry doesn't run.
			clearSessionStorage(s.id);
			ws.clearDelivery(s.id);
			toasts.ok('Archived');
			this.#opts.onclose();
		} catch (e) {
			toasts.err((e as Error).message);
		}
	};

	interrupt = async () => {
		try {
			const res = await this.#opts.actions.interrupt(this.#opts.id());
			// Wait for the adapter to echo back whether the agent actually
			// accepted the interrupt (CCT-339), rather than fire-and-forget.
			// A timeout is not a hard failure — the interrupt may still have
			// landed — so phrase it as unconfirmed. Interrupts settle fast, so
			// a short wait suffices.
			const ack = await ws.awaitCommand(res.command_id, 8_000);
			if (ack.ok) {
				toasts.ok('Interrupted');
			} else if (ack.timedOut) {
				toasts.push('Interrupt sent — no confirmation yet', 'info');
			} else {
				toasts.err(ack.error ?? 'Interrupt not acknowledged');
			}
		} catch (e) {
			toasts.err((e as Error).message);
		}
	};

	resume = async () => {
		try {
			await this.#opts.actions.resume(this.#opts.id());
			toasts.ok('Resume dispatched');
			this.#opts.onclose();
		} catch (e) {
			toasts.err((e as Error).message);
		}
	};

	// In-place model/effort switch (CCT-303), codex only — the editor UI lives in
	// DrawerHeader, this just dispatches the change.
	setModel = async (model: string, effort: string) => {
		try {
			await this.#opts.actions.setModel(this.#opts.id(), model, effort);
			toasts.ok('Model updated');
		} catch (e) {
			toasts.err((e as Error).message);
		}
	};

	toggleAutoApprove = async () => {
		const want = !this.#opts.session().auto_approve;
		try {
			await this.#opts.actions.setAutoApprove(this.#opts.id(), want);
		} catch (e) {
			toasts.err((e as Error).message);
		}
	};

	// Export the transcript as a self-contained HTML file (CCT-227), gated by the
	// current view toggles and themed with the active palette.
	export = () => {
		try {
			downloadConversationHtml(this.#opts.session(), this.#opts.events(), this.#opts.view());
			toasts.ok('Transcript downloaded');
		} catch (e) {
			toasts.err((e as Error).message);
		}
	};

	// Copy the whole conversation as Markdown (CCT-279 item 9), honoring the
	// current view filters.
	copyMarkdown = async () => {
		try {
			const md = conversationToMarkdown(this.#opts.session(), this.#opts.events(), this.#opts.view());
			await navigator.clipboard.writeText(md);
			toasts.ok('Copied as Markdown');
		} catch (e) {
			toasts.err(`Copy failed: ${(e as Error).message}`);
		}
	};

	// Copy the session's stable, shareable URL (CCT-206).
	copyLink = async () => {
		const url = `${location.origin}/sessions?session=${this.#opts.id()}`;
		try {
			await navigator.clipboard.writeText(url);
			toasts.ok('Link copied');
		} catch {
			toasts.err(url);
		}
	};
}
