import {
	drafts,
	readSpawnSlot,
	type SpawnSlotPayload,
	LAST_SPAWN_NAME,
	LAST_SPAWN_LABELS,
	nextSessionName
} from '$lib/drafts';
import type { EnvRow, Form, SpawnPrefill } from './types';

export const blank: Form = {
	machine_id: '',
	adapter_id: 'claude-code',
	working_dir: '',
	name: '',
	prompt: '',
	permission_mode: '' as Form['permission_mode'],
	dispatcher: '',
	dispatch_adapter: 'claude-code',
	identity: '',
	repo: '',
	ticket: '',
	prompt_file: '',
	model_claude: '',
	model_codex: '',
	model_account: '',
	account: '',
	account_provider: '',
	effort_claude: '',
	effort_codex: '',
	service_tier: '',
	timeout: '',
	context_pack_url: '',
	context_pack_ref: '',
	context_pack_subdir: '',
	context_pack_token: '',
	labels: []
};

export interface Seed {
	form: Form;
	draftId: string | null;
	/** A restored slot or an "Edit draft" prefill: memory recall must not overwrite it. */
	loadedDraft: boolean;
	/** Values never come back from disk: only the keys are re-proposed. */
	envRows: EnvRow[];
}

function stripPrefill(prefill: SpawnPrefill | null) {
	const {
		draft_id,
		env_keys,
		label_ids,
		relation: _r,
		parent_session_id: _p,
		archive_source: _a,
		followup_file: _f,
		...form
	} = prefill ?? {};
	return { draft_id, env_keys, label_ids, form };
}

/** Seed the form from its local slot and the prefill. Non-empty prefill
 * values win over the slot; empty ones never clear what the slot holds. */
export function seedForm(prefill: SpawnPrefill | null, slotKey: string): Seed {
	try {
		const saved: SpawnSlotPayload = readSpawnSlot(slotKey) ?? {};
		const raw = Object.keys(saved).length > 0;
		const { draft_id, env_keys, label_ids, form: prefillForm } = stripPrefill(prefill);
		const keys = new Set<string>();
		for (const r of saved.envRows ?? []) if (r?.key) keys.add(String(r.key));
		for (const k of env_keys?.split(',') ?? []) if (k) keys.add(k);
		const { envRows: _envRows, draftId: _draftId, attachmentNames: _names, ...savedForm } = saved;
		const given = Object.fromEntries(
			Object.entries(prefillForm).filter(([, v]) => v !== '' && v != null)
		);
		const form = { ...blank, ...savedForm, ...given } as Form;
		if (label_ids !== undefined) form.labels = label_ids.split(',').filter(Boolean);
		// Fresh open: propose the last submitted name with a bumped suffix and
		// the last-used label set.
		if (!raw && !prefill) {
			const lastName = drafts.get(LAST_SPAWN_NAME);
			if (lastName) form.name = nextSessionName(lastName);
			if (!savedForm.labels) {
				form.labels = (drafts.get(LAST_SPAWN_LABELS) ?? '').split(',').filter(Boolean);
			}
		}
		return {
			form,
			draftId: draft_id ?? saved.draftId ?? null,
			loadedDraft: (raw || !!draft_id) && !(prefill && !draft_id),
			envRows: [...keys].map((key) => ({ key, value: '' }))
		};
	} catch {
		return {
			form: { ...blank, ...stripPrefill(prefill).form } as Form,
			draftId: null,
			loadedDraft: false,
			envRows: []
		};
	}
}
