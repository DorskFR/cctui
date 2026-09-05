<script lang="ts">
	// Pull-request glyph. Local because tsumikit's Icon set has no VCS icons
	// beyond `fork`; drop this once the kit carries `pull-request`.
	let { size = 12 }: { size?: number } = $props();
</script>

<svg
	width={size}
	height={size}
	viewBox="0 0 24 24"
	fill="none"
	stroke="currentColor"
	stroke-width="2"
	stroke-linecap="round"
	stroke-linejoin="round"
	aria-hidden="true"
	focusable="false"
>
	<circle cx="6" cy="6" r="3" />
	<circle cx="18" cy="18" r="3" />
	<path d="M13 6h3a2 2 0 0 1 2 2v7" />
	<path d="M6 9v12" />
</svg>
