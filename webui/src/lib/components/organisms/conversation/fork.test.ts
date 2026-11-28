// @vitest-environment happy-dom
import { beforeEach, describe, it, expect, vi } from 'vitest';
import { ForkController } from './fork.svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { ForkRequest } from '@bindings/ForkRequest';
import type { CommandOutcome } from '#lib/ws.svelte.ts';
import { toasts } from '#lib/toast.svelte.ts';

function makeController(overrides: Partial<{ isCodex: boolean; archived: boolean }> = {}) {
	const session = {
		model: 'opus',
		effort: 'high',
		token_usage: {
			tokens_in: 1,
			tokens_out: 2,
			cache_read_tokens: 3,
			cache_creation_tokens: 4
		}
	} as unknown as SessionListItem;
	const calls: { id: string; body: ForkRequest }[] = [];
	const forked: (string | null | undefined)[] = [];
	const ctl = new ForkController({
		id: () => 'parent-1',
		archived: () => overrides.archived ?? false,
		isCodex: () => overrides.isCodex ?? false,
		session: () => session,
		fork: async (id, body) => {
			calls.push({ id, body });
			return { session_id: 'child-9' };
		},
		onForked: (sid) => forked.push(sid)
	});
	return { ctl, calls, forked };
}

describe('ForkController extract', () => {
	it('openDialog forks the whole history (extract null)', async () => {
		const { ctl, calls } = makeController();
		ctl.openDialog();
		expect(ctl.extract).toBeNull();
		expect(ctl.extractLabel).toBeNull();
		await ctl.submit();
		expect(calls[0].body.extract).toBeNull();
	});

	it('openExtract seeds up_to and carries the anchor into the fork body', async () => {
		const { ctl, calls } = makeController();
		ctl.openExtract({ mode: 'up_to', anchor_message_id: 'msg_abc', selected_message_ids: [] });
		expect(ctl.open).toBe(true);
		expect(ctl.extractLabel).toContain('from this message');
		await ctl.submit();
		expect(calls[0].body.extract).toEqual({
			mode: 'up_to',
			anchor_message_id: 'msg_abc',
			selected_message_ids: []
		});
	});

	it('labels after / selected modes distinctly', () => {
		const { ctl } = makeController();
		ctl.openExtract({ mode: 'after', anchor_message_id: 'msg_x', selected_message_ids: [] });
		expect(ctl.extractLabel).toContain('after this message');
		ctl.openExtract({
			mode: 'selected',
			anchor_message_id: null,
			selected_message_ids: ['msg_a', 'msg_b']
		});
		expect(ctl.extractLabel).toContain('2 selected');
	});

	it('pre-fills model/effort from the parent session', () => {
		const { ctl } = makeController();
		ctl.openExtract({ mode: 'up_to', anchor_message_id: 'msg_abc', selected_message_ids: [] });
		expect(ctl.model).toBe('opus');
		expect(ctl.effort).toBe('high');
	});
});

describe('ForkController spawn verdict', () => {
	beforeEach(() => toasts.reset());

	function tracked(outcome: CommandOutcome) {
		const waited: { commandId: string; sessionId: string | null | undefined }[] = [];
		const ctl = new ForkController({
			id: () => 'parent-1',
			archived: () => false,
			isCodex: () => false,
			session: () => ({ model: 'opus', effort: 'high' }) as unknown as SessionListItem,
			fork: async () => ({ command_id: 'c', session_id: 'child-9' }),
			onForked: () => {},
			awaitResult: async (commandId, sessionId) => {
				waited.push({ commandId, sessionId });
				return outcome;
			}
		});
		return { ctl, waited };
	}
	const errors = () => toasts.items.filter((t) => t.tone === 'error');

	it('toasts a fork the daemon rejected', async () => {
		const { ctl, waited } = tracked({ ok: false, error: 'x' });
		await ctl.submit();
		await vi.waitFor(() => expect(errors()).toHaveLength(1));
		expect(errors()[0].message).toContain('x');
		expect(waited).toEqual([{ commandId: 'c', sessionId: 'child-9' }]);
	});

	it('stays quiet when the verdict only timed out', async () => {
		const { ctl, waited } = tracked({ ok: false, timedOut: true });
		await ctl.submit();
		await vi.waitFor(() => expect(waited).toHaveLength(1));
		await Promise.resolve();
		expect(errors()).toHaveLength(0);
	});
});
