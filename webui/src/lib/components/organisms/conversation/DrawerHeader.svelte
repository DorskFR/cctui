<script lang="ts">
	// Conversation drawer header: title row, label strip and meta row. Action
	// side-effects are delegated to callbacks; rename UI state lives here.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { settings } from '#lib/settings.svelte.ts';
	import { runHeaderKey } from './headerKeys';
	import type { LabelHandlers } from './headerLabels';
	import RebindTrail from '#lib/components/molecules/RebindTrail.svelte';
	import SessionGlyphs from '#lib/components/molecules/SessionGlyphs.svelte';
	import { IconButton, Toolbar } from '@dorsk/tsumikit';
	import HeaderMeta from './HeaderMeta.svelte';
	import HeaderLabels from './HeaderLabels.svelte';
	import DrawerTitle from './DrawerTitle.svelte';
	import DrawerMenu from './DrawerMenu.svelte';
	import { ComposerFocus } from './composerFocus.svelte';
	import { nameOf } from '#lib/sessionName.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { hasFeature } from '#lib/features.svelte.ts';

	let {
		session,
		archived,
		isCodexSession,
		livenessClass,
		showStatusBadge,
		maximized = false,
		compact = false,
		onmaximize,
		onclose,
		onrename,
		onsetmodel,
		oncopylink,
		oncopymarkdown,
		onexport,
		onsearch,
		onescape,
		onescapeaction,
		shortcuts = true,
		onfork,
		onfollowup,
		onforkselect,
		forkSelectActive = false,
		oninterrupt,
		onarchive,
		onstoparchive,
		onTogglePin,
		onAccountClick,
		onsetroom,
		onclearroom,
		onthreadjump,
		...labels
	}: {
		session: SessionListItem;
		archived: boolean;
		isCodexSession: boolean;
		livenessClass: string;
		showStatusBadge: boolean;
		/** The maximize toggle appears only when the shell supplies `onmaximize`. */
		maximized?: boolean;
		/** Tile chrome: one row only — no meta row, no label strip, every action
		 *  but maximize behind the ⋯ menu. */
		compact?: boolean;
		onmaximize?: () => void;
		/** Omitted in a tile, which has nothing to close back to. */
		onclose?: () => void;
		onrename: (name: string) => void;
		onsetmodel: (model: string, effort: string) => void;
		oncopylink: () => void;
		oncopymarkdown: () => void;
		onexport: () => void;
		/** Open the find-in-conversation bar; omitted → no entry and no ⌘F. */
		onsearch?: () => void;
		/** First refusal on Escape: true when it was consumed (the find bar
		 *  clears or closes) and the drawer must stay open. */
		onescape?: () => boolean;
		/** What an unconsumed Escape does instead of closing: a tile has nothing
		 *  to close back to and interrupts its own session. */
		onescapeaction?: () => void;
		/** Whether this header owns the window chords. One tile at a time does. */
		shortcuts?: boolean;
		/** Omitted with `onforkselect` → no fork entry at all. */
		onfork?: () => void;
		onfollowup?: () => void;
		/** Multi-select-to-fork mode; omitted → plain fork. */
		onforkselect?: () => void;
		forkSelectActive?: boolean;
		/** Its promise settles once the interrupt is acknowledged. */
		oninterrupt: () => void | Promise<void>;
		onarchive: () => void;
		/** Stop-then-archive, fired by the ⌘/Ctrl+E chord. */
		onstoparchive: () => void;
		onTogglePin?: (s: SessionListItem) => void;
		/** Omitted → the account glyph is a read-only indicator. */
		onAccountClick?: () => void;
		onsetroom?: (sessionId: string, pick: { id: string } | { name: string }) => void;
		onclearroom?: (sessionId: string) => void;
		/** Scrolls to a session of the shown thread; false when it is not in it. */
		onthreadjump?: (sessionId: string) => boolean;
	} & LabelHandlers = $props();

	const showLabels = $derived(hasFeature('organising'));

	let renaming = $state(false);
	// svelte-ignore state_referenced_locally
	let newName = $state(nameOf(session));

	function startRename() {
		renaming = true;
		newName = nameOf(session);
	}
	function doRename() {
		const n = newName.trim();
		renaming = false;
		if (!n) return;
		onrename(n);
	}

	const COLLAPSE_BELOW = 640;
	let barWidth = $state(Infinity);
	const collapsed = $derived(compact || barWidth < COLLAPSE_BELOW);

	// One square per density for every control in the bar, so they share a
	// height and a top edge whatever glyph they carry.
	const box: 'sm' | 'md' = $derived(collapsed ? 'sm' : 'md');
	let stopping = $state(false);
	async function stop() {
		if (stopping) return;
		stopping = true;
		try {
			await oninterrupt();
		} finally {
			stopping = false;
		}
	}

	let headEl = $state<HTMLElement>();
	const composer = new ComposerFocus();
	$effect(() => (headEl ? composer.attach(headEl) : undefined));

	// Window-level so the chords work wherever focus sits, including the
	// composer. `shortcuts` is what keeps N tiles from firing N times.
	function onWinKey(e: KeyboardEvent) {
		runHeaderKey(
			e,
			{ active: shortcuts, renaming, archived, canSearch: !!onsearch, archiveShortcut: settings.archiveShortcut },
			{ onsearch, onstoparchive, onescape, onescapeaction: onescapeaction ?? onclose }
		);
	}
</script>

<svelte:window onkeydown={onWinKey} />

<div class="dhead" class:compact class:typing={!compact && composer.collapse} bind:this={headEl} data-journey="header">
	<div class="dbar" bind:clientWidth={barWidth}>
	<Toolbar density={collapsed ? 'compact' : 'default'}>
		{#if onclose}
			<IconButton icon="chevron-left" label={m.drawer_back()} {box} onclick={onclose} />
		{/if}
		<SessionGlyphs
			{session}
			{livenessClass}
			stack={collapsed ? 'always' : 'never'}
			showAccountName={settings.accountNames}
			{onTogglePin}
			{onAccountClick}
		/>
		<RebindTrail sessionId={session.id} />
		<DrawerTitle {session} {compact} {showLabels} {labels} {renaming} bind:name={newName} onsubmit={doRename} onrename={startRename} />
		{#if renaming}
			<IconButton {box} variant="ghost" icon="check" label={m.common_save()} onclick={doRename} />
		{/if}
		{#if onmaximize}
			<IconButton
				{box}
				variant="ghost"
				icon={maximized ? 'grid' : 'external'}
				label={maximized ? m.tiles_restore() : m.tiles_maximize()}
				onclick={onmaximize}
			/>
		{/if}
		{#if !archived && !compact}
			<IconButton
				{box}
				variant="ghost"
				tone="danger"
				icon="stop"
				label={m.drawer_interrupt_label()}
				title={m.drawer_interrupt_title()}
				spin={stopping}
				onclick={stop}
				data-journey="stop"
			/>
			<IconButton {box} variant="ghost" tone="warn" icon="archive" label={m.drawer_archive()} onclick={onarchive} />
		{/if}
		<DrawerMenu
			{session}
			{archived}
			{box}
			onrename={startRename}
			{onsearch}
			{oninterrupt}
			{onarchive}
			{oncopylink}
			{oncopymarkdown}
			{onexport}
			{onfork}
			{onfollowup}
			{onforkselect}
			{forkSelectActive}
			{onsetroom}
			{onclearroom}
		/>
	</Toolbar>
	</div>
	{#if showLabels && session.labels.length > 0 && !compact}
		<div class="hlabels">
			<HeaderLabels {session} handlers={labels} />
		</div>
	{/if}
	{#if !compact}
		<HeaderMeta
			{session}
			{archived}
			{isCodexSession}
			{showStatusBadge}
			{box}
			{onsetmodel}
			{onfork}
			{onthreadjump}
		/>
	{/if}
</div>

<style>
	.dhead {
		position: sticky;
		top: 0;
		z-index: 2;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		background: var(--bg-elevated);
		/* TokenUsage degrades its readout against this container. */
		container: drawer-head / inline-size;
	}
	/* Typing on a phone: the transcript gets the height back. */
	.dhead.typing {
		display: none;
	}
	/* One row, tight: a tile's header may not eat the transcript. */
	.dhead.compact {
		gap: 0;
		padding: var(--sp-1) var(--sp-2);
	}
	.dbar {
		min-width: 0;
	}
	.hlabels {
		display: flex;
		min-width: 0;
	}
</style>
