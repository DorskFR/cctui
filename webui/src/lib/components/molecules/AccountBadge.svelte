<script lang="ts">
	import { Tooltip } from '@dorsk/tsumikit';

	// Which OAuth account a session runs under (CCT-430). A compact key glyph
	// shown next to the machine/session name; the full account name is revealed
	// on hover/tap via the tsumikit Tooltip. Renders nothing when the session
	// has no resolved account (e.g. a local session that never routed through
	// the cctui gateway), so it never disrupts the existing layout.
	let { name }: { name?: string | null } = $props();
</script>

{#if name}
	<Tooltip text={`account: ${name}`}>
		{#snippet trigger()}
			<span class="acct" aria-label={`account: ${name}`}>
				<!-- lucide key-round, sized in em so it tracks the font-scale picker. -->
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
					aria-hidden="true"
				>
					<path d="m21 2-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0 3 3L22 7l-3-3" />
				</svg>
			</span>
		{/snippet}
	</Tooltip>
{/if}

<style>
	.acct {
		display: inline-flex;
		align-items: center;
		flex: none;
		color: var(--c-muted, currentColor);
		cursor: default;
	}
	.acct svg {
		/* em-relative so the glyph scales with the font-scale picker (CCT-408). */
		width: 1em;
		height: 1em;
	}
</style>
