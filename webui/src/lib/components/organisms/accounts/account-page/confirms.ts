import { confirmDialog } from '$lib/confirmDialog.svelte';
import { m } from '$lib/paraglide/messages';
import { providerLabel } from '$lib/providers';

export function confirmDeleteAccount(name: string): Promise<boolean> {
	return confirmDialog({
		title: m.acct_delete_title({ name }),
		message: m.account_delete_help({ name }),
		confirmLabel: m.accounts_delete_account()
	});
}

export function confirmRemoveProvider(provider: string, name: string): Promise<boolean> {
	return confirmDialog({
		title: m.acct_remove_provider_title({ provider: providerLabel(provider), name }),
		message: m.acct_remove_provider_message({ provider: providerLabel(provider) }),
		confirmLabel: m.acct_remove_provider_confirm()
	});
}
