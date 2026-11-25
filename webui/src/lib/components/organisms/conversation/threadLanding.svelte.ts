import { untrack } from 'svelte';
import { replaceState } from '$app/navigation';
import { page } from '$app/state';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { FollowupReason } from '@bindings/FollowupReason';
import type { FollowupResponse } from '@bindings/FollowupResponse';
import type { FollowupFailed, SessionSuperseded } from '#lib/ws/frames.ts';
import { sessionHref } from '#lib/sessionFailureToast.ts';
import { ws } from '#lib/ws.svelte.ts';
import { toasts } from '#lib/toast.svelte.ts';
import { m } from '#lib/paraglide/messages.js';
import type { FollowupLine } from './followupLine.svelte';
import type { EarlierPages } from './earlierPages.svelte';
import { anchorFor } from './threadView';
import type { Line } from './types';

export interface ThreadLandingOpts {
	line: FollowupLine;
	earlier: EarlierPages;
	id: () => string;
	session: () => SessionListItem;
	events: () => AgentEvent[];
	lines: () => Line[];
	headComplete: () => boolean;
	historyLoaded: () => boolean;
	viewKey: () => string;
	keepUrl: () => boolean;
	jumpToBottom: () => void;
	restoreDraft: (text: string) => void;
}

export class ThreadLanding {
	anchor = $state<{ key: string; tick: number } | null>(null);
	#tick = 0;
	#landed: string | null = null;
	readonly #o: ThreadLandingOpts;

	constructor(o: ThreadLandingOpts) {
		this.#o = o;
		$effect(() => {
			const sid = o.id();
			const offSwap = ws.onSuperseded(sid, (ev) => untrack(() => this.#swapHead(ev)));
			const offFail = ws.onFollowupFailed(sid, (ev) => untrack(() => this.#failed(ev)));
			return () => {
				offSwap();
				offFail();
			};
		});
		$effect(() => {
			const target = o.line.target;
			if (!target || !o.historyLoaded()) return;
			const token = `${o.viewKey()}|${target}`;
			if (this.#landed === token) return;
			this.#landed = token;
			void untrack(() => this.#landOn(target));
		});
	}

	#swapHead(ev: SessionSuperseded) {
		const o = this.#o;
		if (!o.line.supersede(ev, { session: o.session(), events: o.events(), complete: o.headComplete() }))
			return;
		if (ev.archive_error) toasts.error(m.composer_followup_archive_failed({ error: ev.archive_error }));
		if (!o.keepUrl()) replaceState(sessionHref(ev.to), { ...page.state, spawned: ev.to } as App.PageState);
	}

	#failed(ev: FollowupFailed) {
		if (!this.#o.line.fail(ev)) return;
		toasts.error(m.followup_thread_failed({ error: ev.error }));
		this.#o.restoreDraft(ev.instruction);
	}

	started = (res: FollowupResponse, instruction: string, reason: FollowupReason) => {
		this.#o.line.begin(res, instruction, reason);
		if (res.state !== 'registered' || !res.to) return;
		this.#swapHead({
			followup_id: res.followup_id,
			from: res.from,
			to: res.to,
			reason,
			instruction,
			archived: true,
			archive_error: null,
			at: new Date().toISOString()
		});
	};

	async #landOn(target: string) {
		const { line, earlier, lines } = this.#o;
		if (target === this.#o.id()) {
			this.#o.jumpToBottom();
			return;
		}
		await line.ready;
		const isRoot = target === line.rootId;
		for (let i = 0; i < 50 && earlier.canFetch; i++) await earlier.fetchEarlier();
		for (let i = 0; i < 50 && line.canFetchOlder && (isRoot || !anchorFor(lines(), target, line.rootId)); i++)
			await line.fetchOlder();
		const key = anchorFor(lines(), target, line.rootId);
		if (key) this.anchor = { key, tick: ++this.#tick };
	}

	open = (sessionId: string): boolean => {
		if (!this.#o.line.members.has(sessionId)) return false;
		void this.#landOn(sessionId);
		return true;
	};
}
