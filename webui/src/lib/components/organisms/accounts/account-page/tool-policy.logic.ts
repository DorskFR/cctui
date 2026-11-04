import type { AccountToolPolicy } from '@bindings/AccountToolPolicy';
import type { HookEvent } from '@bindings/HookEvent';
import type { HookRule } from '@bindings/HookRule';
import type { ToolClass } from '@bindings/ToolClass';
import type { ToolPolicy } from '@bindings/ToolPolicy';

export const POLICY_FIELDS = ['terms', 'patterns', 'protected_owners', 'exempt_roots'] as const;

export type PolicyDraft = Record<(typeof POLICY_FIELDS)[number], string>;

export function toDraft(p: ToolPolicy | undefined): PolicyDraft {
	return {
		terms: (p?.terms ?? []).join('\n'),
		patterns: (p?.patterns ?? []).join('\n'),
		protected_owners: (p?.protected_owners ?? []).join('\n'),
		exempt_roots: (p?.exempt_roots ?? []).join('\n')
	};
}

const lines = (text: string) =>
	text
		.split('\n')
		.map((l) => l.trim())
		.filter((l) => l.length > 0);

export function fromDraft(d: PolicyDraft, hooks: HookDraft[] = []): AccountToolPolicy {
	return {
		terms: lines(d.terms),
		patterns: lines(d.patterns),
		protected_owners: lines(d.protected_owners),
		exempt_roots: lines(d.exempt_roots),
		hooks: fromHookDrafts(hooks)
	};
}

export const sameDraft = (a: PolicyDraft, b: PolicyDraft) =>
	POLICY_FIELDS.every((k) => lines(a[k]).join('\n') === lines(b[k]).join('\n'));

/** Exempt roots alone block nothing: the scan is on only with a rule. */
export const policyActive = (p: ToolPolicy) =>
	p.terms.length + p.patterns.length + p.protected_owners.length > 0;

export const HOOK_EVENTS: readonly HookEvent[] = [
	'SessionStart',
	'UserPromptSubmit',
	'PreToolUse',
	'PostToolUse',
	'Stop'
];

export const TOOL_CLASSES: readonly ToolClass[] = [
	'shell',
	'file_edit',
	'file_write',
	'subagent',
	'any'
];

export type HookDraft = { event: HookEvent; tool: ToolClass; command: string; timeout: string };

export const takesTool = (event: HookEvent) => event === 'PreToolUse' || event === 'PostToolUse';

export const blankHook = (): HookDraft => ({
	event: 'PreToolUse',
	tool: 'shell',
	command: '',
	timeout: ''
});

/** The tool-policy query is typed as the gateway policy; the route also returns hooks. */
export const hooksOf = (p: ToolPolicy | undefined): HookRule[] =>
	(p as AccountToolPolicy | undefined)?.hooks ?? [];

export const toHookDrafts = (rules: HookRule[]): HookDraft[] =>
	rules.map((r) => ({
		event: r.event,
		tool: r.tool,
		command: r.command,
		timeout: r.timeout_secs == null ? '' : String(r.timeout_secs)
	}));

function timeoutSecs(text: string): number | null {
	const n = Number(text.trim());
	return text.trim() !== '' && Number.isInteger(n) && n > 0 ? n : null;
}

export const fromHookDrafts = (drafts: HookDraft[]): HookRule[] =>
	drafts
		.filter((d) => d.command.trim() !== '')
		.map((d) => ({
			event: d.event,
			tool: takesTool(d.event) ? d.tool : 'any',
			command: d.command.trim(),
			timeout_secs: timeoutSecs(d.timeout)
		}));

export const sameHooks = (a: HookDraft[], b: HookDraft[]) =>
	JSON.stringify(fromHookDrafts(a)) === JSON.stringify(fromHookDrafts(b));
