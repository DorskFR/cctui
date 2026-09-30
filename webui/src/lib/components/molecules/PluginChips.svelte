<script lang="ts">
	// Chips for a session's `metadata.plugins` slots, one per registered
	// renderer. `editable` adds the manual YouTrack entry the drawer header
	// offers; the card renders read-only.
	import { Badge, Icon, IconButton, Input } from '@dorsk/tsumikit';
	import { safeHref } from '$lib/safeHref';
	import { m } from '$lib/paraglide/messages';
	import { pluginChips, readPluginSlot } from '$lib/plugins/sessionSlots';
	import { parseIssueEntry } from '$lib/plugins/issueId';
	import { lookupYouTrackIssue } from '$lib/plugins/youtrackLookup';

	const YOUTRACK = 'youtrack';

	let {
		metadata,
		editable = false,
		detected = null,
		onset
	}: {
		metadata: unknown;
		/** Show the manual set/clear affordance for the YouTrack slot. */
		editable?: boolean;
		/** An issue id found in the prompt / name / branch, offered when the
		 *  slot is empty. */
		detected?: string | null;
		onset?: (pluginId: string, data: Record<string, unknown> | null) => void;
	} = $props();

	const chips = $derived(pluginChips(metadata));
	const youtrack = $derived(readPluginSlot(metadata, YOUTRACK));
	const suggestion = $derived(editable && !youtrack && detected ? detected : null);

	let editing = $state(false);
	let entry = $state('');
	let invalid = $state(false);

	function open() {
		const current = youtrack?.issue;
		entry = typeof current === 'string' ? current : (detected ?? '');
		invalid = false;
		editing = true;
	}

	async function link(input: string) {
		invalid = false;
		const parsed = parseIssueEntry(input);
		if (!parsed) {
			invalid = true;
			return;
		}
		editing = false;
		const slot = await lookupYouTrackIssue(parsed.issue);
		onset?.(YOUTRACK, { ...slot, ...(parsed.url ? { url: parsed.url } : {}) });
	}

	function apply() {
		if (!entry.trim()) {
			editing = false;
			onset?.(YOUTRACK, null);
			return;
		}
		void link(entry);
	}
</script>

{#if chips.length || suggestion || editable}
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

		{#if editable}
			{#if editing}
				<span class="entry" class:invalid data-journey="plugin-chip-entry">
					<Input
						bind:value={entry}
						placeholder={m.plugin_chip_issue_placeholder()}
						aria-label={m.plugin_chip_issue_aria()}
						aria-invalid={invalid}
						onkeydown={(e: KeyboardEvent) => {
							if (e.key === 'Enter') apply();
							else if (e.key === 'Escape') editing = false;
						}}
					/>
					<span data-journey="plugin-chip-apply">
						<IconButton chip variant="default" icon="check" label={m.common_apply()} onclick={apply} />
					</span>
					<IconButton
						chip
						variant="default"
						icon="x"
						label={m.common_cancel()}
						onclick={() => (editing = false)}
					/>
				</span>
			{:else if suggestion}
				<span data-journey="plugin-chip-suggest">
					<Badge
						as="button"
						mono
						title={m.plugin_chip_link_detected_title({ issue: suggestion })}
						onclick={() => {
							if (suggestion) void link(suggestion);
						}}>+ {suggestion}</Badge
					>
				</span>
			{:else}
				<span data-journey="plugin-chip-edit">
					<IconButton
						chip
						variant="default"
						icon="tag"
						label={youtrack ? m.plugin_chip_issue_edit() : m.plugin_chip_issue_set()}
						onclick={open}
					/>
				</span>
			{/if}
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
	.entry {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
		max-width: 16rem;
	}
	.entry.invalid {
		outline: 1px solid var(--danger);
		border-radius: var(--radius-sm);
	}
</style>
