<script lang="ts">
	// Chips for a session's `metadata.plugins` slots, one per registered
	// renderer. Purely presentational: there is no empty state, so a session
	// with nothing linked adds nothing to its row. Setting an issue by hand
	// lives in the drawer header's overflow menu.
	import { Badge, Icon } from '@dorsk/tsumikit';
	import { safeHref } from '$lib/safeHref';
	import { m } from '$lib/paraglide/messages';
	import { pluginChips, readPluginSlot } from '$lib/plugins/sessionSlots';
	import { YOUTRACK_PLUGIN_ID } from '$lib/plugins/issueLink';

	let {
		metadata,
		detected = null,
		suggestable = false,
		onlink
	}: {
		metadata: unknown;
		/** An issue id found in the prompt / name / branch. */
		detected?: string | null;
		/** Offer `detected` as a one-click link while no issue is stored. */
		suggestable?: boolean;
		onlink?: (issue: string) => void;
	} = $props();

	const chips = $derived(pluginChips(metadata));
	const linked = $derived(!!readPluginSlot(metadata, YOUTRACK_PLUGIN_ID));
	const suggestion = $derived(suggestable && !linked && detected ? detected : null);
</script>

{#if chips.length || suggestion}
	<span class="chips" data-journey="plugin-chips">
		{#each chips as chip (chip.pluginId)}
			{#if chip.href}
				<a
					class="chip-link"
					href={safeHref(chip.href)}
					target="_blank"
					rel="noopener noreferrer"
					title={chip.title}
					data-plugin={chip.pluginId}
					data-journey="plugin-chip"
					onclick={(e) => e.stopPropagation()}
				>
					<Icon name={chip.icon} size={12} />
					<span class="chip-label">{chip.label}</span>
				</a>
			{:else}
				<span
					class="chip-flat"
					title={chip.title}
					data-plugin={chip.pluginId}
					data-journey="plugin-chip"
				>
					<Badge mono style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%">
						<Icon name={chip.icon} size={12} />
						<span class="chip-label">{chip.label}</span>
					</Badge>
				</span>
			{/if}
		{/each}

		{#if suggestion}
			<span data-journey="plugin-chip-suggest">
				<Badge
					as="button"
					mono
					title={m.plugin_chip_link_detected_title({ issue: suggestion })}
					onclick={() => {
						if (suggestion) onlink?.(suggestion);
					}}>+ {suggestion}</Badge
				>
			</span>
		{/if}
	</span>
{/if}

<style>
	.chips {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		flex: 0 1 auto;
		overflow: hidden;
	}
	.chip-flat {
		display: inline-flex;
		min-width: 0;
	}
	.chip-link {
		display: inline-flex;
		align-items: center;
		gap: 0.25em;
		min-width: 0;
		font-size: var(--fs-xs);
		color: var(--accent);
		text-decoration: none;
		white-space: nowrap;
	}
	.chip-link:hover {
		text-decoration: underline;
	}
	.chip-label {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
