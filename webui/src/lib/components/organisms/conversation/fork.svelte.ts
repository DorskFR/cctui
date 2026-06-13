// Fork-conversation hook (CCT-302), extracted from ConversationDrawer with no
// behavior change. Forks the open conversation into a brand-new session,
// optionally changing the model/effort. For claude this doubles as the supported
// "switch model" substitute (no in-place switch — CCT-303); for archived sessions
// it is the "reopen as a new conversation" path. Defaults inherit the parent's
// model/effort so a plain fork preserves them.
import type { SessionListItem } from '@bindings/SessionListItem';
import { toasts } from '$lib/toast.svelte';

export interface ModelOption {
	v: string;
	label: string;
}

// In-place model/effort option lists. The codex set also drives the fork dialog's
// codex branch; the editor UI itself lives in DrawerHeader (CCT-303).
export const CODEX_EFFORTS = ['', 'low', 'medium', 'high', 'xhigh'];
export const CODEX_MODELS: ModelOption[] = [
	{ v: '', label: 'Default' },
	{ v: 'gpt-5.5-codex', label: 'GPT-5.5 Codex' },
	{ v: 'gpt-5.4-codex', label: 'GPT-5.4 Codex' }
];
export const CLAUDE_EFFORTS = ['', 'low', 'medium', 'high', 'xhigh', 'max'];
export const CLAUDE_MODELS: ModelOption[] = [
	{ v: '', label: 'Default' },
	{ v: 'haiku', label: 'Haiku' },
	{ v: 'sonnet', label: 'Sonnet' },
	{ v: 'opus', label: 'Opus' },
	{ v: 'fable', label: 'Fable' }
];

export interface ForkOpts {
	id: () => string;
	archived: () => boolean;
	isCodex: () => boolean;
	session: () => SessionListItem;
	// Server fork action.
	fork: (
		id: string,
		body: { model: string | null; effort: string | null; prompt: null; name: null }
	) => Promise<{ session_id?: string | null } | undefined>;
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

	constructor(opts: ForkOpts) {
		this.#opts = opts;
	}

	get models(): ModelOption[] {
		return this.#opts.isCodex() ? CODEX_MODELS : CLAUDE_MODELS;
	}
	get efforts(): string[] {
		return this.#opts.isCodex() ? CODEX_EFFORTS : CLAUDE_EFFORTS;
	}
	// Parent's total tokens — shown in the fork notice so the user knows the
	// opening turn re-bills this much context (CCT-345).
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
		this.open = true;
	};

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
				prompt: null,
				name: null
			});
			this.open = false;
			toasts.ok(this.#opts.archived() ? 'Reopened as a new conversation' : 'Forked conversation');
			this.#opts.onForked(res?.session_id);
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			this.forking = false;
		}
	};
}
