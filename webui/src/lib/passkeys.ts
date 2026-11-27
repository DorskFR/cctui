/** A closed or timed-out system dialog is the person's choice, never an error. */
export function passkeyDismissed(e: unknown): boolean {
	return e instanceof DOMException && (e.name === 'NotAllowedError' || e.name === 'AbortError');
}

/** Adding a passkey was refused until the person signs in again. */
export function needsSignInAgain(e: unknown): boolean {
	return e instanceof Error && (e as Error & { status?: unknown }).status === 403;
}
