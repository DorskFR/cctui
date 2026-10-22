import { createQuery, useQueryClient, type QueryClient } from '@tanstack/svelte-query';
import type { AccountUsageEntry } from '@bindings/AccountUsageEntry';
import type { AccountUsageLedger } from '@bindings/AccountUsageLedger';
import { api } from '$lib/api';
import { endpoints } from '$lib/queries/endpoints';
import { ledgerFrom, patchUsageEntries, usageOf, type PushedUsage } from './model';

/** Safety net, not the primary refresh: freshness comes from the server's
 *  `account_usage` push. Matches the server-side cache TTL, because Anthropic's
 *  usage endpoint rate-limits per access token — do not shorten it. */
export const USAGE_POLL_MS = 180_000;

export const usageKey = ['accounts-usage'] as const;
export const ledgerKey = (from: string) => ['account-usage-ledger', from] as const;

const fetchUsage = () => api.get<AccountUsageEntry[]>('/accounts/usage');

const slow = {
	staleTime: USAGE_POLL_MS,
	refetchInterval: USAGE_POLL_MS,
	refetchOnWindowFocus: false,
	retry: false
} as const;

export const useAllAccountsUsage = (enabled: () => boolean = () => true) =>
	createQuery(() => ({ queryKey: usageKey, queryFn: fetchUsage, enabled: enabled(), ...slow }));

export const useAccountUsage = (credentialId: () => string, enabled: () => boolean = () => true) =>
	createQuery(() => {
		const id = credentialId();
		return {
			queryKey: usageKey,
			queryFn: fetchUsage,
			enabled: enabled(),
			select: (rows: AccountUsageEntry[]) => usageOf(rows, id),
			...slow
		};
	});

type UsagePushSource = {
	onAccountUsage(cb: (ev: { account_id: string; usage: PushedUsage }) => void): () => void;
};

/** Bind once, at the app root: every page reads the patched cache. */
export function bindAccountUsageLive(qc: QueryClient, source: UsagePushSource): () => void {
	return source.onAccountUsage((ev) =>
		qc.setQueryData<AccountUsageEntry[]>(usageKey, (old) =>
			patchUsageEntries(old, ev.account_id, ev.usage)
		)
	);
}

/** Account and window filters apply on the client so every view shares one entry. */
export const useAccountLedger = (enabled: () => boolean = () => true) =>
	createQuery(() => {
		const from = ledgerFrom(Date.now());
		return {
			queryKey: ledgerKey(from),
			queryFn: () => api.get<AccountUsageLedger>('/accounts/usage/ledger', { from }),
			enabled: enabled(),
			...slow
		};
	});

export function useLimitReset() {
	const qc = useQueryClient();
	return async (credentialId: string, creditId?: string | null) => {
		const r = await endpoints.accountLimitReset(credentialId, creditId);
		qc.invalidateQueries({ queryKey: usageKey });
		return r;
	};
}
