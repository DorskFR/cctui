/**
 * Browser half of the passkey (WebAuthn) ceremonies.
 *
 * The server hands us the W3C options as JSON, which means every binary field
 * (the challenge, the user handle, each credential id) arrives base64url-encoded
 * and has to become an `ArrayBuffer` before `navigator.credentials` will look at
 * it — and the authenticator's answer has to make the same trip back.
 *
 * `PublicKeyCredential.parseCreationOptionsFromJSON` / `toJSON` do exactly this
 * natively, but only on recent browsers. The conversions are twenty lines, so we
 * do them ourselves and stay compatible with everything that has WebAuthn at
 * all.
 */

/** Decode one base64url string into the bytes `navigator.credentials` wants. */
export function fromBase64Url(value: string): ArrayBuffer {
	const padded = value.replace(/-/g, '+').replace(/_/g, '/');
	const raw = atob(padded.padEnd(Math.ceil(padded.length / 4) * 4, '='));
	const bytes = new Uint8Array(raw.length);
	for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
	return bytes.buffer;
}

/** Encode bytes from the authenticator back into the base64url the server reads. */
export function toBase64Url(value: ArrayBuffer): string {
	const bytes = new Uint8Array(value);
	let raw = '';
	for (const b of bytes) raw += String.fromCharCode(b);
	return btoa(raw).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** Whether this browser can do WebAuthn at all. Notably false in a plain-http
 *  context and inside some in-app webviews, which is why every entry point
 *  checks rather than assuming. */
export function passkeysSupported(): boolean {
	return typeof window !== 'undefined' && !!window.PublicKeyCredential;
}

/** Whether the browser can surface passkeys inside the token field's
 *  autocomplete (conditional mediation) rather than throwing a modal. */
export async function conditionalUiSupported(): Promise<boolean> {
	if (!passkeysSupported()) return false;
	try {
		return await PublicKeyCredential.isConditionalMediationAvailable();
	} catch {
		return false;
	}
}

/** Raw JSON options as the server serializes them: binary fields are strings. */
type JsonOptions = Record<string, unknown>;

function decodeCreateOptions(options: JsonOptions): PublicKeyCredentialCreationOptions {
	const pk = (options.publicKey ?? options) as JsonOptions;
	const user = pk.user as { id: string; name: string; displayName: string };
	const exclude = (pk.excludeCredentials ?? []) as Array<{ id: string; type: string }>;
	return {
		...(pk as unknown as PublicKeyCredentialCreationOptions),
		challenge: fromBase64Url(pk.challenge as string),
		user: { ...user, id: fromBase64Url(user.id) },
		excludeCredentials: exclude.map((c) => ({
			...c,
			id: fromBase64Url(c.id),
			type: 'public-key' as const
		}))
	};
}

function decodeRequestOptions(options: JsonOptions): PublicKeyCredentialRequestOptions {
	const pk = (options.publicKey ?? options) as JsonOptions;
	const allow = (pk.allowCredentials ?? []) as Array<{ id: string; type: string }>;
	return {
		...(pk as unknown as PublicKeyCredentialRequestOptions),
		challenge: fromBase64Url(pk.challenge as string),
		allowCredentials: allow.map((c) => ({
			...c,
			id: fromBase64Url(c.id),
			type: 'public-key' as const
		}))
	};
}

/** The registration answer, in the shape webauthn-rs deserializes. */
function encodeAttestation(cred: PublicKeyCredential): Record<string, unknown> {
	const response = cred.response as AuthenticatorAttestationResponse;
	return {
		id: cred.id,
		rawId: toBase64Url(cred.rawId),
		type: cred.type,
		response: {
			clientDataJSON: toBase64Url(response.clientDataJSON),
			attestationObject: toBase64Url(response.attestationObject),
			transports: response.getTransports?.() ?? undefined
		},
		clientExtensionResults: cred.getClientExtensionResults()
	};
}

/** The assertion answer, in the shape webauthn-rs deserializes. */
function encodeAssertion(cred: PublicKeyCredential): Record<string, unknown> {
	const response = cred.response as AuthenticatorAssertionResponse;
	return {
		id: cred.id,
		rawId: toBase64Url(cred.rawId),
		type: cred.type,
		response: {
			clientDataJSON: toBase64Url(response.clientDataJSON),
			authenticatorData: toBase64Url(response.authenticatorData),
			signature: toBase64Url(response.signature),
			userHandle: response.userHandle ? toBase64Url(response.userHandle) : null
		},
		clientExtensionResults: cred.getClientExtensionResults()
	};
}

/** Thrown when the person dismissed the system dialog. The caller treats this
 *  as "never mind", not as a failure worth an error message. */
export class PasskeyAborted extends Error {}

function rethrow(e: unknown): never {
	// `NotAllowedError` is what every browser reports both for an explicit
	// cancel and for a timeout; either way the user chose not to continue.
	if (e instanceof DOMException && (e.name === 'NotAllowedError' || e.name === 'AbortError')) {
		throw new PasskeyAborted(e.message);
	}
	throw e;
}

/** Run the creation ceremony. Returns the credential to POST back, plus what
 *  the browser said about the credential being discoverable (`credProps.rk`) —
 *  the server records it so a key that cannot drive the login says so. */
export async function createPasskey(
	options: JsonOptions
): Promise<{ credential: Record<string, unknown>; discoverable: boolean | null }> {
	try {
		const cred = (await navigator.credentials.create({
			publicKey: decodeCreateOptions(options)
		})) as PublicKeyCredential | null;
		if (!cred) throw new PasskeyAborted('no credential returned');
		const props = cred.getClientExtensionResults().credProps;
		return { credential: encodeAttestation(cred), discoverable: props?.rk ?? null };
	} catch (e) {
		rethrow(e);
	}
}

/** Run the assertion ceremony.
 *
 * `mediation` is ours to choose, not the server's: the same challenge drives the
 * click-a-button modal (`undefined`) and the silent autofill flow
 * (`'conditional'`), so the server always sends the same options and the caller
 * picks the interaction. `signal` lets a pending conditional request be dropped
 * when the user starts typing a token instead.
 */
export async function getAssertion(
	options: JsonOptions,
	mediation?: CredentialMediationRequirement,
	signal?: AbortSignal
): Promise<Record<string, unknown>> {
	try {
		const cred = (await navigator.credentials.get({
			publicKey: decodeRequestOptions(options),
			mediation,
			signal
		})) as PublicKeyCredential | null;
		if (!cred) throw new PasskeyAborted('no assertion returned');
		return encodeAssertion(cred);
	} catch (e) {
		rethrow(e);
	}
}
