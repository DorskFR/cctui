import type { ClientInit, HandleClientError } from '@sveltejs/kit/hooks';

// App-wide error surfacing. Any uncaught error — a synchronous throw, an
// unhandled promise rejection, or an error SvelteKit catches while
// navigating/rendering — becomes an error toast, so a failure never blanks a
// view silently. The browser console still gets the full error + stack for
// debugging; the toast stays terse and carries no internals.
import { toasts } from '#lib/toast.svelte.ts';
import { m } from '#lib/paraglide/messages.js';
import { isStaleChunkError, recoverFromStaleChunk } from '#lib/staleChunk.ts';
import { initTelemetry } from '#lib/telemetry.ts';

function swallowedAsStaleChunk(text: string): boolean {
	return isStaleChunkError(text) && recoverFromStaleChunk();
}

/** Benign browser notices that arrive as `error` events but are not app
 *  failures. The ResizeObserver one fires whenever a resize handler causes
 *  another reflow in the same frame; the spec calls it a warning. */
export function isBenignNotice(text: string): boolean {
	return (/^ResizeObserver loop /i).test(text.trim());
}

const KIT_SCROLL_CAPTURE = [
	/^can't access property "scroll", \w+\[\w+\] is undefined$/,
	/^Cannot set properties of undefined \(setting 'scroll'\)$/,
	/^undefined is not an object \(evaluating '\w+\[\w+\]\.scroll'\)$/
];

/** SvelteKit's router writes `history_info[index].scroll` without checking the
 *  entry exists; it throws when the browser restored an entry the router has
 *  no record of. Nothing breaks for the user, so it is logged, not toasted. */
export function isKitScrollCaptureError(err: unknown): boolean {
	const text = err instanceof Error ? err.message : typeof err === 'string' ? err : '';
	if (err instanceof Error && err.name !== 'TypeError') return false;
	const bare = text.trim().replace(/^(Uncaught )?TypeError: /, '');
	return KIT_SCROLL_CAPTURE.some((re) => re.test(bare));
}

function ignored(err: unknown): boolean {
	if (!isKitScrollCaptureError(err)) return false;
	console.warn('ignored SvelteKit scroll capture error:', err);
	return true;
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
		if (ignored(e.error ?? e.message)) return;
		const text = message(e.error ?? e.message);
		if (isBenignNotice(text) || swallowedAsStaleChunk(text)) return;
		toasts.error(text);
	});
	window.addEventListener('unhandledrejection', (e) => {
		if (ignored(e.reason)) return;
		const text = message(e.reason);
		if (isBenignNotice(text) || swallowedAsStaleChunk(text)) return;
		toasts.error(text);
	});
	// Vite preempts the rejection for its own preloads; without this the failure
	// is only reachable as an unhandled `vite:preloadError`.
	window.addEventListener('vite:preloadError', ((e: Event) => {
		if (recoverFromStaleChunk()) e.preventDefault();
	}) as EventListener);
}

export const init: ClientInit = () => {
	void initTelemetry();
};

export const handleError: HandleClientError = ({ error, kind }) => {
	// The error page already shows these; a toast would repeat it.
	if (kind === 'app' || (kind === 'framework' && error.status === 404)) return;
	const text = message(error);
	if (!ignored(error) && !swallowedAsStaleChunk(text)) toasts.error(text);
	// Returned shape is exposed to the app as `page.error`; keep it message-only.
	return { message: text };
};
