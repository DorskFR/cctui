<script lang="ts">
	import { hashHue } from '$lib/format';

	// Deterministic-color machine badge, shared by the session list and the
	// chat header so the same machine reads the same hue everywhere.
	let {
		name,
		id,
		hue,
		mono = false
	}: {
		name?: string | null;
		id: string;
		/** Operator-set hue override (CCT-222); falls back to the name hash. */
		hue?: number | null;
		mono?: boolean;
	} = $props();

	const label = $derived(name || id.slice(0, 8));
</script>

<span class="badge mach" class:mono style={`--mh:${hue ?? hashHue(label)}`}>{label}</span>

<style>
	.mach {
		background: hsl(var(--mh) 45% 22%);
		color: hsl(var(--mh) 70% 80%);
		border-color: hsl(var(--mh) 45% 35%);
	}
</style>
