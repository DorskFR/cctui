import type { FailedDispatch } from '@bindings/FailedDispatch';
import type { FailedDispatchReason } from '@bindings/FailedDispatchReason';
import { m } from '$lib/paraglide/messages';

export interface FailureCause {
	key: string;
	reason: string;
	cause: string;
	explanation: string | null;
	items: FailedDispatch[];
}

export function reasonLabel(reason: FailedDispatchReason): string {
	switch (reason) {
		case 'refused':
			return m.dispatch_failed_refused();
		case 'never_registered':
			return m.dispatch_failed_never_registered();
		case 'oom_killed':
			return m.sessions_end_oom_killed();
		case 'crashed':
			return m.sessions_end_crashed();
		case 'spawn_failed':
			return m.sessions_end_spawn_failed();
	}
}

export function blockedExplanation(d: FailedDispatch): string | null {
	switch (d.rerun_blocked) {
		case 'env':
			return m.dispatch_failed_rerun_env({ keys: [...(d.env_keys ?? [])].sort().join(', ') });
		case 'prompt_truncated':
			return m.dispatch_failed_rerun_prompt();
		case 'prompt_redacted':
			return m.dispatch_failed_rerun_redacted();
		case 'payload_too_large':
			return m.dispatch_failed_rerun_payload();
		default:
			return null;
	}
}

export function causeLabel(d: FailedDispatch): string {
	switch (d.rerun_blocked) {
		case 'env':
			return m.list_failed_cause_env();
		case 'prompt_truncated':
			return m.list_failed_cause_prompt();
		case 'prompt_redacted':
			return m.list_failed_cause_redacted();
		case 'payload_too_large':
			return m.list_failed_cause_payload();
		default:
			return m.list_failed_cause_none();
	}
}

export function causeKey(d: FailedDispatch): string {
	return `${d.reason}|${d.rerun_blocked ?? ''}`;
}

export function groupByCause(list: readonly FailedDispatch[]): FailureCause[] {
	const groups = new Map<string, FailureCause>();
	for (const d of list) {
		const key = causeKey(d);
		const g = groups.get(key);
		if (g) g.items.push(d);
		else
			groups.set(key, {
				key,
				reason: reasonLabel(d.reason),
				cause: causeLabel(d),
				explanation: null,
				items: [d]
			});
	}
	for (const g of groups.values()) {
		const envKeys = [...new Set(g.items.flatMap((d) => d.env_keys ?? []))];
		g.explanation = blockedExplanation({ ...g.items[0], env_keys: envKeys });
	}
	return [...groups.values()];
}

export interface FailureSummary {
	count: number;
	cause: string | null;
	causes: number;
	ids: string[];
}

export function summarizeFailures(list: readonly FailedDispatch[]): FailureSummary | null {
	if (list.length === 0) return null;
	const reasons = new Set(list.map((d) => d.reason));
	return {
		count: list.length,
		cause: reasons.size === 1 ? reasonLabel(list[0].reason) : null,
		causes: reasons.size,
		ids: list.map((d) => d.session_id)
	};
}

const GITHUB_REPO = /^(?:https?:\/\/github\.com\/)?([\w.-]+)\/([\w.-]+?)(?:\.git)?\/?$/;

export function promptFileHref(repo: string | null | undefined, file: string): string | null {
	const r = repo?.trim().match(GITHUB_REPO);
	const path = file.trim().replace(/^\.?\/+/, '');
	if (!r || !path || path.split('/').includes('..')) return null;
	return `https://github.com/${r[1]}/${r[2]}/blob/HEAD/${path.split('/').map(encodeURIComponent).join('/')}`;
}

export const DISPATCHERS_HREF = '/settings/instance#dispatchers';
