// App-wide error surfacing. Any uncaught error — a synchronous throw, an
// unhandled promise rejection, or an error SvelteKit catches while
// navigating/rendering — becomes an error toast, so a failure never blanks a
// view silently. The browser console still gets the full error + stack for
// debugging; the toast stays terse and carries no internals.
import type { HandleClientError } from '@sveltejs/kit';
import { toasts } from '$lib/toast.svelte';
import { m } from '$lib/paraglide/messages';

/** Benign browser notices that arrive as `error` events but are not app
 *  failures. The ResizeObserver one fires whenever a resize handler causes
 *  another reflow in the same frame; the spec calls it a warning. */
export function isBenignNotice(text: string): boolean {
	return /^ResizeObserver loop /i.test(text.trim());
}

function message(err: unknown): string {
	if (err instanceof Error && err.message) return err.message;
	if (typeof err === 'string' && err.trim()) return err;
	return m.common_error();
}

if (typeof window !== 'undefined') {
	window.addEventListener('error', (e) => {
		// Resource-load failures (a missing <img>/<script>) surface here with no
		// `error` object and a non-window target — those aren't app errors, skip them.
		if (!e.error && !e.message) return;
		const text = message(e.error ?? e.message);
		if (isBenignNotice(text)) return;
		toasts.error(text);
	});
	window.addEventListener('unhandledrejection', (e) => {
		const text = message(e.reason);
		if (isBenignNotice(text)) return;
		toasts.error(text);
	});
}

export const handleError: HandleClientError = ({ error }) => {
	const text = message(error);
	toasts.error(text);
	// Returned shape is exposed to the app as `page.error`; keep it message-only.
	return { message: text };
};
