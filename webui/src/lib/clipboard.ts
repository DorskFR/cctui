import { copyToClipboard } from '@dorsk/tsumikit';
import { toasts } from '$lib/toast.svelte';

// App-wide copy helper: tsumikit's clipboard write + our toast feedback. Use this
// instead of re-implementing navigator.clipboard everywhere.
export async function copyText(text: string, okMsg = 'Copied'): Promise<void> {
	if (await copyToClipboard(text)) toasts.ok(okMsg);
	else toasts.err('Clipboard unavailable');
}
