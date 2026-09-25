<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { machineInitial, machineTint } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import AccountBadge from './AccountBadge.svelte';
	import MachineBadge from './MachineBadge.svelte';
	import SessionDot from './SessionDot.svelte';

	// Star · status dot · machine · account. Inline by default; `stack` folds
	// them into one 2×2 slot (`auto`: once the session row is narrow).
	let {
		session,
		livenessClass,
		now,
		stack = 'auto',
		showMachine = true,
		accountWarn = false,
		showAccountName = false,
		onTogglePin,
		onAccountClick
	}: {
		session: SessionListItem;
		livenessClass: string;
		now?: number;
		stack?: 'auto' | 'always' | 'never';
		showMachine?: boolean;
		accountWarn?: boolean;
		showAccountName?: boolean;
		onTogglePin?: (s: SessionListItem) => void;
		onAccountClick?: () => void;
	} = $props();

	const machineLabel = $derived(session.machine_name || session.machine_id.slice(0, 8));

	function pin(e: Event) {
		e.stopPropagation();
		onTogglePin?.(session);
	}
</script>

<span class="glyphs" class:auto={stack === 'auto'} class:always={stack === 'always'}>
	{#if onTogglePin}
		<span
			class="star"
			class:on={session.pinned}
			role="button"
			tabindex="0"
			title={session.pinned ? m.sessions_unpin_title() : m.sessions_pin_title()}
			aria-pressed={session.pinned}
			aria-label={session.pinned ? m.sessions_unpin_aria() : m.sessions_pin_aria()}
			onpointerdown={(e) => e.stopPropagation()}
			onclick={pin}
			onkeydown={(e) => {
				if (e.key === 'Enter' || e.key === ' ') {
					e.preventDefault();
					pin(e);
				}
			}}>{session.pinned ? '★' : '☆'}</span
		>
	{/if}
	<SessionDot {session} {livenessClass} {now} />
	{#if showMachine}
		<span class="mach-full"
			><MachineBadge name={session.machine_name} id={session.machine_id} hue={session.machine_hue} mono dense /></span
		>
		<span class="mach-tile" style={machineTint(machineLabel, session.machine_hue)} title={machineLabel}
			>{machineInitial(machineLabel)}</span
		>
	{/if}
	<AccountBadge
		name={session.account_name}
		warn={accountWarn}
		showName={showAccountName}
		onclick={onAccountClick}
	/>
</span>

<style>
	.glyphs {
		display: contents;
	}
	.star {
		flex: none;
		width: 14px;
		text-align: center;
		line-height: 1;
		font-size: var(--fs-md);
		color: var(--text-faint);
		cursor: pointer;
		user-select: none;
	}
	.star.on,
	.star:hover {
		color: var(--warn);
	}
	.mach-full {
		display: inline-flex;
		flex: none;
	}
	.mach-tile {
		display: none;
	}
	.glyphs.always {
		display: inline-grid;
		grid-template-columns: repeat(2, 1rem);
		grid-auto-rows: 1rem;
		gap: 1px;
		place-items: center;
		flex: none;
	}
	.always .star {
		font-size: 0.8125rem;
	}
	.always .mach-full {
		display: none;
	}
	.always .mach-tile {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		min-width: 1rem;
		height: 0.875rem;
		padding: 0 1px;
		border: 1px solid;
		border-radius: var(--r-sm);
		font-family: var(--font-mono);
		font-size: 0.625rem;
		font-weight: 600;
		line-height: 1;
	}
	@container sess-row (max-width: 34rem) {
		.glyphs.auto {
			display: inline-grid;
			grid-template-columns: repeat(2, 1rem);
			grid-auto-rows: 1rem;
			gap: 1px;
			place-items: center;
			flex: none;
		}
		.auto .star {
			font-size: 0.8125rem;
		}
		.auto .mach-full {
			display: none;
		}
		.auto .mach-tile {
			display: inline-flex;
			align-items: center;
			justify-content: center;
			min-width: 1rem;
			height: 0.875rem;
			padding: 0 1px;
			border: 1px solid;
			border-radius: var(--r-sm);
			font-family: var(--font-mono);
			font-size: 0.625rem;
			font-weight: 600;
			line-height: 1;
		}
	}
</style>
