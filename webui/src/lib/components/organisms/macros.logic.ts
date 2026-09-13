import type { SpawnRequest } from '@bindings/SpawnRequest';
import type { MacroSpec } from '$lib/settings.svelte';

export const CLAUDE_EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'] as const;
export const CODEX_EFFORTS = ['minimal', 'low', 'medium', 'high'] as const;

/** Efforts a harness accepts (the spawn form's lists, minus the "default" blank). */
export const effortsFor = (adapter: string): readonly string[] =>
	adapter === 'codex' ? CODEX_EFFORTS : CLAUDE_EFFORTS;

export const newMacro = (id: string): MacroSpec => ({
	id,
	title: '',
	prompt: '',
	adapter: 'claude-code',
	machine_id: null,
	working_dir: null,
	model: null,
	effort: null,
	pool_id: null,
	permission_mode: null,
	confirm: true
});

/** Which knobs a macro is missing before it can run. */
export function macroProblems(mac: MacroSpec): ('title' | 'prompt' | 'machine' | 'cwd')[] {
	const out: ('title' | 'prompt' | 'machine' | 'cwd')[] = [];
	if (!mac.title.trim()) out.push('title');
	if (!mac.prompt.trim()) out.push('prompt');
	if (!mac.machine_id) out.push('machine');
	if (!mac.working_dir?.trim()) out.push('cwd');
	return out;
}

/** A macro's spawn payload: its knobs as the spawn form would send them, plus
 *  `auto_archive` so the server files the session away once it is done. */
export function spawnBodyFor(mac: MacroSpec): SpawnRequest {
	const pool = mac.pool_id?.trim() || null;
	return {
		machine_id: mac.machine_id ?? '',
		working_dir: (mac.working_dir ?? '').trim(),
		adapter_id: mac.adapter || 'claude-code',
		name: mac.title.trim() || null,
		prompt: mac.prompt.trim() || null,
		prompt_name: null,
		permission_mode: (mac.permission_mode as SpawnRequest['permission_mode']) || null,
		effort: mac.effort || null,
		service_tier: null,
		model: mac.model || null,
		env: {},
		account: null,
		provider: null,
		no_account: false,
		auto_account: !pool,
		pool,
		save_draft: false,
		auto_archive: true
	};
}
