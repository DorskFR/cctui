<script lang="ts">
	import { Tooltip } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import AccountAvatar from './AccountAvatar.svelte';
	import { useAccounts } from '$lib/queries';

	// Which OAuth account a session runs under: the account's identity mark
	// (owner emoji, else its colour square) next to the machine/session name.
	// The full account name is revealed on hover/tap via the tsumikit Tooltip. Renders nothing when the session
	// has no resolved account (e.g. a local session that never routed through
	// the cctui gateway), so it never disrupts the existing layout.
	//
	// When `onclick` is supplied the glyph becomes a button that opens the
	// at-will account switcher; otherwise it stays a plain
	// read-only indicator.
	// `warn` marks an account-bound session whose gateway token has never been
	// observed at the gateway — it may be silently running on ambient credentials.
	// The mark gains a warning ring and the tooltip explains the mismatch.
	//
	// `showName` (Settings › Session list › "Account names") swaps the glyph for
	// the account name itself: with two accounts on the same provider every glyph
	// looks alike, and the name is the only thing that tells them apart at a
	// glance. Everything else — tooltip, click target, warn hue — is unchanged.
	// `emoji`/`id` are optional: callers that only know the account NAME (the
	// session list, the drawer) get them resolved from the accounts cache.
	let {
		name,
		emoji,
		id,
		onclick,
		warn = false,
		showName = false,
	}: {
		name?: string | null;
		emoji?: string | null;
		id?: string;
		onclick?: () => void;
		warn?: boolean;
		showName?: boolean;
	} = $props();

	const accounts = useAccounts();
	const resolved = $derived(
		emoji !== undefined && id !== undefined
			? { emoji, id }
			: (() => {
					const a = (accounts.data ?? []).find((x) => x.name === name);
					return { emoji: emoji ?? a?.emoji ?? null, id: id ?? a?.id ?? '' };
				})(),
	);

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
				class:named={showName}
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
				{#if showName}
					{name}
				{:else}
					<AccountAvatar emoji={resolved.emoji} id={resolved.id} name={name ?? ''} size={14} decorative />
				{/if}
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
		border-radius: var(--r-sm);
		box-shadow: 0 0 0 2px var(--warn);
	}
	/* Name variant: a quiet text chip, capped so a long account name can't push
	   the rest of the row's chips off the end. It still shrinks to its content
	   when short. */
	.acct.named {
		max-width: 12ch;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--fs-xs);
	}
</style>
