<script lang="ts">
	// Per-account Claude Code settings editor (CCT-541). Renders inside the
	// account edit modal and edits three persisted-per-account surfaces:
	//   1. `settings_json` — a validated, allowlisted subset of Claude Code
	//      settings.json keys (SAFE/CARE only). Boolean keys get a grouped
	//      tri-state toggle list; an "Advanced" raw-JSON box merges anything else.
	//   2. extra `env_json` — a curated, allowlisted env-var editor. WRITE-ONLY:
	//      the server never returns stored values, so this only ever SETS new ones
	//      (with an explicit replace/clear affordance) and never displays secrets.
	//
	// The launch `defaults` surface (CCT-539) was removed with the CCT-558 schema
	// split — per-account launch defaults are superseded by per-(machine, cwd)
	// client memory (CCT-561).
	//
	// The catalog it renders from is a generated mirror of the server-side
	// settings_catalog (see $lib/settingsCatalog). The server re-validates every
	// write, so the client-side checks here are fast feedback, not the boundary.
	import { Button, Input, Select, Text } from '@dorsk/tsumikit';
	import Error from '$lib/components/atoms/Error.svelte';
	import {
		BOOL_KEYS,
		CATALOG_ENV,
		ENV_GROUPS,
		QUIET_DEFAULTS,
		SETTINGS_GROUPS,
		invalidEnvKeys,
		invalidSettingsKeys,
		isKnownBoolKey
	} from '$lib/settingsCatalog';

	interface EnvRow {
		name: string;
		value: string;
	}

	let {
		settings = $bindable({}),
		envRows = $bindable([]),
		// When true, `env_json` is sent on save (replacing/clearing the stored env);
		// when false the stored env is left untouched. Flipped on by any edit here.
		replaceEnv = $bindable(false),
		// CCT-560 split: `settings_json` lives on a provider row, `env_json` on the
		// account identity — the two edit modals each show only their half.
		showSettings = true,
		showEnv = true
	}: {
		settings?: Record<string, unknown>;
		envRows?: EnvRow[];
		replaceEnv?: boolean;
		showSettings?: boolean;
		showEnv?: boolean;
	} = $props();

	// --- Boolean settings tri-state (Default / On / Off) -----------------------
	// `undefined` in `settings` = inherit the Claude Code default; `true`/`false`
	// = an explicit account override.
	function triValue(name: string): '' | 'true' | 'false' {
		const v = settings[name];
		return v === true ? 'true' : v === false ? 'false' : '';
	}
	function setTri(name: string, raw: string) {
		const next = { ...settings };
		if (raw === 'true') next[name] = true;
		else if (raw === 'false') next[name] = false;
		else delete next[name];
		settings = next;
	}

	const keysInGroup = (group: string) => BOOL_KEYS.filter((k) => k.group === group);
	const envInGroup = (group: string) => CATALOG_ENV.filter((e) => e.group === group);

	// --- Advanced raw-JSON merge ----------------------------------------------
	let rawJson = $state('');
	let rawError = $state('');
	function applyRawJson() {
		rawError = '';
		const text = rawJson.trim();
		if (!text) return;
		let parsed: unknown;
		try {
			parsed = JSON.parse(text);
		} catch (e) {
			rawError = `Invalid JSON: ${(e as globalThis.Error).message}`;
			return;
		}
		if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
			rawError = 'Settings must be a JSON object.';
			return;
		}
		const obj = parsed as Record<string, unknown>;
		const bad = invalidSettingsKeys(obj);
		if (bad.length) {
			rawError = `Not settable per-account (MANAGED/SYSTEM/unknown): ${bad.join(', ')}`;
			return;
		}
		settings = { ...settings, ...obj };
		rawJson = '';
	}

	// Non-boolean settings keys currently set (edited only via the raw box) — shown
	// so the operator can see what advanced values are in effect.
	const advancedEntries = $derived(
		Object.entries(settings).filter(([k]) => !isKnownBoolKey(k))
	);
	function clearAdvancedKey(name: string) {
		const next = { ...settings };
		delete next[name];
		settings = next;
	}

	// --- Env editor ------------------------------------------------------------
	const badEnvKeys = $derived(invalidEnvKeys(envRows.map((r) => r.name)));
	function addEnvRow() {
		envRows = [...envRows, { name: '', value: '' }];
		replaceEnv = true;
	}
	function removeEnvRow(i: number) {
		envRows = envRows.filter((_, j) => j !== i);
		replaceEnv = true;
	}
	function onEnvEdit() {
		replaceEnv = true;
	}

	// --- Quiet defaults preset -------------------------------------------------
	// Applies only the visible halves (CCT-560): the settings preset in the
	// provider modal, the env preset in the identity modal.
	function applyQuietDefaults() {
		if (showSettings) settings = { ...settings, ...QUIET_DEFAULTS.settings };
		if (showEnv) {
			// Merge the preset env into the editor rows (replace matching names).
			const byName = new Map(envRows.map((r) => [r.name, r]));
			for (const [name, value] of Object.entries(QUIET_DEFAULTS.env)) {
				byName.set(name, { name, value });
			}
			envRows = [...byName.values()];
			replaceEnv = true;
		}
	}
</script>

<div class="settings-editor">
	<div class="head">
		<Text as="div" weight="semibold" size="sm">
			{showSettings && showEnv
				? 'Account settings & environment'
				: showSettings
					? 'Provider settings'
					: 'Extra environment'}
		</Text>
		<Button onclick={applyQuietDefaults}>Quiet defaults</Button>
	</div>
	<Text as="p" tone="faint" size="xs">
		{#if showSettings}
			Applied to every session run under this provider. Only low-risk settings
			are exposed; the server rejects org-managed keys.
		{:else}
			Applied to every session run under this account. Values are stored
			encrypted and never shown again.
		{/if}
	</Text>

	{#if showSettings}
	<!-- Boolean toggle list (CCT-541), grouped. Tri-state: Default / On / Off. -->
	<div class="block">
		<Text as="div" tone="muted" size="sm">Settings toggles</Text>
		<Text as="div" tone="faint" size="xs">
			"Disable web / published sessions" is NOT a per-account setting — it's an
			org-level toggle and can't be injected here. Only Remote Control
			(<Text variant="code">disableRemoteControl</Text>) is per-device/per-account.
		</Text>
		{#each SETTINGS_GROUPS as group (group)}
			<div class="group">
				<Text as="div" tone="faint" size="xs" class="group-title">{group}</Text>
				{#each keysInGroup(group) as k (k.name)}
					<div class="key-row">
						<div class="key-meta">
							<Text as="div" size="sm">
								{k.label}
								{#if k.tag === 'care'}<span class="care" title="Has caveats — set with care">care</span>{/if}
							</Text>
							<Text as="div" tone="faint" size="xs">
								{k.notes}{#if k.default}{' '}(default: {k.default}){/if}
							</Text>
						</div>
						<Select
							value={triValue(k.name)}
							onchange={(e) => setTri(k.name, (e.currentTarget as HTMLSelectElement).value)}
							aria-label={k.label}
							compact
						>
							<option value="">Default</option>
							<option value="true">On</option>
							<option value="false">Off</option>
						</Select>
					</div>
				{/each}
			</div>
		{/each}
	</div>

	<!-- Advanced raw-JSON paste box (CCT-541): merge any other allowlisted key. -->
	<div class="block">
		<Text as="div" tone="muted" size="sm">Advanced settings (raw JSON)</Text>
		<Text as="div" tone="faint" size="xs">
			Paste a settings.json fragment to merge in additional allowlisted keys
			(e.g. <Text variant="code">"editorMode": "vim"</Text>). MANAGED/SYSTEM keys
			are rejected.
		</Text>
		{#if advancedEntries.length}
			<div class="adv-list">
				{#each advancedEntries as [k, v] (k)}
					<div class="adv-item">
						<Text variant="code" size="xs">{k}: {JSON.stringify(v)}</Text>
						<Button onclick={() => clearAdvancedKey(k)} aria-label={`Remove ${k}`}>✕</Button>
					</div>
				{/each}
			</div>
		{/if}
		<Input
			bind:value={rawJson}
			placeholder={'{ "editorMode": "vim" }'}
			mono
			onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && applyRawJson()}
		/>
		{#if rawError}<Error>{rawError}</Error>{/if}
		<Button onclick={applyRawJson} disabled={!rawJson.trim()}>Merge JSON</Button>
	</div>
	{/if}

	{#if showEnv}
	<!-- Curated env editor (CCT-541). WRITE-ONLY: stored values are never returned,
	     so this only sets new ones. The block title dedupes against the editor
	     head when env is the only section shown (CCT-560). -->
	<div class="block">
		{#if showSettings}
			<Text as="div" tone="muted" size="sm">Extra environment (write-only)</Text>
		{/if}
		<Text as="div" tone="faint" size="xs">
			Only allowlisted vars. Values are stored encrypted and never shown again —
			this editor can only set or clear them, not display what's stored.
			{#if replaceEnv}
				<Text as="span" tone="warn" size="xs">On save, the stored environment will be replaced with the rows below (empty = cleared).</Text>
			{:else}
				<Text as="span" size="xs">Stored environment is left unchanged unless you edit a row.</Text>
			{/if}
		</Text>
		{#if envRows.length}
			<div class="env-rows">
				{#each envRows as row, i (i)}
					<div class="env-row">
						<Select bind:value={row.name} onchange={onEnvEdit} aria-label="Env var name">
							<option value="">Pick a variable…</option>
							{#each ENV_GROUPS as group (group)}
								<optgroup label={group}>
									{#each envInGroup(group) as ev (ev.name)}
										<option value={ev.name}>{ev.name}</option>
									{/each}
								</optgroup>
							{/each}
						</Select>
						<Input
							bind:value={row.value}
							oninput={onEnvEdit}
							type="password"
							mono
							placeholder="value"
							aria-label="Env var value"
						/>
						<Button variant="danger" onclick={() => removeEnvRow(i)} aria-label="Remove env var">✕</Button>
					</div>
				{/each}
			</div>
		{/if}
		{#if badEnvKeys.length}
			<Error>Not in the per-account allowlist: {badEnvKeys.join(', ')}</Error>
		{/if}
		<Button onclick={addEnvRow}>+ Add env var</Button>
	</div>
	{/if}
</div>

<style>
	.settings-editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		border-top: 1px solid var(--border);
		padding-top: var(--sp-3);
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.settings-editor :global(.group-title) {
		text-transform: uppercase;
		letter-spacing: 0.04em;
		margin-top: var(--sp-1);
	}
	.key-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.key-meta {
		min-width: 0;
		flex: 1;
	}
	.care {
		display: inline-block;
		margin-left: var(--sp-1);
		padding: 0 0.35em;
		border-radius: var(--r-sm);
		font-size: var(--fs-xs);
		background: color-mix(in srgb, var(--warn, #e0a800) 18%, transparent);
		color: var(--warn, #e0a800);
	}
	.env-rows,
	.adv-list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.env-row {
		display: grid;
		grid-template-columns: 1fr 1fr auto;
		gap: var(--sp-2);
		align-items: center;
	}
	.adv-item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
</style>
