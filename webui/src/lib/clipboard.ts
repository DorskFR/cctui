import { copyToClipboard } from '@dorsk/tsumikit';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';

// App-wide copy helper: tsumikit's clipboard write + our toast feedback. Use this
// instead of re-implementing navigator.clipboard everywhere.
export async function copyText(text: string, okMsg?: string): Promise<void> {
	if (await copyToClipboard(text)) toasts.ok(okMsg ?? m.common_copied());
	else toasts.error(m.common_clipboard_unavailable());
}
