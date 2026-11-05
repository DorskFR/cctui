// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';

const { auth } = vi.hoisted(() => ({
	auth: {
		passkeyConfig: vi.fn(),
		loginWithPasskey: vi.fn(),
		login: vi.fn()
	}
}));
vi.mock('$lib/auth.svelte', () => ({ auth }));
vi.mock('$lib/passkeys', () => ({
	PasskeyAborted: class PasskeyAborted extends Error {},
	conditionalUiSupported: async () => false
}));

import Login from './Login.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
});

beforeEach(() => {
	auth.passkeyConfig.mockResolvedValue({ available: true, enrolled: true, auto_prompt: true });
	auth.loginWithPasskey.mockReset();
});

async function settle() {
	for (let i = 0; i < 5; i++) {
		await tick();
		await Promise.resolve();
	}
	flushSync();
}

const passkeyFailedText = 'That passkey was not accepted';

describe('Login passkey errors', () => {
	it('stays quiet when the automatic passkey prompt fails before any user action', async () => {
		auth.loginWithPasskey.mockRejectedValue(new Error('NotAllowedError'));
		comp = mount(Login, { target: document.body });
		await settle();
		expect(auth.loginWithPasskey).toHaveBeenCalled();
		expect(document.body.textContent).not.toContain(passkeyFailedText);
	});

	it('stays quiet when the automatic prompt resolves without signing in', async () => {
		auth.loginWithPasskey.mockResolvedValue(false);
		comp = mount(Login, { target: document.body });
		await settle();
		expect(document.body.textContent).not.toContain(passkeyFailedText);
	});

	it('reports a failure the user caused by pressing the passkey button', async () => {
		auth.passkeyConfig.mockResolvedValue({ available: true, enrolled: true, auto_prompt: false });
		auth.loginWithPasskey.mockRejectedValue(new Error('NotAllowedError'));
		comp = mount(Login, { target: document.body });
		await settle();
		const button = [...document.querySelectorAll('button')].find((b) =>
			b.textContent?.includes('passkey')
		) as HTMLButtonElement;
		button.click();
		await settle();
		expect(document.body.textContent).toContain(passkeyFailedText);
	});
});
