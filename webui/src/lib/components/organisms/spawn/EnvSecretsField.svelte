<script lang="ts">
	// Environment-secrets rows, extracted from SpawnModal (CCT-202). Rows are
	// injected as env vars in the worker process — never shown in the conversation,
	// logs, or transcript, and fixed for the session's lifetime. Values live on the
	// parent's modal-scoped state (kept out of the persisted draft); this component
	// just renders + edits the rows. The "add" affordance lives in the parent's
	// shared action row, so this component only renders existing rows + the error.
	interface EnvRow {
		key: string;
		value: string;
	}
	import { IconButton, Input, Text } from '@dorsk/tsumikit';
	import Error from '$lib/components/atoms/Error.svelte';

	let {
		envRows = $bindable(),
		invalid
	}: {
		envRows: EnvRow[];
		// True when some row's key fails the shell-var pattern (validated by parent).
		invalid: boolean;
	} = $props();

	const removeEnvRow = (i: number) => (envRows = envRows.filter((_, idx) => idx !== i));
</script>

{#if envRows.length}
	<div class="rows">
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
				<IconButton icon="trash" label="Remove secret" hoverDanger onclick={() => removeEnvRow(i)} />
			</div>
		{/each}
		{#if invalid}
			<Error>Keys must match <Text variant="code">^[A-Z_][A-Z0-9_]*$</Text></Error>
		{/if}
	</div>
{/if}

<style>
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.row.gap {
		display: flex;
		gap: var(--sp-2);
	}
</style>
