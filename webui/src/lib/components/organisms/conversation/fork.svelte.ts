// Fork-conversation hook: forks the open conversation into a brand-new
// session, optionally changing the model/effort. For claude this doubles as
// the supported "switch model" substitute (no in-place switch); for archived
// sessions it is the "reopen as a new conversation" path. Defaults inherit
// the parent's model/effort so a plain fork preserves them.
import { errMessage } from '$lib/api';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { ForkExtract } from '@bindings/ForkExtract';
import type { HarnessModels } from '@bindings/HarnessModels';
import type { ForkRequest } from '@bindings/ForkRequest';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';
import { withCurrentModel, type ModelOption } from '$lib/harnessModels';
import { harnessModelsFallback } from '$lib/domainTables';
import { useHarnessModels } from '$lib/queries';

export type { ModelOption };

export interface ForkOpts {
	id: () => string;
	archived: () => boolean;
	isCodex: () => boolean;
	session: () => SessionListItem;
	// Server fork action.
	fork: (id: string, body: ForkRequest) => Promise<{ session_id?: string | null } | undefined>;
	// Called on a successful fork with the new session id (claude returns one;
	// codex returns none → let the caller jump to it or close + refetch).
	onForked: (newSessionId: string | null | undefined) => void;
}

export class ForkController {
	#opts: ForkOpts;
	forking = $state(false);
	open = $state(false);
	model = $state('');
	effort = $state('');
	prompt = $state('');
	// Conversation-extract selector. Null → full-history fork.
	extract = $state<ForkExtract | null>(null);

	// A fork may land on any machine, so the server merges every catalog.
	// Null when built outside a component (no query context).
	#models: { data?: HarnessModels } | null = null;

	constructor(opts: ForkOpts) {
		this.#opts = opts;
		try {
			this.#models = useHarnessModels(
				() => (opts.isCodex() ? 'codex' : 'claude-code'),
				() => '',
				() => this.model,
				() => this.open
			);
		} catch {
			this.#models = null;
		}
	}

	#fallback() {
		return harnessModelsFallback(this.#opts.isCodex() ? 'codex' : 'claude-code');
	}

	// The parent's model stays selectable even when no list knows it.
	get models(): ModelOption[] {
		return withCurrentModel(this.#models?.data?.models ?? this.#fallback().models, this.model);
	}
	get efforts(): string[] {
		return this.#models?.data?.efforts ?? this.#fallback().efforts;
	}
	// Parent's total tokens — shown in the fork notice so the user knows the
	// opening turn re-bills this much context.
	get parentTokens(): number {
		const u = this.#opts.session().token_usage;
		return (
			Number(u.tokens_in) +
			Number(u.tokens_out) +
			Number(u.cache_read_tokens) +
			Number(u.cache_creation_tokens)
		);
	}

	openDialog = () => {
		const s = this.#opts.session();
		this.model = s.model ?? '';
		this.effort = s.effort ?? '';
		this.extract = null;
		this.prompt = '';
		this.open = true;
	};

	// Open the dialog for a subset fork from an extract of the conversation.
	// Claude-only; the caller gates these actions off for codex.
	openExtract = (extract: ForkExtract) => {
		const s = this.#opts.session();
		this.model = s.model ?? '';
		this.effort = s.effort ?? '';
		this.extract = extract;
		this.prompt = '';
		this.open = true;
	};

	get extractLabel(): string | null {
		const x = this.extract;
		if (!x) return null;
		if (x.mode === 'up_to') return m.fork_extract_up_to();
		if (x.mode === 'after') return m.fork_extract_below();
		return m.fork_extract_selected({ count: (x.selected_message_ids ?? []).length });
	}

	cancel = () => {
		this.open = false;
	};

	submit = async () => {
		if (this.forking) return;
		this.forking = true;
		try {
			const res = await this.#opts.fork(this.#opts.id(), {
				model: this.model.trim() || null,
				effort: this.effort.trim() || null,
				prompt: this.prompt.trim() || null,
				name: null,
				extract: this.extract
			});
			this.open = false;
			toasts.ok(
				this.extract
					? m.fork_toast_from_selected()
					: this.#opts.archived()
						? m.fork_toast_reopened()
						: m.fork_toast_forked()
			);
			this.#opts.onForked(res?.session_id);
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			this.forking = false;
		}
	};
}
