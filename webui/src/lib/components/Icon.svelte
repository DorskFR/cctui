<script lang="ts">
	let {
		name,
		size = 16
	}: {
		name: 'search' | 'archive' | 'download' | 'link' | 'markdown' | 'image' | 'fork' | 'stop';
		size?: number;
	} = $props();
</script>

<svg
	class="icon"
	width={size}
	height={size}
	viewBox="0 0 24 24"
	fill={name === 'stop' ? 'currentColor' : 'none'}
	stroke={name === 'stop' ? 'none' : 'currentColor'}
	stroke-width="2"
	stroke-linecap="round"
	stroke-linejoin="round"
	aria-hidden="true"
>
	{#if name === 'search'}
		<circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" />
	{:else if name === 'archive'}
		<rect x="3" y="4" width="18" height="4" rx="1" /><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8" /><path d="M9 12h6" />
	{:else if name === 'download'}
		<path d="M12 3v12" /><path d="m7 10 5 5 5-5" /><path d="M4 19h16" />
	{:else if name === 'link'}
		<path d="M10 13a5 5 0 0 0 7.07 0l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /><path d="M14 11a5 5 0 0 0-7.07 0l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
	{:else if name === 'markdown'}
		<rect x="3" y="5" width="18" height="14" rx="2" /><path d="M7 15V9l3 3 3-3v6" /><path d="m15 11 2 2 2-2" />
	{:else if name === 'image'}
		<rect x="3" y="3" width="18" height="18" rx="2" /><circle cx="8.5" cy="8.5" r="1.8" /><path d="m21 15-4.5-4.5L7 21" />
	{:else if name === 'fork'}
		<circle cx="6" cy="6" r="2.5" /><circle cx="18" cy="6" r="2.5" /><circle cx="12" cy="19" r="2.5" /><path d="M6 8.5v2a3 3 0 0 0 3 3h6a3 3 0 0 0 3-3v-2" /><path d="M12 13.5v3" />
	{:else if name === 'stop'}
		<rect x="6" y="6" width="12" height="12" rx="1.5" />
	{/if}
</svg>
