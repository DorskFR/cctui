import type { AccountProvider, OAuthAccount, SessionBinding } from '#lib/queries/index.ts';

export interface SwitchChoice {
	credId: string;
	name: string;
	provider: string;
	takeover: boolean;
}

export interface SwitchRow {
	binding: SessionBinding;
	limited: boolean;
	current: AccountProvider | null;
	options: SwitchChoice[];
}

export interface PendingSwitch {
	family: string;
	credId: string;
	name: string;
}

export const KEEP = '';

export function switchRows(
	bindings: SessionBinding[],
	accounts: OAuthAccount[],
	limitedCredential: string | null
): SwitchRow[] {
	return bindings.map((b) => {
		const owner = accounts.find((a) => a.id === b.account_id);
		const current = owner?.providers.find((p) => p.id === b.credential_id) ?? null;
		const options = b.options
			.filter((o) => o.credential_id !== b.credential_id)
			.map((o) => ({
				credId: o.credential_id,
				name: o.account_name,
				provider: o.provider,
				takeover: o.takeover
			}));
		return { binding: b, limited: limitedCredential === b.credential_id, current, options };
	});
}

export function pendingSwitches(rows: SwitchRow[], chosen: Record<string, string>): PendingSwitch[] {
	return rows.flatMap((r) => {
		const credId = chosen[r.binding.family];
		const option = credId && credId !== KEEP ? r.options.find((o) => o.credId === credId) : undefined;
		return option ? [{ family: r.binding.family, credId, name: option.name }] : [];
	});
}
