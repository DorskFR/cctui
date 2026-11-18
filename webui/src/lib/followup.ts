import type { BriefSession } from '@bindings/BriefSession';
import type { PermissionMode } from '@bindings/PermissionMode';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SpawnRequest } from '@bindings/SpawnRequest';
import type { SpawnResponse } from '@bindings/SpawnResponse';
import { CCTUI_TAGS } from '#lib/components/organisms/conversation/format.ts';
import { PERMISSION_MODES } from '#lib/domainTables.ts';

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

function attr(v: string): string {
	return v.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
}

function byteLength(s: string): number {
	return new TextEncoder().encode(s).length;
}

export interface FollowupPrompt {
	prompt: string;
	file: string | null;
}

function chainPrompt(sessions: BriefSession[], ask: string): FollowupPrompt {
	const tag = CCTUI_TAGS.previousConversation;
	const envelope = (body: (s: BriefSession, last: boolean) => string) => {
		const blocks = sessions.map((s, i) => {
			const name = s.name === null ? '' : ` session="${attr(s.name)}"`;
			const open = `<${tag} n="${i + 1}"${name} id="${attr(s.id)}">`;
			return `${open}\n${body(s, i === sessions.length - 1)}\n</${tag}>`;
		});
		return `<${PREVIOUS_SESSIONS_TAG}>\n${blocks.join('\n')}\n</${PREVIOUS_SESSIONS_TAG}>`;
	};
	const total = sessions.reduce((n, s) => n + byteLength(s.markdown), 0);
	if (total > INLINE_BRIEF_MAX_BYTES) {
		const pointer = envelope((_, last) =>
			last
				? `The transcripts are in the attached file ${BRIEF_FILE_NAME}. Read it first.`
				: `In the attached file ${BRIEF_FILE_NAME}.`
		);
		return { prompt: `${pointer}\n\n${ask}`, file: `${envelope((s) => s.markdown)}\n` };
	}
	return { prompt: `${envelope((s) => s.markdown)}\n\n${ask}`, file: null };
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
	const brief = typeof full === 'string' ? full : (chain?.[0]?.markdown ?? full.markdown);
	const name = attr(session.name || session.working_dir);
	const tag = CCTUI_TAGS.previousConversation;
	const open = `<${tag} session="${name}" id="${attr(session.id)}">`;
	const close = `</${tag}>`;
	if (byteLength(brief) > INLINE_BRIEF_MAX_BYTES) {
		return {
			prompt: `${open}\nThe transcript is in the attached file ${BRIEF_FILE_NAME}. Read it first.\n${close}\n\n${ask}`,
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
		name: '',
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

/** The spawn request `followupPrefill` describes, sent without the modal. */
export function followupSpawnRequest(p: Record<string, string>): { body: SpawnRequest; files: File[] } {
	const codex = p.adapter_id === 'codex';
	const account = p.account ?? '';
	const mode = p.permission_mode as PermissionMode | undefined;
	const body: SpawnRequest = {
		machine_id: p.machine_id,
		working_dir: p.working_dir,
		adapter_id: p.adapter_id,
		name: null,
		prompt: p.prompt ?? null,
		prompt_name: null,
		model: (codex ? p.model_codex : p.model_claude) || null,
		effort: (codex ? p.effort_codex : p.effort_claude) || null,
		permission_mode: mode && PERMISSION_MODES.includes(mode) ? mode : null,
		account: account || null,
		auto_account: !account,
		label_ids: p.label_ids ? p.label_ids.split(',') : [],
		relation: p.relation ?? null,
		parent_session_id: p.parent_session_id ?? null
	};
	const files = p.followup_file
		? [new File([p.followup_file], BRIEF_FILE_NAME, { type: 'text/markdown' })]
		: [];
	return { body, files };
}

export interface FollowupLaunchDeps {
	brief: (sessionId: string) => Promise<Exclude<Brief, string>>;
	spawn: (body: SpawnRequest, files: File[]) => Promise<SpawnResponse>;
	awaitSpawn: (
		commandId: string,
		sessionId: string | null
	) => Promise<{ ok: boolean; error?: string; timedOut?: boolean; sessionId?: string | null }>;
	archive: (sessionId: string) => Promise<void>;
}

export type FollowupLaunch =
	| { status: 'launched'; sessionId: string | null; archiveError?: unknown }
	| { status: 'unconfirmed'; sessionId: string | null; commandId: string }
	| { status: 'failed'; error: unknown };

/** Spawn the follow-up of `session` and archive `session` once it is
 *  confirmed. Nothing is archived unless the spawn succeeded. */
export async function launchFollowup(
	session: SessionListItem,
	instruction: string,
	deps: FollowupLaunchDeps
): Promise<FollowupLaunch> {
	let res: SpawnResponse;
	try {
		const brief = await deps.brief(session.id);
		const prefill = followupPrefill(session, brief, { instruction, archiveSource: true });
		const req = followupSpawnRequest(prefill);
		res = await deps.spawn(req.body, req.files);
	} catch (error) {
		return { status: 'failed', error };
	}
	const out = await deps.awaitSpawn(res.command_id, res.session_id ?? null);
	const sessionId = out.sessionId ?? res.session_id ?? null;
	if (!out.ok && out.timedOut) return { status: 'unconfirmed', sessionId, commandId: res.command_id };
	if (!out.ok) return { status: 'failed', error: out.error ?? null };
	try {
		await deps.archive(session.id);
	} catch (archiveError) {
		return { status: 'launched', sessionId, archiveError };
	}
	return { status: 'launched', sessionId };
}
