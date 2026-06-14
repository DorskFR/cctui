<script lang="ts">
	// Environment-secrets editor, extracted from SpawnModal (CCT-202). Rows are
	// injected as env vars in the worker process — never shown in the conversation,
	// logs, or transcript, and fixed for the session's lifetime. Values live on the
	// parent's modal-scoped state (kept out of the persisted draft); this component
	// just renders + edits the rows.
	interface EnvRow {
		key: string;
		value: string;
	}
	import { Button, Field, IconButton, Input, Text } from '@dorsk/tsumikit';

	let {
		envRows = $bindable(),
		invalid
	}: {
		envRows: EnvRow[];
		// True when some row's key fails the shell-var pattern (validated by parent).
		invalid: boolean;
	} = $props();

	const addEnvRow = () => (envRows = [...envRows, { key: '', value: '' }]);
	const removeEnvRow = (i: number) => (envRows = envRows.filter((_, idx) => idx !== i));
</script>

<Field label="Environment secrets">
	<Text tone="faint" size="xs">
		Injected as env vars in the worker — not visible in the conversation,
		logs, or transcript, and fixed for the session's lifetime.
	</Text>
	{#each envRows as row, i (i)}
		<div class="row gap">
			<Input
				mono
				style="flex:1;min-width:0"
				placeholder="API_KEY"
				aria-label="Secret name"
				bind:value={row.key}
			/>
			<Input
				mono
				style="flex:1;min-width:0"
				type="password"
				placeholder="value"
				aria-label="Secret value"
				bind:value={row.value}
			/>
			<IconButton inline class="hover-danger" icon="x" size={16} label="Remove secret" onclick={() => removeEnvRow(i)} />
		</div>
	{/each}
	<Button control style="align-self:flex-start" onclick={addEnvRow}>+ Add secret</Button>
	{#if invalid}
		<Text class="err" size="xs">Keys must match <Text variant="code">^[A-Z_][A-Z0-9_]*$</Text></Text>
	{/if}
</Field>

<style>
	.row.gap {
		display: flex;
		gap: var(--sp-2);
	}
	/* Error colour; size is the Text atom's. Rides on a Text child, so :global. */
	:global(.err) {
		color: var(--c-red);
	}
</style>
