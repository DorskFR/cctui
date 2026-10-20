import { describe, expect, it } from 'vitest';
import type { MachineRow } from '@bindings/MachineRow';
import type { MeResponse } from '@bindings/MeResponse';
import pane from '../ConversationPane.svelte?raw';
import { ownsSession } from './ownership';

const who = (role: string): MeResponse =>
	({ role, user_id: 'u1', user_name: null, machine_id: null, scopes: [], token_preview: '' }) as MeResponse;
const machine = (id: string) => ({ id, user_id: 'u1' }) as MachineRow;

describe('ownsSession', () => {
	it('is false until the viewer is known', () => {
		expect(ownsSession(undefined, [machine('m1')], 'm1')).toBe(false);
	});

	it('is true for a non-admin, who only ever sees their own sessions', () => {
		expect(ownsSession(who('user'), undefined, 'm9')).toBe(true);
	});

	it('for an admin, follows whether the session machine is one of theirs', () => {
		expect(ownsSession(who('admin'), [machine('m1')], 'm1')).toBe(true);
		expect(ownsSession(who('admin'), [machine('m1')], 'm2')).toBe(false);
		expect(ownsSession(who('admin'), undefined, 'm1')).toBe(false);
	});
});

describe('ConversationPane context attach', () => {
	it('offers Attach only on live sessions the viewer owns', () => {
		expect(pane).toContain('contextAttachable={!archived && owned}');
		expect(pane).toContain('ownsSession(me.data, myMachines.data, session.machine_id)');
	});
});
