import { describe, expect, it } from 'vitest';
import type { SessionProfile } from '@bindings/SessionProfile';
import type { OAuthAccount } from '$lib/queries';
import {
	applySpec,
	initialProfile,
	modelField,
	specChain,
	specChanges,
	specFromForm,
	specOf,
	uniqueProfileName,
	type ProfileSpec
} from './profiles';

const account = (over: Partial<Record<keyof OAuthAccount, unknown>>) =>
	({
		id: 'a1',
		name: 'personal',
		emoji: '🐼',
		providers: [{ provider: 'anthropic' }],
		...over
	}) as unknown as OAuthAccount;

const accounts = [
	account({}),
	account({
		id: 'a2',
		name: 'compat',
		emoji: null,
		providers: [{ provider: 'anthropic-compatible' }]
	}),
	account({ id: 'a3', name: 'oai', emoji: '🦊', providers: [{ provider: 'openai' }] })
];

const form = {
	adapter_id: 'claude-code',
	account: 'personal',
	account_provider: 'anthropic',
	model_claude: 'fable',
	model_codex: 'gpt-5.6',
	model_account: 'llama',
	effort_claude: 'medium',
	effort_codex: 'high',
	permission_mode: 'yolo' as const
};

const profile = (over: Partial<SessionProfile> = {}): SessionProfile => ({
	id: 'p1',
	user_id: 'u1',
	name: 'Orchestrator',
	harness: 'claude-code',
	account_id: 'a1',
	model_alias: 'fable',
	effort: 'medium',
	permission_mode: 'yolo',
	created_at: '',
	updated_at: '',
	...over
});

const labels = {
	auto: 'Auto',
	defaultModel: 'default model',
	defaultEffort: 'default',
	defaultMode: 'Default'
};

describe('modelField', () => {
	it('picks the per-harness family, or the account list for a compatible endpoint', () => {
		expect(modelField('claude-code', accounts[0])).toBe('model_claude');
		expect(modelField('codex', undefined)).toBe('model_codex');
		expect(modelField('claude-code', accounts[1])).toBe('model_account');
	});
});

describe('specFromForm / applySpec', () => {
	it('round-trips the knobs through the form', () => {
		const spec = specFromForm(form, accounts);
		expect(spec).toEqual({
			harness: 'claude-code',
			account_id: 'a1',
			model_alias: 'fable',
			effort: 'medium',
			permission_mode: 'yolo'
		});
		expect(applySpec({ ...form, account: '', model_claude: '' }, spec, accounts)).toMatchObject({
			adapter_id: 'claude-code',
			account: 'personal',
			account_provider: 'anthropic',
			model_claude: 'fable',
			effort_claude: 'medium',
			permission_mode: 'yolo'
		});
	});

	it('writes codex knobs to the codex fields and clears a missing account', () => {
		const spec: ProfileSpec = {
			harness: 'codex',
			account_id: 'gone',
			model_alias: null,
			effort: 'xhigh',
			permission_mode: null
		};
		const out = applySpec(form, spec, accounts);
		expect(out.adapter_id).toBe('codex');
		expect(out.account).toBe('');
		expect(out.account_provider).toBe('');
		expect(out.model_codex).toBe('');
		expect(out.effort_codex).toBe('xhigh');
		expect(out.effort_claude).toBe('medium');
		expect(out.permission_mode).toBe('');
	});

	it('treats blank strings as unset', () => {
		expect(
			specFromForm(
				{ ...form, model_claude: ' ', effort_claude: '', permission_mode: '' },
				accounts
			)
		).toMatchObject({ model_alias: null, effort: null, permission_mode: null });
		expect(specOf(profile({ model_alias: '', effort: null })).model_alias).toBeNull();
	});
});

describe('specChanges / specChain', () => {
	it('counts differing knobs', () => {
		const a = specOf(profile());
		expect(specChanges(a, a)).toBe(0);
		expect(specChanges(a, { ...a, effort: 'high', permission_mode: null })).toBe(2);
	});

	it('renders the summary line', () => {
		expect(specChain(specOf(profile()), accounts, labels, (_h, alias) => `${alias}!`)).toBe(
			'Claude Code · 🐼 personal · fable! · medium · Yolo'
		);
		expect(
			specChain(
				{
					harness: 'codex',
					account_id: null,
					model_alias: null,
					effort: null,
					permission_mode: null
				},
				accounts,
				labels
			)
		).toBe('Codex · Auto · default model · default · Default');
	});
});

describe('uniqueProfileName / initialProfile', () => {
	it('suffixes taken names case-insensitively', () => {
		expect(uniqueProfileName('Default', [])).toBe('Default');
		expect(uniqueProfileName('Default', ['default', 'Default 2'])).toBe('Default 3');
	});

	it('opens on the last-used profile when it still exists', () => {
		const list = [profile({ id: 'p1' }), profile({ id: 'p2' })];
		expect(initialProfile(list, 'p2')?.id).toBe('p2');
		expect(initialProfile(list, 'zz')?.id).toBe('p1');
		expect(initialProfile([], 'p1')).toBeNull();
	});
});
