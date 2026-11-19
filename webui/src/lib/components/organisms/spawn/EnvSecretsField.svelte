<script lang="ts">
	// Env rows injected into the worker process, never shown in the conversation,
	// logs or transcript. A stored row's value lives sealed on the server and is
	// never sent back: it shows as set, and can be replaced or removed.
	import { Badge, Button, IconButton, Input, Text } from '@dorsk/tsumikit';
	import type { EnvRow } from './types';
	import Error from '#lib/components/atoms/Error.svelte';
	import { m } from '#lib/paraglide/messages.js';

	let {
		envRows = $bindable(),
		invalid
	}: {
		envRows: EnvRow[];
		// True when some row's key fails the shell-var pattern (validated by parent).
		invalid: boolean;
	} = $props();

	let replacing = $state<string[]>([]);

	const removeEnvRow = (i: number) => (envRows = envRows.filter((_, idx) => idx !== i));
	const replace = (key: string) => (replacing = [...replacing, key]);
</script>

{#if envRows.length}
	<div class="rows">
		{#each envRows as row, i (i)}
			<div class="row gap">
				{#if row.stored}
					<span class="name" data-testid="env-stored-name"><Text variant="code">{row.key}</Text></span>
					{#if row.value || replacing.includes(row.key)}
						<Input
							mono
							style="flex:1;min-width:0"
							type="password"
							placeholder={m.spawn_secret_replace_placeholder()}
							aria-label={m.spawn_secret_value_aria()}
							bind:value={row.value}
						/>
					{:else}
						<Badge tone="muted" aria-label={m.spawn_secret_stored_aria({ name: row.key })}>
							{m.spawn_secret_stored()}
						</Badge>
						<Button size="sm" variant="ghost" onclick={() => replace(row.key)}>
							{m.spawn_secret_replace()}
						</Button>
					{/if}
				{:else}
					<Input
						mono
						style="flex:1;min-width:0"
						placeholder="API_KEY"
						aria-label={m.spawn_secret_name_aria()}
						bind:value={row.key}
					/>
					<Input
						mono
						style="flex:1;min-width:0"
						type="password"
						placeholder={m.spawn_secret_value_placeholder()}
						aria-label={m.spawn_secret_value_aria()}
						bind:value={row.value}
					/>
				{/if}
				<IconButton icon="trash" label={m.spawn_remove_secret()} hoverDanger onclick={() => removeEnvRow(i)} />
			</div>
		{/each}
		{#if invalid}
			<Error>{m.spawn_secret_key_error()} <Text variant="code">^[A-Z_][A-Z0-9_]*$</Text></Error>
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
		align-items: center;
		gap: var(--sp-2);
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>
