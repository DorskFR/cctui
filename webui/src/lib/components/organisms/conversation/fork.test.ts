import { describe, it, expect } from 'vitest';
import { ForkController } from './fork.svelte';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { ForkRequest } from '@bindings/ForkRequest';

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

describe('ForkController extract (CCT-553)', () => {
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
