import type { AccountProvider, OAuthAccount } from '$lib/queries';
import { isPageId, type PageId } from '../provider-drawer/pages.logic';

export const ACCOUNTS_HREF = '/accounts';

export function accountHref(accountId: string, providerId?: string, page?: PageId): string {
	const q = new URLSearchParams();
	if (providerId) q.set('provider', providerId);
	if (providerId && page) q.set('page', page);
	const search = q.toString();
	return `${ACCOUNTS_HREF}/${accountId}${search ? `?${search}` : ''}`;
}

export function editableProviders(account: OAuthAccount, managed: boolean): AccountProvider[] {
	return managed ? [] : account.providers.filter((p) => !p.managed);
}

export function pickProvider(
	providers: AccountProvider[],
	requested: string | null
): AccountProvider | undefined {
	return providers.find((p) => p.id === requested) ?? providers[0];
}

export function pageParam(raw: string | null): PageId | undefined {
	return raw && isPageId(raw) ? raw : undefined;
}
