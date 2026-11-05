<script lang="ts">
	import { formatStat, type StatKind } from '$lib/statText';

	let {
		value,
		kind,
		compact = true,
		class: klass = ''
	}: { value: number; kind: StatKind; compact?: boolean; class?: string } = $props();

	const text = $derived(formatStat(value, kind, compact));
	const full = $derived(compact ? formatStat(value, kind, false) : text);
</script>

<span class="stat num {klass}" title={full === text ? undefined : full}>{text}</span>

<style>
	.stat {
		white-space: nowrap;
	}
</style>
