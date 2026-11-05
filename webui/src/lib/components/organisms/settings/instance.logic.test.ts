import { describe, expect, it } from 'vitest';
import type { SelfUpdateTargetInfo } from '@bindings/SelfUpdateTargetInfo';
import { selfUpdateCommit } from './instance.logic';

const saved: SelfUpdateTargetInfo = {
	source: 'settings',
	target: { machine_id: 'm1', working_dir: '/srv/cctui', adapter_id: 'claude-code' }
};
const draft = (over: Partial<{ machine: string; dir: string; adapter: string }> = {}) => ({
	machine: 'm1',
	dir: '/srv/cctui',
	adapter: 'claude-code',
	...over
});

describe('selfUpdateCommit', () => {
	it('does nothing while the fields match what is saved', () => {
		expect(selfUpdateCommit(saved, draft())).toEqual({ kind: 'none' });
		expect(selfUpdateCommit(saved, draft({ dir: ' /srv/cctui ' }))).toEqual({ kind: 'none' });
	});

	it('saves a complete, changed target', () => {
		expect(selfUpdateCommit(saved, draft({ machine: 'm2' }))).toEqual({
			kind: 'save',
			target: { machine_id: 'm2', working_dir: '/srv/cctui', adapter_id: 'claude-code' }
		});
		expect(selfUpdateCommit(saved, draft({ adapter: 'codex' }))).toMatchObject({ kind: 'save' });
		expect(selfUpdateCommit({ source: null, target: null }, draft())).toMatchObject({ kind: 'save' });
	});

	it('waits for a machine before saving', () => {
		expect(selfUpdateCommit({ source: null, target: null }, draft({ machine: '' }))).toEqual({
			kind: 'none'
		});
	});

	it('clears a saved target once the directory is emptied', () => {
		expect(selfUpdateCommit(saved, draft({ dir: '  ' }))).toEqual({ kind: 'clear' });
	});

	it('never clears a target that comes from the environment', () => {
		const fromEnv: SelfUpdateTargetInfo = { ...saved, source: 'env' };
		expect(selfUpdateCommit(fromEnv, draft({ dir: '' }))).toEqual({ kind: 'none' });
	});
});
