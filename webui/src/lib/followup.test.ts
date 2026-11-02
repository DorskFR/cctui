import { describe, expect, it, vi } from 'vitest';
import type { SessionListItem } from '@bindings/SessionListItem';
import {
	BRIEF_FILE_NAME,
	INLINE_BRIEF_MAX_BYTES,
	clampFollowupWhenCold,
	followupPrefill,
	followupPrompt,
	followupSpawnRequest,
	launchFollowup,
	migrateFollowupWhenCold,
	nestsUnderParent,
	previousSessionId,
	resolveFollowupWhenCold,
	routeEnter,
	showColdOffer,
	type FollowupLaunchDeps,
	type FollowupWhenCold
} from './followup';
import { splitInjected } from '$lib/components/organisms/conversation/format';

const src = { id: 's1', name: 'my "run"', working_dir: '/w' };

describe('followupPrompt', () => {
	it('wraps the brief in the namespaced tag, then the instruction with no separator', () => {
		const { prompt, file } = followupPrompt(src, '**User:**\n\nq', 'Now add tests.');
		expect(file).toBeNull();
		expect(prompt).toBe(
			'<cctui-previous-conversation session="my &quot;run&quot;" id="s1">\n**User:**\n\nq\n</cctui-previous-conversation>\n\nNow add tests.'
		);
		expect(prompt).not.toContain('===');
	});

	it('splits back into one previous-conversation block and the ask', () => {
		const { prompt } = followupPrompt(src, '**User:**\n\nq', 'Now add tests.');
		const { blocks, prose } = splitInjected(prompt);
		expect(blocks).toHaveLength(1);
		expect(blocks[0]).toMatchObject({
			kind: 'previous-conversation',
			attrs: { session: 'my "run"', id: 's1' },
			body: '**User:**\n\nq'
		});
		expect(prose).toBe('Now add tests.');
	});

	it('defaults the instruction and falls back to the cwd for an unnamed session', () => {
		const { prompt } = followupPrompt({ ...src, name: null }, 'b', '  ');
		expect(prompt.startsWith('<cctui-previous-conversation session="/w" id="s1">')).toBe(true);
		expect(prompt.endsWith('</cctui-previous-conversation>\n\nContinue from this.')).toBe(true);
	});

	it('moves an oversized brief into the bootstrap file', () => {
		const brief = 'x'.repeat(INLINE_BRIEF_MAX_BYTES + 1);
		const { prompt, file } = followupPrompt(src, brief);
		expect(prompt).toContain(BRIEF_FILE_NAME);
		expect(prompt).not.toContain(brief);
		expect(prompt).not.toContain('===');
		expect(file).toContain(brief);
		expect(file?.startsWith('<cctui-previous-conversation session="my &quot;run&quot;" id="s1">')).toBe(true);
		expect(file?.endsWith('</cctui-previous-conversation>\n')).toBe(true);
	});
});

describe('followupPrefill', () => {
	const session = {
		id: 's1',
		name: 'n',
		working_dir: '/w',
		machine_id: 'm1',
		adapter_id: 'codex',
		model: 'gpt-5.6',
		effort: 'high',
		account_name: 'acct',
		permission_mode: null,
		labels: [{ id: 'l1' }, { id: 'l2' }]
	} as unknown as SessionListItem;

	it('carries the source config and the follow-up linkage', () => {
		const p = followupPrefill(session, 'brief', { archiveSource: true });
		expect(p).toMatchObject({
			machine_id: 'm1',
			working_dir: '/w',
			adapter_id: 'codex',
			model_codex: 'gpt-5.6',
			effort_codex: 'high',
			account: 'acct',
			label_ids: 'l1,l2',
			relation: 'followup',
			parent_session_id: 's1',
			archive_source: '1'
		});
		expect(p.prompt).toContain('brief');
		expect(p).not.toHaveProperty('permission_mode');
		expect(p).not.toHaveProperty('followup_file');
	});
});

describe('settings routing', () => {
	it('clamps unknown values to auto', () => {
		expect(clampFollowupWhenCold(undefined)).toBe('auto');
		expect(clampFollowupWhenCold('nope')).toBe('auto');
		expect(clampFollowupWhenCold('default')).toBe('auto');
		for (const v of ['auto', 'modal', 'offer', 'off']) expect(clampFollowupWhenCold(v)).toBe(v);
	});

	it('migrates the stored pre-auto values: offer to auto, default to modal, unknown to auto', () => {
		expect(migrateFollowupWhenCold('offer')).toBe('auto');
		expect(migrateFollowupWhenCold('default')).toBe('modal');
		expect(migrateFollowupWhenCold('off')).toBe('off');
		expect(migrateFollowupWhenCold('nope')).toBe('auto');
		expect(migrateFollowupWhenCold(undefined)).toBe('auto');
	});

	it('reads the legacy value only while the new one is absent', () => {
		expect(resolveFollowupWhenCold(undefined, 'default')).toBe('modal');
		expect(resolveFollowupWhenCold(undefined, 'offer')).toBe('auto');
		expect(resolveFollowupWhenCold('offer', 'default')).toBe('offer');
		expect(resolveFollowupWhenCold('bogus', 'default')).toBe('auto');
	});

	it('routes Enter for every setting while cold, and always sends in place when warm or shifted', () => {
		const cold: Record<FollowupWhenCold, string> = {
			auto: 'launch',
			modal: 'followup',
			offer: 'send',
			off: 'send'
		};
		for (const [setting, route] of Object.entries(cold) as [FollowupWhenCold, string][]) {
			expect(routeEnter(setting, true, false)).toBe(route);
			expect(routeEnter(setting, true, true)).toBe('send');
			expect(routeEnter(setting, false, false)).toBe('send');
			expect(routeEnter(setting, false, true)).toBe('send');
		}
	});

	it('offers only when cold, not dismissed and not off', () => {
		expect(showColdOffer('auto', true, false)).toBe(true);
		expect(showColdOffer('modal', true, false)).toBe(true);
		expect(showColdOffer('offer', true, false)).toBe(true);
		expect(showColdOffer('off', true, false)).toBe(false);
		expect(showColdOffer('auto', true, true)).toBe(false);
		expect(showColdOffer('auto', false, false)).toBe(false);
	});
});

describe('followupSpawnRequest', () => {
	const source = {
		id: 's1',
		name: 'n',
		working_dir: '/w',
		machine_id: 'm1',
		adapter_id: 'claude-code',
		model: 'opus',
		effort: 'high',
		account_name: 'acct',
		permission_mode: 'yolo',
		labels: [{ id: 'l1' }]
	} as unknown as SessionListItem;

	it('spawns on the source machine, cwd, model and account, linked as its follow-up', () => {
		const { body, files } = followupSpawnRequest(followupPrefill(source, 'brief', { instruction: 'go on' }));
		expect(body).toMatchObject({
			machine_id: 'm1',
			working_dir: '/w',
			adapter_id: 'claude-code',
			model: 'opus',
			effort: 'high',
			account: 'acct',
			auto_account: false,
			permission_mode: 'yolo',
			label_ids: ['l1'],
			relation: 'followup',
			parent_session_id: 's1'
		});
		expect(body.prompt?.endsWith('go on')).toBe(true);
		expect(files).toEqual([]);
	});

	it('lets the server pick an account when the source had none, and ships an oversized brief as a file', () => {
		const brief = 'x'.repeat(INLINE_BRIEF_MAX_BYTES + 1);
		const bare = { ...source, account_name: null, permission_mode: 'nope' } as unknown as SessionListItem;
		const { body, files } = followupSpawnRequest(followupPrefill(bare, brief));
		expect(body.account).toBeNull();
		expect(body.auto_account).toBe(true);
		expect(body.permission_mode).toBeNull();
		expect(files.map((f) => f.name)).toEqual([BRIEF_FILE_NAME]);
	});
});

describe('launchFollowup', () => {
	const source = {
		id: 'src',
		name: 'n',
		working_dir: '/w',
		machine_id: 'm1',
		adapter_id: 'claude-code',
		labels: []
	} as unknown as SessionListItem;

	function deps(over: Partial<FollowupLaunchDeps> = {}) {
		return {
			brief: vi.fn(async () => ({ markdown: 'brief' })),
			spawn: vi.fn(async () => ({ command_id: 'c1', status: 'dispatched', session_id: 'new' })),
			awaitSpawn: vi.fn(async () => ({ ok: true })),
			archive: vi.fn(async () => {}),
			...over
		};
	}

	it('spawns with the draft as instruction, then archives the source', async () => {
		const d = deps();
		const out = await launchFollowup(source, 'next step', d);
		expect(out).toEqual({ status: 'launched', sessionId: 'new' });
		const body = vi.mocked(d.spawn).mock.calls[0][0];
		expect(body.prompt?.endsWith('next step')).toBe(true);
		expect(body.parent_session_id).toBe('src');
		expect(d.awaitSpawn).toHaveBeenCalledWith('c1', 'new');
		expect(d.archive).toHaveBeenCalledWith('src');
	});

	it('does not archive when the spawn fails', async () => {
		const d = deps({ awaitSpawn: vi.fn(async () => ({ ok: false, error: 'boom' })) });
		expect(await launchFollowup(source, 'x', d)).toEqual({ status: 'failed', error: 'boom' });
		expect(d.archive).not.toHaveBeenCalled();
	});

	it('does not archive when the spawn request or the brief is rejected', async () => {
		const err = new Error('500');
		const rejected = deps({ spawn: vi.fn(async () => Promise.reject(err)) });
		expect(await launchFollowup(source, 'x', rejected)).toEqual({ status: 'failed', error: err });
		expect(rejected.archive).not.toHaveBeenCalled();
		const noBrief = deps({ brief: vi.fn(async () => Promise.reject(err)) });
		expect((await launchFollowup(source, 'x', noBrief)).status).toBe('failed');
		expect(noBrief.spawn).not.toHaveBeenCalled();
		expect(noBrief.archive).not.toHaveBeenCalled();
	});

	it('leaves the source alone while the spawn is unconfirmed', async () => {
		const d = deps({ awaitSpawn: vi.fn(async () => ({ ok: false, timedOut: true })) });
		expect(await launchFollowup(source, 'x', d)).toEqual({
			status: 'unconfirmed',
			sessionId: 'new',
			commandId: 'c1'
		});
		expect(d.archive).not.toHaveBeenCalled();
	});

	it('takes the id the probe found for an adapter without a pre-minted one', async () => {
		const d = deps({
			spawn: vi.fn(async () => ({ command_id: 'c1', status: 'dispatched', session_id: null })),
			awaitSpawn: vi.fn(async () => ({ ok: true, sessionId: 'probed' }))
		});
		expect(await launchFollowup(source, 'x', d)).toEqual({ status: 'launched', sessionId: 'probed' });
	});

	it('reports a failed archive without failing the launch', async () => {
		const err = new Error('403');
		const d = deps({ archive: vi.fn(async () => Promise.reject(err)) });
		expect(await launchFollowup(source, 'x', d)).toEqual({
			status: 'launched',
			sessionId: 'new',
			archiveError: err
		});
	});
});

describe('thread links', () => {
	const at = (parent_id: string | null, relation?: string) => ({
		parent_id,
		metadata: relation ? { relation } : null
	});

	it('nests only a parented session that is neither a fork nor a follow-up', () => {
		expect(nestsUnderParent(at('p'))).toBe(true);
		expect(nestsUnderParent(at('p', 'subagent'))).toBe(true);
		expect(nestsUnderParent(at('p', 'fork'))).toBe(false);
		expect(nestsUnderParent(at('p', 'followup'))).toBe(false);
		expect(nestsUnderParent(at(null))).toBe(false);
	});

	it('links back only from a follow-up', () => {
		expect(previousSessionId(at('p', 'followup'))).toBe('p');
		expect(previousSessionId(at('p', 'fork'))).toBeNull();
		expect(previousSessionId(at('p'))).toBeNull();
	});
});
