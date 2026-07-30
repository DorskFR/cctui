<script lang="ts">
	import { Tooltip } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// Which OAuth account a session runs under. A compact key glyph
	// shown next to the machine/session name; the full account name is revealed
	// on hover/tap via the tsumikit Tooltip. Renders nothing when the session
	// has no resolved account (e.g. a local session that never routed through
	// the cctui gateway), so it never disrupts the existing layout.
	//
	// When `onclick` is supplied the glyph becomes a button that opens the
	// at-will account switcher; otherwise it stays a plain
	// read-only indicator.
	// `warn` marks an account-bound session whose gateway token has never been
	// observed at the gateway — it may be silently running on ambient credentials.
	// The glyph turns to a warning hue and the tooltip explains the mismatch.
	let {
		name,
		onclick,
		warn = false,
	}: { name?: string | null; onclick?: () => void; warn?: boolean } = $props();

	const tip = $derived(
		warn && name
			? m.sessions_account_unobserved_tip({ name })
			: onclick
				? m.sessions_account_switch_tip({ name: name ?? '' })
				: m.sessions_account_tip({ name: name ?? '' }),
	);
</script>

{#if name}
	<Tooltip text={tip}>
		{#snippet trigger()}
			<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
			<!-- role + tabindex are applied together only when `onclick` is set, so the
			     glyph is a real button then and a plain indicator otherwise. -->
			<span
				class="acct"
				class:clickable={!!onclick}
				class:warn
				role={onclick ? 'button' : undefined}
				tabindex={onclick ? 0 : undefined}
				aria-label={warn ? tip : onclick ? m.sessions_account_switch_aria({ name }) : m.sessions_account_tip({ name })}
				{onclick}
				onkeydown={onclick
					? (e: KeyboardEvent) => {
							if (e.key === 'Enter' || e.key === ' ') {
								e.preventDefault();
								onclick();
							}
						}
					: undefined}
			>
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
	.acct.clickable {
		cursor: pointer;
		border: none;
		background: none;
		padding: 0;
	}
	.acct.clickable:hover,
	.acct.clickable:focus-visible {
		color: var(--text, currentColor);
		outline: none;
	}
	.acct.warn {
		color: var(--warn);
	}
	.acct svg {
		/* em-relative so the glyph scales with the font-scale picker. */
		width: 1em;
		height: 1em;
	}
</style>
