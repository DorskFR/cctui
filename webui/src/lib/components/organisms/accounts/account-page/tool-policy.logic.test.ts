import { describe, expect, it } from 'vitest';
import type { HookRule } from '@bindings/HookRule';
import {
	blankHook,
	fromDraft,
	fromHookDrafts,
	hooksOf,
	policyActive,
	sameDraft,
	sameHooks,
	takesTool,
	toDraft,
	toHookDrafts,
	type HookDraft
} from './tool-policy.logic';

describe('tool policy draft', () => {
	it('round-trips one entry per line, dropping blanks and padding', () => {
		const draft = toDraft({
			terms: ['acme'],
			patterns: [],
			protected_owners: ['acme', 'acme-corp'],
			exempt_roots: ['/home/u/work']
		});
		expect(draft.protected_owners).toBe('acme\nacme-corp');
		draft.terms = '  acme \n\n secret\n';
		expect(fromDraft(draft)).toEqual({
			terms: ['acme', 'secret'],
			patterns: [],
			protected_owners: ['acme', 'acme-corp'],
			exempt_roots: ['/home/u/work'],
			hooks: []
		});
	});

	it('treats whitespace-only edits as unchanged', () => {
		const a = toDraft({ terms: ['x'], patterns: [], protected_owners: [], exempt_roots: [] });
		expect(sameDraft(a, { ...a, terms: 'x\n\n' })).toBe(true);
		expect(sameDraft(a, { ...a, terms: 'y' })).toBe(false);
	});

	it('is off with exempt roots only', () => {
		expect(policyActive(fromDraft(toDraft(undefined)))).toBe(false);
		expect(
			policyActive({ terms: [], patterns: [], protected_owners: [], exempt_roots: ['/w'] })
		).toBe(false);
		expect(
			policyActive({ terms: [], patterns: [], protected_owners: ['acme'], exempt_roots: [] })
		).toBe(true);
	});
});

describe('hooks editor', () => {
	const deny: HookRule = {
		event: 'PreToolUse',
		tool: 'shell',
		command: '/opt/guard/deny-rm',
		timeout_secs: 5
	};

	it('round-trips stored rules through the editor rows', () => {
		const stop: HookRule = { event: 'Stop', tool: 'any', command: 'notify', timeout_secs: null };
		const rows = toHookDrafts([deny, stop]);
		expect(rows[0]).toEqual({
			event: 'PreToolUse',
			tool: 'shell',
			command: '/opt/guard/deny-rm',
			timeout: '5'
		});
		expect(rows[1].timeout).toBe('');
		expect(fromHookDrafts(rows)).toEqual([deny, stop]);
	});

	it('drops empty rows, trims commands and ignores a bad timeout', () => {
		const rows: HookDraft[] = [
			{ ...blankHook(), command: '  /opt/guard/deny-rm ', timeout: '5' },
			{ ...blankHook(), command: '   ' },
			{ event: 'Stop', tool: 'file_edit', command: 'n', timeout: '-3' }
		];
		const saved: HookRule[] = [
			deny,
			{ event: 'Stop', tool: 'any', command: 'n', timeout_secs: null }
		];
		expect(fromHookDrafts(rows)).toEqual(saved);
	});

	it('offers a tool class only on tool events', () => {
		expect(takesTool('PreToolUse')).toBe(true);
		expect(takesTool('PostToolUse')).toBe(true);
		expect(takesTool('Stop')).toBe(false);
		expect(takesTool('SessionStart')).toBe(false);
		expect(takesTool('UserPromptSubmit')).toBe(false);
	});

	it('compares rows by what would be saved', () => {
		const a = toHookDrafts([deny]);
		expect(sameHooks(a, [...a, blankHook()])).toBe(true);
		expect(sameHooks(a, [{ ...a[0], command: 'other' }])).toBe(false);
	});

	it('saves hooks with the gateway policy and reads them back', () => {
		const body = fromDraft(toDraft(undefined), toHookDrafts([deny]));
		expect(body.hooks).toEqual([deny]);
		expect(hooksOf(body)).toEqual([deny]);
		expect(hooksOf(undefined)).toEqual([]);
	});
});
