<script lang="ts">
	export type IconName =
		| 'archive'
		| 'back'
		| 'check'
		| 'download'
		| 'edit'
		| 'filter'
		| 'folder'
		| 'fork'
		| 'image'
		| 'link'
		| 'live'
		| 'markdown'
		| 'more'
		| 'retry'
		| 'search'
		| 'send'
		| 'star'
		| 'stop'
		| 'x';

	let {
		name,
		size = 16
	}: {
		name: IconName;
		size?: number;
	} = $props();
</script>

<svg
	class="icon"
	width={size}
	height={size}
	viewBox="0 0 24 24"
	fill={name === 'stop' || name === 'star' || name === 'live' ? 'currentColor' : 'none'}
	stroke={name === 'stop' || name === 'star' || name === 'live' ? 'none' : 'currentColor'}
	stroke-width="2"
	stroke-linecap="round"
	stroke-linejoin="round"
	aria-hidden="true"
>
	{#if name === 'search'}
		<circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" />
	{:else if name === 'back'}
		<path d="m15 18-6-6 6-6" />
	{:else if name === 'check'}
		<path d="M20 6 9 17l-5-5" />
	{:else if name === 'x'}
		<path d="M18 6 6 18" /><path d="m6 6 12 12" />
	{:else if name === 'archive'}
		<rect x="3" y="4" width="18" height="4" rx="1" /><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8" /><path d="M9 12h6" />
	{:else if name === 'download'}
		<path d="M12 3v12" /><path d="m7 10 5 5 5-5" /><path d="M4 19h16" />
	{:else if name === 'edit'}
		<path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" />
	{:else if name === 'folder'}
		<path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />
	{:else if name === 'link'}
		<path d="M10 13a5 5 0 0 0 7.07 0l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /><path d="M14 11a5 5 0 0 0-7.07 0l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
	{:else if name === 'markdown'}
		<rect x="3" y="5" width="18" height="14" rx="2" /><path d="M7 15V9l3 3 3-3v6" /><path d="m15 11 2 2 2-2" />
	{:else if name === 'image'}
		<rect x="3" y="3" width="18" height="18" rx="2" /><circle cx="8.5" cy="8.5" r="1.8" /><path d="m21 15-4.5-4.5L7 21" />
	{:else if name === 'fork'}
		<circle cx="6" cy="6" r="2.5" /><circle cx="18" cy="6" r="2.5" /><circle cx="12" cy="19" r="2.5" /><path d="M6 8.5v2a3 3 0 0 0 3 3h6a3 3 0 0 0 3-3v-2" /><path d="M12 13.5v3" />
	{:else if name === 'more'}
		<circle cx="5" cy="12" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="19" cy="12" r="1.6" />
	{:else if name === 'retry'}
		<path d="M21 12a9 9 0 1 1-2.64-6.36" /><path d="M21 3v6h-6" />
	{:else if name === 'stop'}
		<rect x="6" y="6" width="12" height="12" rx="1.5" />
	{:else if name === 'star'}
		<path d="M12 2.5l2.9 5.9 6.5.95-4.7 4.58 1.1 6.47L12 17.9l-5.8 3.05 1.1-6.47-4.7-4.58 6.5-.95z" />
	{:else if name === 'live'}
		<circle cx="12" cy="12" r="5" />
	{:else if name === 'send'}
		<path d="M22 2 11 13" /><path d="M22 2 15 22l-4-9-9-4z" />
	{:else if name === 'filter'}
		<path d="M3 4h18l-7 8v6l-4 2v-10z" />
	{/if}
</svg>
