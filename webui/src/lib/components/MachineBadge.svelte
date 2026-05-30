<script lang="ts">
	import { hashHue } from '$lib/format';

	// Deterministic-color machine badge, shared by the session list and the
	// chat header so the same machine reads the same hue everywhere.
	let {
		name,
		id,
		mono = false
	}: {
		name?: string | null;
		id: string;
		mono?: boolean;
	} = $props();

	const label = $derived(name || id.slice(0, 8));
</script>

<span class="badge mach" class:mono style={`--mh:${hashHue(label)}`}>{label}</span>

<style>
	.mach {
		background: hsl(var(--mh) 45% 22%);
		color: hsl(var(--mh) 70% 80%);
		border-color: hsl(var(--mh) 45% 35%);
	}
</style>
