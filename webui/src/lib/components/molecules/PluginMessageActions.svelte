<script lang="ts">
	// Buttons a runtime plugin contributes to one assistant message; each opens
	// the plugin's session pane with the action's params.
	import { Button, Icon } from '@dorsk/tsumikit';
	import type { PluginActionButton } from '$lib/plugins/types';

	let { actions, onopen }: { actions: PluginActionButton[]; onopen: (a: PluginActionButton) => void } = $props();
</script>

{#if actions.length}
	<div class="plugin-actions" data-journey="plugin-actions">
		{#each actions as a, i (`${a.pluginId}:${i}`)}
			<Button
				size="sm"
				pill
				variant="default"
				data-journey="plugin-action"
				data-plugin={a.pluginId}
				onclick={() => onopen(a)}
			>
				<Icon name={a.icon} size={14} />
				{a.label}
			</Button>
		{/each}
	</div>
{/if}

<style>
	.plugin-actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
		margin-top: var(--sp-1);
	}
</style>
