import type { AgentEvent } from '@bindings/AgentEvent';
import type { SessionEndReason } from '@bindings/SessionEndReason';
import type { ServerEvent } from '@bindings/ServerEvent';
import type { AccountUsage } from '../queries/types';

export interface CommandOutcome {
	ok: boolean;
	error?: string;
	timedOut?: boolean;
}

/** What a spawn probe found for the pre-minted session id: `null` while no
 * row exists yet. */
export interface SpawnProbeHit {
	end_reason?: SessionEndReason | null;
	end_detail?: string | null;
}

type EventOf<T extends ServerEvent['type']> = Omit<Extract<ServerEvent, { type: T }>, 'type'>;

export type SessionEndedEvent = EventOf<'session_ended'>;

export type PermReq = EventOf<'permission_request'>;


/** History stores the user's own turns as a `text` event prefixed with this
 * marker (there is no `reply` row on read); live optimistic echoes are `reply`
 * events. Shared so both shapes reconcile to one identity. */
export const USER_PREFIX = '▷ User:';

/** Mirrors the server's `normalize_last_message` (collapse whitespace,
 *  cap at 200 chars) so patched excerpts match the next poll's. */
export function lastMessageExcerpt(content: string): string {
	const collapsed = content.startsWith(USER_PREFIX)
		? content.slice(USER_PREFIX.length).split(/\s+/).filter(Boolean).join(' ')
		: content.split(/\s+/).filter(Boolean).join(' ');
	return collapsed.length > 200 ? `${[...collapsed].slice(0, 200).join('')}…` : collapsed;
}

/** Canonicalize a user-turn body so its optimistic-echo and persisted/server
 * shapes reduce to the same string. The composer appends staged
 * *absolute* upload paths under an `Attached file(s):` header, and the
 * persisted form can diverge on whitespace, trailing newlines, and path
 * rewrites — so content-based dedup fails for attachment messages. Normalize
 * defensively: trim each line, drop blank lines, and reduce every `- <path>`
 * bullet to its basename so absolute-path differences don't matter. */
export function canonUserBody(body: string): string {
	return body
		.split('\n')
		.map((line) => {
			const trimmed = line.trim();
			// Attachment bullets: `- /abs/path/to/file` → `- file`. Also collapses
			// emptied bullets (`-`) the persisted form sometimes carries.
			const m = trimmed.match(/^-\s*(.*)$/);
			if (m) {
				const path = m[1].trim();
				const base = path.split(/[\\/]/).pop() ?? '';
				return base ? `- ${base}` : '-';
			}
			return trimmed;
		})
		.filter((line) => line.length > 0)
		.join('\n');
}

/** Stable identity of a user-typed message across its three shapes (optimistic
 * `reply`, server `reply` echo, persisted `▷ User:` text), or null if `ev`
 * isn't a user message. Used to reconcile optimistic echoes and to dedup the
 * live stream against fetched history. */
/** The client-minted identity of the human turn `ev` belongs to, when it has
 * one. Present only on turns cctui itself sent; everything else falls back to
 * `userMsgKey`. */
export function turnIdOf(ev: AgentEvent): string | null {
	return 'turn_id' in ev && ev.turn_id ? ev.turn_id : null;
}

export function userMsgKey(ev: AgentEvent): string | null {
	if (ev.type === 'reply') return canonUserBody(ev.content);
	if (ev.type === 'text' && ev.content.startsWith(USER_PREFIX))
		return canonUserBody(ev.content.slice(USER_PREFIX.length));
	return null;
}

/** A live GitHub inbox nudge: "something about a tracked PR changed" — the
 * `/github` inbox refetches the affected rows in response. */
export type GithubEvent = EventOf<'github_event'>;
/** A live AskUserQuestion. `questions` is the raw `tool_input.questions` array
 * when the daemon's hook forwarded it, for the interactive option-card form. */
export type LiveAsk = Omit<EventOf<'ask_question'>, 'session_id'>;
/** A live ExitPlanMode plan-approval prompt. */
export type LivePlan = Omit<EventOf<'plan_request'>, 'session_id'>;
/** The gateway refused a request because cctui's share of `account_name`'s
 * usage window is at cap; the webui offers to continue on another account. */
export type SoftLimitBlock = Omit<EventOf<'soft_limit_reached'>, 'session_id'>;
/** The gateway refused to forward a tool call in this session because it
 * matched the account's tool-call policy; the turn ended with an explanation. */
export type ToolBlock = Omit<EventOf<'tool_call_blocked'>, 'session_id'>;
export type MachineResourcesEvent = EventOf<'machine_resources'>;


/** An account's usage windows, pushed by the server refresh that already
 *  fetched them. Patched into the query cache in place: invalidating instead
 *  would refetch and defeat the point of the push. */
export interface AccountUsageEvent {
	account_id: string;
	usage: AccountUsage;
}

export interface SessionListPatch {
	session_id: string;
	last_message_text?: string;
	last_message_at?: string;
	attention?: 'needs_input';
	bucket?: 'blocked';
}

export type MessageAck = EventOf<'message_ack'>;


/** Decode a standard-base64 string to raw bytes (PTY chunks). */
export function decodeBase64(b64: string): Uint8Array {
	const bin = atob(b64);
	const out = new Uint8Array(bin.length);
	for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
	return out;
}

/**
 * A registry of per-key callback sets. `add` returns an unsubscribe fn and drops
 * the key's set once empty, so a long-lived client doesn't accumulate one entry
 * per session ever visited.
 */
export class KeyedListeners<T> {
	private cbs = new Map<string, Set<(v: T) => void>>();

	add(key: string, cb: (v: T) => void): () => void {
		let set = this.cbs.get(key);
		if (!set) {
			set = new Set();
			this.cbs.set(key, set);
		}
		set.add(cb);
		return () => {
			const cur = this.cbs.get(key);
			if (!cur) return;
			cur.delete(cb);
			if (cur.size === 0) this.cbs.delete(key);
		};
	}

	emit(key: string, value: T) {
		const set = this.cbs.get(key);
		if (set) for (const cb of set) cb(value);
	}

	has(key: string): boolean {
		return (this.cbs.get(key)?.size ?? 0) > 0;
	}
}

/** Per-session seed buffer caps. A subscribed-but-unopened session streams
 * events forever, so the buffer is bounded on both counts — it only ever needs
 * to seed a freshly-opened drawer, which then fetches real history. */
const MAX_BUFFER_CHARS = 1_000_000;
const MAX_BUFFER_EVENTS = 1500;

/**
 * Bounded, de-duplicating event buffer. Dedup is by serialized identity via a
 * hash set (one serialization per arriving event), and the total serialized size
 * is tracked incrementally — so appending is O(1), not O(N) stringifies.
 */
export class BoundedEventBuffer {
	private entries: { ev: AgentEvent; sig: string }[] = [];
	private sigs = new Set<string>();
	private chars = 0;

	constructor(
		private maxChars = MAX_BUFFER_CHARS,
		private maxEvents = MAX_BUFFER_EVENTS
	) {}

	push(ev: AgentEvent): boolean {
		const sig = JSON.stringify(ev);
		if (this.sigs.has(sig)) return false;
		this.entries.push({ ev, sig });
		this.sigs.add(sig);
		this.chars += sig.length;
		// Evict oldest-first, but never the event just appended.
		while (this.entries.length > 1 && (this.entries.length > this.maxEvents || this.chars > this.maxChars)) {
			const dropped = this.entries.shift();
			if (!dropped) break;
			this.sigs.delete(dropped.sig);
			this.chars -= dropped.sig.length;
		}
		return true;
	}

	list(): AgentEvent[] {
		return this.entries.map((e) => e.ev);
	}

	clear() {
		this.entries = [];
		this.sigs.clear();
		this.chars = 0;
	}

	get size(): number {
		return this.entries.length;
	}
}

/** A row that exists is a landed spawn unless it ended as a failed start. */
export function spawnOutcomeFromEnd(
	reason: SessionEndReason | null,
	detail: string | null
): CommandOutcome {
	if (reason === 'spawn_failed' || reason === 'resume_failed') {
		return { ok: false, error: detail ?? reason };
	}
	return { ok: true };
}
