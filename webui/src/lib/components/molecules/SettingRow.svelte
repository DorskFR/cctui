<script lang="ts">
	// One setting: label + help on the left, the control pinned to a fixed-width
	// column on the right (stacked on narrow screens). `wide` drops the column so
	// a textarea or a radio grid can take the whole row. `server` / `admin` add
	// the scope pills that tell the user where the value lives. The
	// `data-setting-row` hook is what the Settings page filter walks. Controls
	// that should fill the column pass `style="width:100%"` themselves (rule 4.2
	// of webui/DESIGN.md), so this row never reaches into Tsumikit internals.
	import { untrack, type Snippet } from 'svelte';
	import { Badge, setFieldContext, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		label,
		help,
		server = false,
		admin = false,
		wide = false,
		disabled = false,
		selfLabelled = false,
		children,
		helpSlot
	}: {
		label: string;
		help?: string;
		server?: boolean;
		admin?: boolean;
		wide?: boolean;
		disabled?: boolean;
		/** The row's control(s) name themselves (a segmented group, or several
		 *  controls sharing the row), so the row label owns no `for`. */
		selfLabelled?: boolean;
		children?: Snippet;
		/** Rich help (links, kbd) — used instead of `help` when given. */
		helpSlot?: Snippet;
	} = $props();

	const uid = $props.id();
	if (!untrack(() => selfLabelled)) setFieldContext({ id: uid, invalid: false });
</script>

{#snippet lblLine()}
	<span class="lbl-line">
		{label}
		{#if server}<Badge tone="info" size="sm" uppercase border>{m.settings_scope_server()}</Badge>{/if}
		{#if admin}<Badge tone="warn" size="sm" uppercase border>{m.settings_scope_admin()}</Badge>{/if}
	</span>
{/snippet}

<div class="row" class:wide class:disabled data-setting-row>
	<div class="lbl">
		{#if selfLabelled}
			<Text weight="semibold" as="div">{@render lblLine()}</Text>
		{:else}
			<label for={uid}><Text weight="semibold" as="span">{@render lblLine()}</Text></label>
		{/if}
		{#if helpSlot}
			<Text size="sm" tone="faint" as="div">{@render helpSlot()}</Text>
		{:else if help}
			<Text size="sm" tone="faint" as="div">{help}</Text>
		{/if}
	</div>
	<div class="ctl">{@render children?.()}</div>
</div>

<style>
	.row {
		display: grid;
		grid-template-columns: minmax(0, 1fr) var(--settings-ctl-w, 15rem);
		gap: var(--sp-2) var(--sp-6);
		align-items: center;
		padding: var(--sp-3) var(--sp-4);
		border-top: 1px solid var(--border);
	}
	.row:first-child {
		border-top: 0;
	}
	.row.wide {
		grid-template-columns: minmax(0, 1fr);
	}
	.row.disabled {
		opacity: 0.45;
	}
	.lbl {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.lbl-line {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
	.ctl {
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.row.wide .ctl {
		display: block;
	}
	@media (max-width: 47.999rem) {
		.row {
			grid-template-columns: minmax(0, 1fr);
		}
		.ctl {
			justify-content: flex-start;
			width: 100%;
		}
	}
</style>
