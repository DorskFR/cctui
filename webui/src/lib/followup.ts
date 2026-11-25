import type { BriefSession } from '@bindings/BriefSession';
import type { FollowupReason } from '@bindings/FollowupReason';
import type { FollowupRequest } from '@bindings/FollowupRequest';
import type { FollowupResponse } from '@bindings/FollowupResponse';
import type { SessionListItem } from '@bindings/SessionListItem';
import { CCTUI_TAGS } from '#lib/components/organisms/conversation/format.ts';
import { nameOf } from '#lib/sessionName.ts';

/** On a cold cache, Send: launches a follow-up and archives this session
 *  (`auto`), opens the prefilled spawn modal (`modal`), or sends in place with
 *  a banner offering the follow-up (`offer`) or without one (`off`). */
export type FollowupWhenCold = 'auto' | 'modal' | 'offer' | 'off';

const FOLLOWUP_MODES: readonly FollowupWhenCold[] = ['auto', 'modal', 'offer', 'off'];

export const FOLLOWUP_RELATION = 'followup';
export const BRIEF_FILE_NAME = 'previous-conversation.md';
export const INLINE_BRIEF_MAX_BYTES = 48 * 1024;
export const BRIEF_FETCH_MAX_BYTES = 512 * 1024;
export const DEFAULT_INSTRUCTION = 'Continue from this.';
export const PREVIOUS_SESSIONS_TAG = CCTUI_TAGS.previousSessions;

/** A brief as `GET /sessions/{id}/brief` returns it; `sessions` with `?chain=1`. */
export type Brief = string | { markdown: string; sessions?: BriefSession[] | null };

const LINKED_RELATIONS = new Set(['fork', FOLLOWUP_RELATION]);

function relation(s: Pick<SessionListItem, 'metadata'>): unknown {
	return (s.metadata as Record<string, unknown> | null)?.relation;
}

/** A child of its `parent_id`. A fork or follow-up only links to its source. */
export function nestsUnderParent(s: Pick<SessionListItem, 'parent_id' | 'metadata'>): boolean {
	return s.parent_id !== null && !LINKED_RELATIONS.has(relation(s) as string);
}

/** The session this follow-up continues. */
export function previousSessionId(s: Pick<SessionListItem, 'parent_id' | 'metadata'>): string | null {
	return relation(s) === FOLLOWUP_RELATION ? s.parent_id : null;
}

export function clampFollowupWhenCold(v: unknown): FollowupWhenCold {
	return FOLLOWUP_MODES.includes(v as FollowupWhenCold) ? (v as FollowupWhenCold) : 'auto';
}

/** The pre-`auto` setting (`off | offer | default`): `offer` was the default
 *  and becomes `auto`, `default` was today's modal. */
export function migrateFollowupWhenCold(legacy: unknown): FollowupWhenCold {
	if (legacy === 'default') return 'modal';
	if (legacy === 'off') return 'off';
	return 'auto';
}

export function resolveFollowupWhenCold(current: unknown, legacy: unknown): FollowupWhenCold {
	return current === undefined ? migrateFollowupWhenCold(legacy) : clampFollowupWhenCold(current);
}

const COUNTER = /^(.*\S) ([1-9]\d{0,2})$/;

/** A follow-up's name, parsed from its parent's user-set name only: a trailing
 *  ` <n>` (1 to 999) is bumped, anything else gets ` 2`. Forks of one parent
 *  share a name; it is an editable prefill. */
export function followupName(s: Pick<SessionListItem, 'name' | 'metadata'>): string {
	const meta = s.metadata as Record<string, unknown> | null;
	if (meta?.name_source !== 'user') return '';
	const name = nameOf(s);
	if (!name) return '';
	const m = name.match(COUNTER);
	return m ? `${m[1]} ${Number(m[2]) + 1}` : `${name} 2`;
}

const MAX_ATTR_CHARS = 200;
const FILE_POINTER = `In the attached file ${BRIEF_FILE_NAME}.`;
const READ_POINTER =
	`The whole conversation is in the attached file ${BRIEF_FILE_NAME}. Read all of it before you ` +
	'continue: it can be longer than one Read call returns, so keep reading until the end of the file.';

function attr(v: string): string {
	return Array.from(v)
		.slice(0, MAX_ATTR_CHARS)
		.join('')
		.replace(/&/g, '&amp;')
		.replace(/"/g, '&quot;')
		.replace(/</g, '&lt;');
}

function byteLength(s: string): number {
	return new TextEncoder().encode(s).length;
}

export interface FollowupPrompt {
	prompt: string;
	file: string | null;
}

function pointed(pointer: string, tail: string | undefined): string {
	return tail === undefined ? pointer : `${pointer}\n\n${tail}`;
}

function chainEnvelope(sessions: BriefSession[], body: (s: BriefSession, i: number) => string): string {
	const tag = CCTUI_TAGS.previousConversation;
	const blocks = sessions.map((s, i) => {
		const name = s.name === null ? '' : ` session="${attr(s.name)}"`;
		return `<${tag} n="${i + 1}"${name} id="${attr(s.id)}">\n${body(s, i)}\n</${tag}>`;
	});
	return `<${PREVIOUS_SESSIONS_TAG}>\n${blocks.join('\n')}\n</${PREVIOUS_SESSIONS_TAG}>`;
}

/** The same envelope the server's `followup_envelope` builds: the whole chain
 *  in the file, each session's pointer and newest turns inline. */
function chainPrompt(sessions: BriefSession[], ask: string): FollowupPrompt {
	const last = sessions.length - 1;
	const inline = chainEnvelope(sessions, (s, i) =>
		pointed(i === last ? READ_POINTER : FILE_POINTER, s.tail)
	);
	return { prompt: `${inline}\n\n${ask}`, file: `${chainEnvelope(sessions, (s) => s.markdown)}\n` };
}

/** The follow-up's first prompt: one previous-conversation block for a lone
 *  parent, every session of a longer chain inside a previous-sessions block. */
export function followupPrompt(
	session: Pick<SessionListItem, 'id' | 'name' | 'working_dir'>,
	full: Brief,
	instruction: string = DEFAULT_INSTRUCTION
): FollowupPrompt {
	const ask = instruction.trim() || DEFAULT_INSTRUCTION;
	const chain = typeof full === 'string' ? null : (full.sessions ?? null);
	if (chain && chain.length > 1) return chainPrompt(chain, ask);
	const lone = chain?.[0];
	const brief = typeof full === 'string' ? full : (lone?.markdown ?? full.markdown);
	const name = attr(session.name || session.working_dir);
	const tag = CCTUI_TAGS.previousConversation;
	const open = `<${tag} session="${name}" id="${attr(session.id)}">`;
	const close = `</${tag}>`;
	if (byteLength(brief) > INLINE_BRIEF_MAX_BYTES) {
		return {
			prompt: `${open}\n${pointed(READ_POINTER, lone?.tail)}\n${close}\n\n${ask}`,
			file: `${open}\n${brief}\n${close}\n`
		};
	}
	return { prompt: `${open}\n${brief}\n${close}\n\n${ask}`, file: null };
}

export function followupPrefill(
	session: SessionListItem,
	brief: Brief,
	opts: { instruction?: string; archiveSource?: boolean } = {}
): Record<string, string> {
	const adapter = session.adapter_id ?? 'claude-code';
	const codex = adapter === 'codex';
	const { prompt, file } = followupPrompt(session, brief, opts.instruction);
	const full: Record<string, string> = {
		machine_id: session.machine_id,
		working_dir: session.working_dir,
		adapter_id: adapter,
		name: followupName(session),
		prompt,
		[codex ? 'model_codex' : 'model_claude']: session.model ?? '',
		[codex ? 'effort_codex' : 'effort_claude']: session.effort ?? '',
		account: session.account_name ?? '',
		permission_mode: session.permission_mode ?? '',
		label_ids: session.labels.map((l) => l.id).join(','),
		relation: FOLLOWUP_RELATION,
		parent_session_id: session.id,
		archive_source: opts.archiveSource ? '1' : '',
		followup_file: file ?? ''
	};
	return Object.fromEntries(Object.entries(full).filter(([, v]) => v !== ''));
}

/** `launch`: spawn the follow-up now; `followup`: open the prefilled modal. */
export type EnterRoute = 'send' | 'followup' | 'launch';

export function routeEnter(setting: FollowupWhenCold, cold: boolean): EnterRoute {
	if (!cold) return 'send';
	if (setting === 'auto') return 'launch';
	if (setting === 'modal') return 'followup';
	return 'send';
}

export function showColdOffer(setting: FollowupWhenCold, cold: boolean, dismissed: boolean): boolean {
	return cold && !dismissed && setting !== 'off';
}

/** What `POST /sessions/{id}/followup` made of a launch. `failed` hands the
 *  instruction back so the composer can restore it. */
export type FollowupLaunch =
	| { status: 'accepted'; followup: FollowupResponse }
	| { status: 'in_progress'; error: unknown }
	| { status: 'failed'; error: unknown; instruction: string };

export type PostFollowup = (sourceId: string, body: FollowupRequest) => Promise<FollowupResponse>;

/** Ask the server to follow `sourceId` up with `instruction`: it collects the
 *  line, spawns, archives the source once the new session registers, and
 *  announces the swap with `session_superseded`. */
export async function launchFollowup(
	sourceId: string,
	instruction: string,
	reason: FollowupReason,
	post: PostFollowup,
	idempotencyKey: string = crypto.randomUUID()
): Promise<FollowupLaunch> {
	const body: FollowupRequest = {
		instruction,
		reason,
		archive_source: true,
		idempotency_key: idempotencyKey
	};
	try {
		const followup = await post(sourceId, body);
		if (followup.state === 'failed') return { status: 'failed', error: followup.error, instruction };
		return { status: 'accepted', followup };
	} catch (error) {
		if ((error as { status?: unknown } | null)?.status === 409) return { status: 'in_progress', error };
		return { status: 'failed', error, instruction };
	}
}
