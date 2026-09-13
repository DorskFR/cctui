import { describe, expect, it } from 'vitest';
import { effortsFor, macroProblems, newMacro, spawnBodyFor } from './macros.logic';

describe('macros', () => {
	it('a fresh macro is missing everything but its id', () => {
		expect(macroProblems(newMacro('x'))).toEqual(['title', 'prompt', 'machine', 'cwd']);
	});

	it('the spawn body carries the knobs, binds the pool and asks for auto-archive', () => {
		const body = spawnBodyFor({
			...newMacro('m'),
			title: ' Nettoyage ',
			prompt: 'range ',
			machine_id: 'mach',
			working_dir: ' /srv ',
			model: 'opus',
			effort: 'high',
			pool_id: 'p1',
			permission_mode: 'yolo'
		});
		expect(body).toMatchObject({
			machine_id: 'mach',
			working_dir: '/srv',
			adapter_id: 'claude-code',
			name: 'Nettoyage',
			prompt: 'range',
			model: 'opus',
			effort: 'high',
			permission_mode: 'yolo',
			pool: 'p1',
			auto_account: false,
			auto_archive: true,
			save_draft: false
		});
	});

	it('no pool means the server picks the account', () => {
		const body = spawnBodyFor({ ...newMacro('m'), machine_id: 'a', working_dir: '/x' });
		expect(body.pool).toBeNull();
		expect(body.auto_account).toBe(true);
	});

	it('efforts follow the harness', () => {
		expect(effortsFor('codex')).toContain('minimal');
		expect(effortsFor('claude-code')).toContain('max');
	});
});
