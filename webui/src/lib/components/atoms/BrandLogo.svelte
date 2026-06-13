<script lang="ts">
	// Tiny brand marks for the two adapters. `currentColor` so callers control
	// the hue via CSS. Anthropic mark for claude-code, OpenAI blossom for codex.
	let {
		adapter,
		size = 16
	}: { adapter: string | null | undefined; size?: number } = $props();

	const isCodex = $derived((adapter ?? 'claude-code').toString().startsWith('codex'));
</script>

{#if isCodex}
	<!-- OpenAI blossom -->
	<svg
		width={size}
		height={size}
		viewBox="0 0 24 24"
		fill="currentColor"
		aria-label="OpenAI / Codex"
		role="img"
	>
		<path
			d="M22.28 9.82a5.98 5.98 0 0 0-.52-4.91 6.05 6.05 0 0 0-6.51-2.9A5.99 5.99 0 0 0 4.98 4.2a5.98 5.98 0 0 0-4 2.9 6.05 6.05 0 0 0 .74 7.1 5.98 5.98 0 0 0 .52 4.9 6.05 6.05 0 0 0 6.51 2.91A5.98 5.98 0 0 0 13.78 22a6.05 6.05 0 0 0 5.77-4.19 5.98 5.98 0 0 0 4-2.9 6.05 6.05 0 0 0-.75-7.09zM13.78 20.7a4.48 4.48 0 0 1-2.88-1.04l.14-.08 4.78-2.76a.78.78 0 0 0 .39-.68v-6.74l2.02 1.17a.07.07 0 0 1 .04.05v5.58a4.5 4.5 0 0 1-4.5 4.5zM4.1 16.56a4.48 4.48 0 0 1-.54-3.02l.14.08 4.78 2.76a.78.78 0 0 0 .78 0l5.84-3.37v2.33a.07.07 0 0 1-.03.06l-4.83 2.79a4.5 4.5 0 0 1-6.14-1.65zM2.84 7.75a4.48 4.48 0 0 1 2.34-1.97V11.45a.78.78 0 0 0 .39.68l5.84 3.37-2.02 1.16a.07.07 0 0 1-.07.01L4.49 13.9a4.5 4.5 0 0 1-1.65-6.15zM19.1 11.53l-5.84-3.38 2.02-1.16a.07.07 0 0 1 .07-.01l4.83 2.79a4.5 4.5 0 0 1-.69 8.12v-5.68a.78.78 0 0 0-.39-.68zM21.1 8.5l-.14-.09-4.77-2.78a.78.78 0 0 0-.79 0L9.57 9V6.67a.07.07 0 0 1 .03-.06l4.83-2.79a4.5 4.5 0 0 1 6.68 4.67zM8.47 12.65l-2.02-1.17a.07.07 0 0 1-.04-.05V5.85a4.5 4.5 0 0 1 7.38-3.45l-.14.08L8.86 5.24a.78.78 0 0 0-.39.68zM9.57 10.3l2.6-1.5 2.6 1.5v3l-2.6 1.5-2.6-1.5z"
		/>
	</svg>
{:else}
	<!-- Anthropic mark -->
	<svg
		width={size}
		height={size}
		viewBox="0 0 24 24"
		fill="currentColor"
		aria-label="Anthropic / Claude Code"
		role="img"
	>
		<path
			d="M15.6 3h-3.05l5.5 18H21L15.6 3zM6.9 3 1.5 21h3.1l1.12-3.78h5.62L12.45 21h3.1L10.15 3H6.9zm-.27 11.4 1.82-6.13 1.82 6.13H6.63z"
		/>
	</svg>
{/if}
