import { describe, expect, it } from 'vitest';
import type { OAuthAccount, SessionBinding } from '#lib/queries/index.ts';
import { KEEP, pendingSwitches, switchRows } from './accountSwitch.logic.ts';

const option = (credential_id: string, account_name: string, takeover = false) => ({
	credential_id,
	account_id: `acct-${credential_id}`,
	account_name,
	provider: 'anthropic',
	takeover
});

const bindings: SessionBinding[] = [
	{
		family: 'anthropic',
		credential_id: 'svc-claude',
		account_id: 'svc',
		account_name: 'svc',
		options: [option('personal', 'personal', true)]
	},
	{
		family: 'openai',
		credential_id: 'svc-codex',
		account_id: 'svc-codex-acct',
		account_name: 'svc-codex',
		options: []
	}
];

const viewerAccounts = [
	{
		id: 'stranger-acct',
		name: 'someone-else',
		providers: [{ id: 'stranger', provider: 'anthropic', soft_limits: null }]
	},
	{
		id: 'svc',
		name: 'svc',
		providers: [{ id: 'svc-claude', provider: 'anthropic', soft_limits: null }]
	}
] as unknown as OAuthAccount[];

describe('account switch rows', () => {
	it('offers only what the server accepts, never the viewer-listed rest', () => {
		const rows = switchRows(bindings, viewerAccounts, null);
		expect(rows.map((r) => r.options.map((o) => o.credId))).toEqual([['personal'], []]);
		expect(rows[0].options[0]).toEqual({
			credId: 'personal',
			name: 'personal',
			provider: 'anthropic',
			takeover: true
		});
		expect(rows[0].current?.id).toBe('svc-claude');
		expect(rows[1].current).toBeNull();
	});

	it('drops the current credential and marks the limited binding', () => {
		const withSelf = [{ ...bindings[0], options: [option('svc-claude', 'svc'), option('spare', 'spare')] }];
		const [row] = switchRows(withSelf, [], 'svc-claude');
		expect(row.options.map((o) => o.credId)).toEqual(['spare']);
		expect(row.limited).toBe(true);
	});

	it('submits only changed rows, naming their family and account', () => {
		const rows = switchRows(bindings, viewerAccounts, null);
		expect(pendingSwitches(rows, { anthropic: KEEP, openai: KEEP })).toEqual([]);
		expect(pendingSwitches(rows, { anthropic: 'personal', openai: KEEP })).toEqual([
			{ family: 'anthropic', credId: 'personal', name: 'personal' }
		]);
		expect(pendingSwitches(rows, { anthropic: 'not-offered' })).toEqual([]);
	});
});
