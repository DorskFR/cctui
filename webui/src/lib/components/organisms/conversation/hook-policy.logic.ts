import type { HookRule } from '@bindings/HookRule';
import type { SessionHookPolicy } from '@bindings/SessionHookPolicy';

export type HookChip =
	| { kind: 'none' }
	| { kind: 'applied'; rules: HookRule[] }
	| { kind: 'unguarded'; rules: HookRule[] };

export function hookChip(p: SessionHookPolicy | undefined): HookChip {
	if (!p || !Array.isArray(p.rules) || p.rules.length === 0 || p.status == null) {
		return { kind: 'none' };
	}
	return p.status === 'applied'
		? { kind: 'applied', rules: p.rules }
		: { kind: 'unguarded', rules: p.rules };
}

const TOOL_EVENTS = new Set(['PreToolUse', 'PostToolUse']);

export function describeRule(r: HookRule): string {
	const on = TOOL_EVENTS.has(r.event) && r.tool !== 'any' ? `${r.event}(${r.tool})` : r.event;
	return `${on} → ${r.command}`;
}
