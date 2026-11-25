<script lang="ts">
	import { Checkbox } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';

	let { checked = $bindable(false) }: { checked?: boolean } = $props();

	const hintId = $props.id();
</script>

<div class="auto-archive">
	<Checkbox
		bind:checked
		data-journey="auto-archive"
		label={m.spawn_auto_archive()}
		aria-describedby={hintId}
	/>
	<span id={hintId} class="hint">{m.spawn_auto_archive_hint()}</span>
</div>

<style>
	.auto-archive {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		column-gap: var(--sp-2);
		flex-basis: 100%;
	}
	.hint {
		color: var(--text-muted);
		font-size: var(--text-xs);
	}
</style>
