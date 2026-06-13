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
	import Button from '$lib/components/atoms/Button.svelte';
	import IconButton from '$lib/components/molecules/IconButton.svelte';
	import Field from '$lib/components/molecules/Field.svelte';
	import Input from '$lib/components/atoms/Input.svelte';

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
	<span class="faint sm">
		Injected as env vars in the worker — not visible in the conversation,
		logs, or transcript, and fixed for the session's lifetime.
	</span>
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
			<IconButton inline class="hover-danger" icon="x" label="Remove secret" onclick={() => removeEnvRow(i)} />
		</div>
	{/each}
	<Button control style="align-self:flex-start" onclick={addEnvRow}>+ Add secret</Button>
	{#if invalid}
		<span class="err sm">Keys must match <code>^[A-Z_][A-Z0-9_]*$</code></span>
	{/if}
</Field>

<style>
	.row.gap {
		display: flex;
		gap: var(--sp-2);
	}
	.err {
		color: var(--c-red);
	}
	.sm {
		font-size: var(--fs-xs);
	}
</style>
