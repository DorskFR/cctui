<script lang="ts">
	// Free-form account env editor. The account-level `env_json` blob is
	// encrypted and WRITE-ONLY: the server returns the var NAMES (not values), so
	// this shows what is currently stored and lets the operator type arbitrary
	// NAME=VALUE pairs. Any well-formed name is accepted except a denylist of
	// session-critical / gateway-managed vars (the server is the real boundary;
	// these checks are fast feedback). Saving replaces the whole stored blob;
	// stored names can also be deleted individually via `env_remove` (the server
	// drops them from the decrypted blob without the other values round-tripping).
	import { Button, Input, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import Error from '$lib/components/atoms/Error.svelte';

	interface EnvRow {
		name: string;
		value: string;
	}

	let {
		envRows = $bindable([]),
		replaceEnv = $bindable(false),
		envRemove = $bindable([]),
		storedNames = []
	}: {
		envRows?: EnvRow[];
		replaceEnv?: boolean;
		envRemove?: string[];
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
		if (!NAME_RE.test(n)) return m.providers_env_err_invalid_name({ name: n });
		if (DENYLIST.has(n)) return m.providers_env_err_denied({ name: n });
		return '';
	}
	const nameErrors = $derived(envRows.map((r) => nameError(r.name)).filter((e) => e));

	let revealed = $state<Set<number>>(new Set());
	function toggleReveal(i: number) {
		const next = new Set(revealed);
		if (next.has(i)) next.delete(i);
		else next.add(i);
		revealed = next;
	}

	function addEnvRow() {
		envRows = [...envRows, { name: '', value: '' }];
		replaceEnv = true;
	}
	function removeEnvRow(i: number) {
		envRows = envRows.filter((_, j) => j !== i);
		revealed = new Set([...revealed].filter((j) => j !== i).map((j) => (j > i ? j - 1 : j)));
		replaceEnv = true;
	}
	function onEdit() {
		replaceEnv = true;
	}

	function markRemove(name: string) {
		if (!envRemove.includes(name)) envRemove = [...envRemove, name];
	}
	function unmarkRemove(name: string) {
		envRemove = envRemove.filter((n) => n !== name);
	}
</script>

<div class="env-editor">
	<Text as="p" tone="faint" size="xs">
		{m.providers_env_intro()}
	</Text>

	{#if storedNames.length}
		<div class="stored">
			<Text as="div" tone="muted" size="xs">{m.providers_env_currently_set()}</Text>
			<div class="chips">
				{#each storedNames as n (n)}
					{#if envRemove.includes(n)}
						<span class="chip removing">
							<s>{n}</s>
							<button class="chip-x" onclick={() => unmarkRemove(n)} aria-label={m.providers_env_keep_aria({ name: n })}>↩</button>
						</span>
					{:else}
						<span class="chip">
							{n}
							<button class="chip-x" onclick={() => markRemove(n)} aria-label={m.providers_env_delete_aria({ name: n })}>✕</button>
						</span>
					{/if}
				{/each}
			</div>
			{#if envRemove.length}
				<Text as="div" tone="warn" size="xs">
					{m.providers_env_will_delete({ names: envRemove.join(', ') })}
				</Text>
			{/if}
		</div>
	{/if}

	<Text as="div" tone="faint" size="xs">
		{#if replaceEnv}
			<Text as="span" tone="warn" size="xs">{m.providers_env_replace_warning()}</Text>
		{:else}
			<Text as="span" size="xs">{m.providers_env_unchanged()}</Text>
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
						aria-label={m.providers_env_name_aria()}
					/>
					<Input
						bind:value={row.value}
						oninput={onEdit}
						type={revealed.has(i) ? 'text' : 'password'}
						mono
						placeholder={m.providers_env_value_placeholder()}
						aria-label={m.providers_env_value_aria()}
					/>
					<Button onclick={() => toggleReveal(i)} aria-label={revealed.has(i) ? m.providers_env_hide_value() : m.providers_env_show_value()}>
						{revealed.has(i) ? '🙈' : '👁'}
					</Button>
					<Button variant="danger" onclick={() => removeEnvRow(i)} aria-label={m.providers_env_remove_aria()}>✕</Button>
				</div>
			{/each}
		</div>
	{/if}
	{#each nameErrors as e (e)}<Error>{e}</Error>{/each}
	<Button onclick={addEnvRow}>{m.providers_env_add()}</Button>
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
		display: inline-flex;
		align-items: center;
		gap: 0.35em;
		font-family: var(--font-mono, monospace);
		font-size: var(--fs-xs);
		padding: 0.1em 0.5em;
		border-radius: var(--r-sm);
		background: var(--surface-2, color-mix(in srgb, var(--text) 8%, transparent));
	}
	.chip.removing {
		opacity: 0.6;
	}
	.chip-x {
		border: none;
		background: none;
		cursor: pointer;
		padding: 0;
		font-size: var(--fs-xs);
		color: inherit;
		line-height: 1;
	}
	.env-rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.env-row {
		display: grid;
		grid-template-columns: 1fr 1fr auto auto;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
