import type { SelfUpdateTarget } from '@bindings/SelfUpdateTarget';
import type { SelfUpdateTargetInfo } from '@bindings/SelfUpdateTargetInfo';

export type SelfUpdateCommit =
	| { kind: 'none' }
	| { kind: 'clear' }
	| { kind: 'save'; target: SelfUpdateTarget };

/**
 * What committing the self-update fields should do: save a complete target,
 * clear the saved one once the directory is emptied, or nothing while the
 * fields are incomplete or unchanged.
 */
export function selfUpdateCommit(
	current: SelfUpdateTargetInfo | null,
	draft: { machine: string; dir: string; adapter: string }
): SelfUpdateCommit {
	const saved = current?.target ?? null;
	const dir = draft.dir.trim();
	if (!dir) return current?.source === 'settings' ? { kind: 'clear' } : { kind: 'none' };
	if (!draft.machine) return { kind: 'none' };
	const unchanged =
		saved !== null &&
		saved.machine_id === draft.machine &&
		saved.working_dir === dir &&
		(saved.adapter_id ?? 'claude-code') === draft.adapter;
	if (unchanged) return { kind: 'none' };
	return { kind: 'save', target: { machine_id: draft.machine, working_dir: dir, adapter_id: draft.adapter } };
}
