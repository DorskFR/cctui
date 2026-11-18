import type { OAuthAccount } from '@bindings/OAuthAccount';

const byName = (a: OAuthAccount, b: OAuthAccount) => a.name.localeCompare(b.name);

export function ownedAccounts(
	accounts: readonly OAuthAccount[] | undefined,
	meId: string | null | undefined
): OAuthAccount[] {
	if (!meId) return [];
	return (accounts ?? []).filter((a) => a.user_id === meId).sort(byName);
}

export function sharedWithMe(
	accounts: readonly OAuthAccount[] | undefined,
	meId: string | null | undefined
): OAuthAccount[] {
	if (!meId) return [];
	return (accounts ?? []).filter((a) => a.user_id !== meId).sort(byName);
}
