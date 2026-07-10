<script lang="ts">
	// Free-form account env editor (CCT-591). The account-level `env_json` blob is
	// encrypted and WRITE-ONLY: the server returns the var NAMES (not values), so
	// this shows what is currently stored and lets the operator type arbitrary
	// NAME=VALUE pairs. Any well-formed name is accepted except a denylist of
	// session-critical / gateway-managed vars (the server is the real boundary;
	// these checks are fast feedback). Saving replaces the whole stored blob.
	import { Button, Input, Text } from '@dorsk/tsumikit';
	import Error from '$lib/components/atoms/Error.svelte';

	interface EnvRow {
		name: string;
		value: string;
	}

	let {
		envRows = $bindable([]),
		replaceEnv = $bindable(false),
		storedNames = []
	}: {
		envRows?: EnvRow[];
		replaceEnv?: boolean;
		storedNames?: string[];
	} = $props();

	// Mirror of the server denylist (crates/.../settings_catalog::ENV_DENYLIST):
	// session-critical / gateway-managed vars that must never be set per-account.
	const DENYLIST = new Set([
		'ANTHROPIC_BASE_URL',
		'ANTHROPIC_AUTH_TOKEN',
		'ANTHROPIC_API_KEY',
		'CLAUDE_CODE_SESSION_KIND',
		'CLAUDE_BG_SOURCE',
		'CLAUDE_BG_BACKEND',
		'CLAUDE_BG_CLAIM_AUTH'
	]);
	const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

	function nameError(name: string): string {
		const n = name.trim();
		if (!n) return '';
		if (!NAME_RE.test(n)) return `${n}: invalid name (use A–Z, 0–9, _)`;
		if (DENYLIST.has(n)) return `${n}: session-critical / gateway-managed — not allowed`;
		return '';
	}
	const nameErrors = $derived(envRows.map((r) => nameError(r.name)).filter((e) => e));

	function addEnvRow() {
		envRows = [...envRows, { name: '', value: '' }];
		replaceEnv = true;
	}
	function removeEnvRow(i: number) {
		envRows = envRows.filter((_, j) => j !== i);
		replaceEnv = true;
	}
	function onEdit() {
		replaceEnv = true;
	}
</script>

<div class="env-editor">
	<Text as="p" tone="faint" size="xs">
		Arbitrary environment applied to every session run under this account. Stored
		encrypted; values are never shown again. Curated Claude Code knobs live on
		the provider's settings instead.
	</Text>

	{#if storedNames.length}
		<div class="stored">
			<Text as="div" tone="muted" size="xs">Currently set (values hidden)</Text>
			<div class="chips">
				{#each storedNames as n (n)}<span class="chip">{n}</span>{/each}
			</div>
		</div>
	{/if}

	<Text as="div" tone="faint" size="xs">
		{#if replaceEnv}
			<Text as="span" tone="warn" size="xs">On save, the stored environment is replaced with the rows below (empty = cleared).</Text>
		{:else}
			<Text as="span" size="xs">Stored environment is left unchanged unless you edit a row.</Text>
		{/if}
	</Text>

	{#if envRows.length}
		<div class="env-rows">
			{#each envRows as row, i (i)}
				<div class="env-row">
					<Input
						bind:value={row.name}
						oninput={onEdit}
						mono
						placeholder="MY_TOKEN"
						aria-label="Env var name"
					/>
					<Input
						bind:value={row.value}
						oninput={onEdit}
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
	{#each nameErrors as e (e)}<Error>{e}</Error>{/each}
	<Button onclick={addEnvRow}>+ Add env var</Button>
</div>

<style>
	.env-editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		border-top: 1px solid var(--border);
		padding-top: var(--sp-3);
	}
	.stored {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
	.chip {
		font-family: var(--font-mono, monospace);
		font-size: var(--fs-xs);
		padding: 0.1em 0.5em;
		border-radius: var(--r-sm);
		background: var(--surface-2, color-mix(in srgb, var(--text) 8%, transparent));
	}
	.env-rows {
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
</style>
