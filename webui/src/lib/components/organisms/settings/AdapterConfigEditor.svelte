<script lang="ts">
	import { Button, Text, Textarea } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { formatAdapterConfig, parseAdapterConfig } from './machineAdapters.logic';

	let {
		config,
		label,
		disabled = false,
		onsave
	}: {
		config: Record<string, unknown>;
		label: string;
		disabled?: boolean;
		onsave: (config: Record<string, unknown>) => Promise<boolean>;
	} = $props();

	let open = $state(false);
	let text = $state('');
	const parsed = $derived(parseAdapterConfig(text));
	const error = $derived.by(() => {
		if (parsed.ok) return null;
		return parsed.reason === 'syntax'
			? m.settings_machine_adapters_config_syntax({ detail: parsed.detail })
			: m.settings_machine_adapters_config_not_object();
	});
	const errorId = $props.id();

	function edit() {
		text = formatAdapterConfig(config);
		open = true;
	}

	async function save() {
		if (!parsed.ok) return;
		if (await onsave(parsed.config)) open = false;
	}
</script>

{#if open}
	<div class="editor">
		<Textarea
			mono
			autoresize
			block
			size="sm"
			bind:value={text}
			{disabled}
			invalid={!parsed.ok}
			aria-label={label}
			aria-describedby={error ? errorId : null}
			placeholder={'{ "bin": "/usr/local/bin/…" }'}
		/>
		{#if error}
			<Text size="xs" tone="danger" id={errorId}>{error}</Text>
		{:else}
			<Text size="xs" tone="faint">{m.settings_machine_adapters_config_help()}</Text>
		{/if}
		<div class="actions">
			<Button size="sm" variant="primary" disabled={disabled || !parsed.ok} onclick={save}>
				{m.settings_machine_adapters_config_save()}
			</Button>
			<Button size="sm" variant="ghost" {disabled} onclick={() => (open = false)}>
				{m.settings_machine_adapters_config_cancel()}
			</Button>
		</div>
	</div>
{:else}
	<Button size="sm" variant="ghost" {disabled} onclick={edit}>
		{m.settings_machine_adapters_config_edit()}
	</Button>
{/if}

<style>
	.editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		flex-basis: 100%;
	}
	.actions {
		display: flex;
		gap: var(--sp-2);
	}
</style>
