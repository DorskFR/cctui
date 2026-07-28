<script lang="ts">
	// Tiny brand marks per adapter/provider. `currentColor` so callers control the
	// hue via CSS. Anthropic mark for claude-code / anthropic, OpenAI blossom for
	// codex / openai, Fireworks chevrons for opencode / fireworks. Pass `provider`
	// (account-side) or `adapter` (session-side) — either resolves the mark.
	let {
		adapter,
		provider,
		size = 16
	}: {
		adapter?: string | null;
		provider?: string | null;
		size?: number;
	} = $props();

	const isCodex = $derived(
		provider != null
			? provider === 'openai'
			: (adapter ?? 'claude-code').toString().startsWith('codex')
	);
	const isFireworks = $derived(
		provider != null
			? provider === 'fireworks'
			: (adapter ?? '').toString().startsWith('opencode')
	);
</script>

{#if isFireworks}
	<!-- Fireworks AI mark -->
	<svg
		width={size * 2}
		height={size}
		viewBox="0 0 638 315"
		preserveAspectRatio="xMidYMid meet"
		fill="currentColor"
		aria-label="Fireworks"
		role="img"
	>
		<path
			d="M318.563 221.755C300.863 221.755 284.979 211.247 278.206 194.978L196.549 0H244.342L318.842 178.361L393.273 0H441.066L358.92 195.048C352.112 211.247 336.263 221.755 318.563 221.755Z"
		/>
		<path
			d="M425.111 314.933C407.481 314.933 391.667 304.494 384.824 288.366C377.947 272.097 381.507 253.524 393.936 240.921L542.657 90.2803L561.229 134.094L425.076 271.748L619.147 270.666L637.72 314.479L425.146 315.003L425.076 314.933H425.111Z"
		/>
		<path
			d="M0 314.408L18.5727 270.595L212.643 271.677L76.525 133.988L95.0977 90.1748L243.819 240.816C256.247 253.384 259.843 272.026 252.93 288.26C246.088 304.424 230.203 314.827 212.643 314.827L0.0698221 314.339L0 314.408Z"
		/>
	</svg>
{:else if isCodex}
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
