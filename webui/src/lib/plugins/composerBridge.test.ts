import { describe, expect, it, vi } from 'vitest';
import { composerFor, registerComposer } from './composerBridge.svelte';
import type { ComposerBridge } from './types';

const fake = (): ComposerBridge => ({ insertText: vi.fn(), addFiles: vi.fn(), focus: vi.fn() });

describe('composer bridge', () => {
	it('routes calls to the composer registered for the session', () => {
		const a = fake();
		const off = registerComposer('s1', a);
		const handle = composerFor('s1');
		handle.insertText('hi');
		handle.focus();
		expect(a.insertText).toHaveBeenCalledWith('hi');
		expect(a.focus).toHaveBeenCalled();
		off();
		handle.insertText('gone');
		expect(a.insertText).toHaveBeenCalledTimes(1);
	});

	it('is a no-op for a session without a composer and keeps a newer registration', () => {
		expect(() => composerFor('nobody').insertText('x')).not.toThrow();
		const old = fake();
		const fresh = fake();
		const offOld = registerComposer('s2', old);
		registerComposer('s2', fresh);
		offOld();
		composerFor('s2').addFiles([]);
		expect(fresh.addFiles).toHaveBeenCalled();
		expect(old.addFiles).not.toHaveBeenCalled();
	});
});
