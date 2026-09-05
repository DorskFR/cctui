<script lang="ts">
	import { accountAvatarColors, accountInitial } from './avatar';

	// An account's identity mark: the owner's emoji when set, else a rounded
	// square coloured from the account id carrying the first letter of the name.
	let {
		emoji = null,
		name = '',
		id = '',
		size = 16,
		decorative = false
	}: {
		emoji?: string | null;
		name?: string | null;
		/** Account id — seeds the fallback colour, so it is stable everywhere. */
		id?: string;
		/** Box size in px; the glyph tracks it. */
		size?: number;
		/** The surrounding element already names the account: stay out of the
		 *  accessibility tree rather than repeating it. */
		decorative?: boolean;
	} = $props();

	const label = $derived(name ?? '');
	const glyph = $derived(emoji?.trim() ? emoji.trim() : null);
	const colors = $derived(accountAvatarColors(id || label));
	const box = $derived(
		`width: ${size}px; height: ${size}px; border-radius: ${Math.max(2, Math.round(size / 4))}px;`
	);
</script>

{#if glyph}
	<span
		class="av emoji"
		style="{box} font-size: {Math.round(size * 0.82)}px"
		role={decorative ? 'presentation' : 'img'}
		aria-hidden={decorative ? 'true' : undefined}
		aria-label={decorative ? undefined : label}
		title={decorative ? undefined : label}
	>
		{glyph}
	</span>
{:else}
	<span
		class="av square"
		style="{box} background: {colors.background}; color: {colors.color}; font-size: {Math.round(size * 0.58)}px"
		role={decorative ? 'presentation' : 'img'}
		aria-hidden={decorative ? 'true' : undefined}
		aria-label={decorative ? undefined : label}
		title={decorative ? undefined : label}
	>
		{accountInitial(label)}
	</span>
{/if}

<style>
	.av {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		line-height: 1;
		user-select: none;
	}
	.square {
		font-weight: 600;
	}
	.emoji {
		background: none;
	}
</style>
