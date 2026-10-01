<script lang="ts">
	import { Text } from '@dorsk/tsumikit';
	import { harnessCommandHead, type HarnessCommand } from './format';
	import { m } from '$lib/paraglide/messages';

	let { command }: { command: HarnessCommand } = $props();

	const head = $derived(harnessCommandHead(command));
</script>

<div class="bubble cmd">
	{#if head}
		<Text as="div" variant="code" size="sm" weight="semibold">{head}</Text>
	{/if}
	{#if command.message && command.message !== head}
		<Text as="p" tone="muted" size="xs">{command.message}</Text>
	{/if}
	{#if command.stdout}
		<pre class="out">{command.stdout}</pre>
	{/if}
	{#if command.stderr}
		<div class="stderr">
			<Text as="div" tone="danger" size="xs">{m.conversation_command_stderr()}</Text>
			<pre class="out">{command.stderr}</pre>
		</div>
	{/if}
</div>

<style>
	.cmd {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.stderr {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.out {
		margin: 0;
		padding: var(--sp-2);
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		font-family: var(--font-mono);
		white-space: pre-wrap;
		overflow: auto;
		max-height: 22rem;
		font-size: calc(var(--fs-sm) - 0.0625rem);
	}
</style>
