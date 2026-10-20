// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import PendingSpawnCard from './PendingSpawnCard.svelte';
import { pendingSpawns } from '$lib/pendingSpawns.svelte';

vi.mock('$lib/ws.svelte', () => ({
	ws: { onCommandResult: () => () => {}, onSessionEnded: () => () => {} }
}));

const flush = () => new Promise((r) => setTimeout(r, 0));

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	pendingSpawns.reset();
});

const text = () => document.body.textContent ?? '';

async function render(ondismiss = vi.fn()) {
	pendingSpawns.add({ commandId: 'cmd-1', sessionId: 'sess-1', machineId: 'm1', name: 'Fix the build', workingDir: '/w/repo' });
	const spawn = pendingSpawns.items[0];
	if (!spawn) throw new Error('no pending spawn');
	comp = mount(PendingSpawnCard, { target: document.body, props: { spawn, ondismiss } });
	await flush();
	return ondismiss;
}

describe('PendingSpawnCard', () => {
	it('shows the dispatched spawn as waiting, then as the failure the daemon reports', async () => {
		await render();
		expect(text()).toContain('Fix the build');
		expect(text()).toContain('/w/repo');
		expect(document.querySelector('[data-journey="pending-spawn"]')?.getAttribute('data-failed')).toBeNull();
		expect(document.querySelector('pre.detail')).toBeNull();

		pendingSpawns.resolve('cmd-1', false, 'worker abcd1234 never started: the claude daemon accepted the dispatch\n--- /tmp/claude-daemon.err.log ---\nEPERM');
		await flush();
		expect(document.querySelector('[data-journey="pending-spawn"]')?.getAttribute('data-failed')).toBe('true');
		expect(document.querySelector('pre.detail')?.textContent).toContain('/tmp/claude-daemon.err.log');
	});

	it('the swept spawn_failed end turns the card into the failure, any other end retires it', () => {
		pendingSpawns.add({ commandId: 'a', sessionId: 's-a', machineId: 'm', name: 'A', workingDir: '/a' });
		pendingSpawns.add({ commandId: 'b', sessionId: 's-b', machineId: 'm', name: 'B', workingDir: '/b' });
		pendingSpawns.ended('s-a', 'spawn_failed', 'spawn_failed: no acknowledgement from daemon within 120s');
		pendingSpawns.ended('s-b', 'completed', null);
		expect(pendingSpawns.items.map((p) => p.commandId)).toEqual(['a']);
		expect(pendingSpawns.items[0]?.failure).toContain('no acknowledgement');
	});

	it('a session that lands on the list retires its card, a failed one stays until dismissed', () => {
		pendingSpawns.add({ commandId: 'a', sessionId: 's-a', machineId: 'm', name: 'A', workingDir: '/a' });
		pendingSpawns.add({ commandId: 'b', sessionId: 's-b', machineId: 'm', name: 'B', workingDir: '/b' });
		pendingSpawns.resolve('b', false, 'boom');
		expect(pendingSpawns.visible(['s-a', 's-b']).map((p) => p.commandId)).toEqual(['b']);
		pendingSpawns.settle(['s-a', 's-b']);
		expect(pendingSpawns.items.map((p) => p.commandId)).toEqual(['b']);
		pendingSpawns.resolve('a', true);
		pendingSpawns.dismiss('b');
		expect(pendingSpawns.items).toEqual([]);
	});

	it('a spawn with no id yet adopts the session carrying its spawn key, on its machine only', () => {
		pendingSpawns.add({ commandId: 'cmd-x', sessionId: null, machineId: 'm', name: 'X', workingDir: '/x' });
		const listed = [
			{ id: 'other-machine', machine_id: 'n', metadata: { spawn_key: 'cmd-x' } },
			{ id: 'codex-1', machine_id: 'm', metadata: { spawn_key: 'cmd-x' } }
		];
		pendingSpawns.adopt([listed[0]!]);
		expect(pendingSpawns.items[0]?.sessionId).toBeNull();
		pendingSpawns.adopt(listed);
		pendingSpawns.settle(listed.map((s) => s.id));
		expect(pendingSpawns.items).toEqual([]);
	});

	it('dismiss hands the command id back', async () => {
		const ondismiss = await render();
		const button = [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) => /dismiss/i.test(b.textContent ?? ''));
		button?.click();
		expect(ondismiss).toHaveBeenCalledWith('cmd-1');
	});
});
