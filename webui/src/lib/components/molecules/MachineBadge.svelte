<script lang="ts">
	import { hashHue } from '$lib/format';
	import { Badge } from '@dorsk/tsumikit';

	// Deterministic-color machine badge, shared by the session list and the
	// chat header so the same machine reads the same hue everywhere. Composes the
	// base Badge and overrides its palette with a per-machine hue.
	let {
		name,
		id,
		hue,
		mono = false
	}: {
		name?: string | null;
		id: string;
		/** Operator-set hue override; falls back to the name hash. */
		hue?: number | null;
		mono?: boolean;
	} = $props();

	const label = $derived(name || id.slice(0, 8));
	// Compose the tint inline, on the element where --mh is set: the
	// theme supplies only `<sat%> <light%>` pairs, so the per-machine hue and the
	// theme's saturation/lightness resolve together in a real property. Inline
	// styles apply across the component boundary regardless of Badge's scope.
	const tint = $derived(
		`--mh:${hue ?? hashHue(label)};` +
			'background:hsl(var(--mh) var(--mach-bg-sl));' +
			'color:hsl(var(--mh) var(--mach-fg-sl));' +
			'border-color:hsl(var(--mh) var(--mach-border-sl))'
	);
</script>

<Badge class={mono ? 'mono' : ''} style={tint}>{label}</Badge>
