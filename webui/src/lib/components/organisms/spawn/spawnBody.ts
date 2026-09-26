import type { SpawnRequest } from '@bindings/SpawnRequest';
import { normalizeDir } from '$lib/drafts';
import { FOLLOWUP_RELATION } from '$lib/followup';
import { isCompatibleProvider, NO_ACCOUNT, poolName } from './options';
import type { EnvRow, Form } from './types';

/** Complete rows only (both key and value set). */
export function envMap(rows: EnvRow[]): Record<string, string> {
	const out: Record<string, string> = {};
	for (const r of rows) {
		const k = r.key.trim();
		if (k && r.value) out[k] = r.value;
	}
	return out;
}

export function buildSpawnBody(
	f: Form,
	spawnProvider: string | undefined,
	env: Record<string, string>,
	followupParent: string | null
): SpawnRequest {
	const adapter = f.adapter_id;
	const noAccount = f.account === NO_ACCOUNT;
	const pool = poolName(f.account) ?? null;
	const compatible = !!spawnProvider && isCompatibleProvider(spawnProvider);
	const model = compatible
		? f.model_account || null
		: (adapter === 'codex' ? f.model_codex : f.model_claude) || null;
	return {
		machine_id: f.machine_id,
		working_dir: normalizeDir(f.working_dir.trim()),
		adapter_id: adapter,
		name: f.name.trim() || null,
		prompt: f.prompt.trim() || null,
		prompt_name: null,
		// null lets the server resolve the account default permission mode.
		permission_mode: f.permission_mode || null,
		effort: (adapter === 'codex' ? f.effort_codex : f.effort_claude) || null,
		service_tier: (adapter === 'codex' && f.service_tier) || null,
		model,
		env,
		account: noAccount || pool ? null : f.account.trim() || null,
		provider: noAccount || pool ? null : spawnProvider || null,
		no_account: noAccount,
		// "Auto" delegates the choice to the server; a pool is the bounded form.
		auto_account: !noAccount && !pool && !f.account.trim(),
		pool,
		save_draft: false,
		// Attached by the server once the session registers, so a draft
		// launched later keeps them too.
		label_ids: [...f.labels],
		relation: followupParent ? FOLLOWUP_RELATION : null,
		parent_session_id: followupParent
	};
}

/** A draft carries env keys and attachment names, never values or bytes. */
export function draftBody(spawn: SpawnRequest, rows: EnvRow[], files: File[]): SpawnRequest {
	return {
		...spawn,
		env: {},
		env_keys: rows.map((r) => r.key.trim()).filter(Boolean),
		attachment_names: files.map((f) => f.name)
	};
}
