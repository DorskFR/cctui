import { describe, expect, it } from 'vitest';
import { describeRule, hookChip } from './hook-policy.logic';

const deny = {
	event: 'PreToolUse',
	tool: 'shell',
	command: '/opt/guard/deny-rm',
	timeout_secs: null
} as const;

describe('session hook chip', () => {
	it('shows nothing when the launch carried no rules', () => {
		expect(hookChip(undefined)).toEqual({ kind: 'none' });
		expect(hookChip({ rules: [], status: null })).toEqual({ kind: 'none' });
	});

	it('counts the rules a harness applied', () => {
		expect(hookChip({ rules: [deny], status: 'applied' })).toEqual({
			kind: 'applied',
			rules: [deny]
		});
	});

	it('flags a session whose harness could not take the rules', () => {
		expect(hookChip({ rules: [deny], status: 'unsupported' }).kind).toBe('unguarded');
	});

	it('names the tool class only on tool events', () => {
		expect(describeRule(deny)).toBe('PreToolUse(shell) → /opt/guard/deny-rm');
		expect(describeRule({ ...deny, event: 'Stop', tool: 'any' })).toBe('Stop → /opt/guard/deny-rm');
		expect(describeRule({ ...deny, tool: 'any' })).toBe('PreToolUse → /opt/guard/deny-rm');
	});
});
