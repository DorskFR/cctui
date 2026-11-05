<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { Label } from '@bindings/Label';
	import { settings } from '$lib/settings.svelte';
	import { headerKeyAction } from './headerKeys';
	import { drawerMenuGroups, groupedMenu } from './drawerMenu';
	import RebindTrail from '$lib/components/molecules/RebindTrail.svelte';
	import SessionGlyphs from '$lib/components/molecules/SessionGlyphs.svelte';
	import LabelBadge from '$lib/components/molecules/LabelBadge.svelte';
	import RoomBadge from '$lib/components/molecules/RoomBadge.svelte';
	import CctuiverseLinkChips from '$lib/components/molecules/CctuiverseLinkChips.svelte';
	import { cctuiverseConfig, loadCctuiverseConfig } from '$lib/cctuiverseConfig.svelte';
	import {
		Button,
		FontScalePicker,
		Icon,
		IconButton,
		Input,
		Menu,
		Spinner,
		Text,
		Toolbar
	} from '@dorsk/tsumikit';
	import HeaderMeta from './HeaderMeta.svelte';
	import DrawerHeaderModals from './DrawerHeaderModals.svelte';
	import { headerTitleOf, nameOf } from '$lib/sessionName';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		archived,
		isCodexSession,
		livenessClass,
		showStatusBadge,
		working = false,
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
		allLabels = [],
		onCreateLabel,
		onAttachLabel,
		onDetachLabel,
		onUpdateLabel,
		onDeleteLabel,
		onsetroom,
		onclearroom
	}: {
		session: SessionListItem;
		archived: boolean;
		isCodexSession: boolean;
		livenessClass: string;
		showStatusBadge: boolean;
		/** A turn is in flight: the only time Stop is offered. */
		working?: boolean;
		maximized?: boolean;
		/** Tile chrome: one row, no meta row, no label strip. */
		compact?: boolean;
		onmaximize?: () => void;
		onclose?: () => void;
		onrename: (name: string) => void;
		onsetmodel: (model: string, effort: string) => void;
		oncopylink: () => void;
		oncopymarkdown: () => void;
		onexport: () => void;
		onsearch?: () => void;
		/** First refusal on Escape: true when it was consumed and the drawer must stay open. */
		onescape?: () => boolean;
		/** What an unconsumed Escape does instead of closing. */
		onescapeaction?: () => void;
		/** Whether this header owns the window chords. One tile at a time does. */
		shortcuts?: boolean;
		onfork: () => void;
		onfollowup?: () => void;
		/** Omitted where the harness has no partial fork. */
		onforkselect?: () => void;
		forkSelectActive?: boolean;
		oninterrupt: () => void;
		onarchive: () => void;
		onstoparchive: () => void;
		onTogglePin?: (s: SessionListItem) => void;
		onAccountClick?: () => void;
		allLabels?: Label[];
		onCreateLabel?: (name: string, color: string) => Promise<Label>;
		onAttachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onDetachLabel?: (id: string, labelId: string) => void | Promise<void>;
		onUpdateLabel?: (labelId: string, patch: { name?: string; color?: string }) => Promise<Label>;
		onDeleteLabel?: (labelId: string) => void | Promise<void>;
		onsetroom?: (sessionId: string, pick: { id: string } | { name: string }) => void;
		onclearroom?: (sessionId: string) => void;
	} = $props();

	const labelEditable = $derived(!!onAttachLabel);

	const plainTitle = $derived(nameOf(session) || session.working_dir);
	const headTitle = $derived(headerTitleOf(session));

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

	let keepaliveOpen = $state(false);
	let roomOpen = $state(false);
	let inviteOpen = $state(false);
	let joinOpen = $state(false);

	$effect(() => {
		void loadCctuiverseConfig();
	});

	// Mirrors the Toolbar's `collapseBelow`.
	const COLLAPSE_BELOW = 640;
	let barWidth = $state(Infinity);
	const collapsed = $derived(compact || barWidth < COLLAPSE_BELOW);
	// A narrow drawer gives the title its own line under the controls.
	const titleOwnLine = $derived(collapsed && !compact);
	const stoppable = $derived(working && !archived);

	const box: 'sm' | 'md' = $derived(collapsed ? 'sm' : 'md');
	const CHIP_CHROME = '--pop-trigger-border: var(--border-strong); --pop-trigger-bg: var(--surface)';

	const overflowItems = $derived(
		groupedMenu(
			drawerMenuGroups(
				{
					archived,
					compact,
					collapsed,
					renaming,
					keepalive: !!session.keepalive,
					preferFollowup: settings.preferFollowupOverFork,
					cctuiverse: cctuiverseConfig.enabled,
					roomName: session.room_name ?? null,
					forkSelectActive,
					textSize: textSizeRow
				},
				{
					copylink: oncopylink,
					copymarkdown: oncopymarkdown,
					export: onexport,
					fork: onfork,
					forkselect: onforkselect,
					followup: onfollowup,
					search: onsearch,
					room: onsetroom ? () => (roomOpen = true) : undefined,
					invite: () => (inviteOpen = true),
					join: () => (joinOpen = true),
					rename: startRename,
					save: doRename,
					keepalive: () => (keepaliveOpen = true),
					archive: onarchive
				}
			),
			menuSeparator
		)
	);

	// Window-level so the chords work wherever focus sits, including the
	// composer. `shortcuts` is what keeps N tiles from firing N times.
	function onWinKey(e: KeyboardEvent) {
		const action = headerKeyAction(e, {
			active: shortcuts,
			renaming,
			archived,
			canSearch: !!onsearch,
			archiveShortcut: settings.archiveShortcut
		});
		if (action === 'search') {
			e.preventDefault();
			onsearch?.();
			return;
		}
		if (action === 'archive') {
			e.preventDefault();
			onstoparchive();
			return;
		}
		if (action !== 'escape') return;
		if (onescape?.()) {
			e.preventDefault();
			return;
		}
		(onescapeaction ?? onclose)?.();
	}
</script>

<svelte:window onkeydown={onWinKey} />

{#snippet menuSeparator()}
	<span class="msep" role="separator"></span>
{/snippet}

{#snippet textSizeRow()}
	<span class="mrow-label">{m.chrome_text_size()}</span>
	<FontScalePicker box="sm" />
{/snippet}

{#snippet titleBlock()}
	<div class="dtitle">
		{#if renaming}
			<Input bind:value={newName} aria-label={m.a11y_rename_session()} onsubmit={doRename} />
		{:else}
			<Text weight="semibold" size="md" truncate>{headTitle}</Text>
			{#if session.labels.length === 0 && labelEditable}
				<LabelBadge
					labels={[]}
					editable
					{allLabels}
					onCreate={onCreateLabel}
					onAttach={(lid) => onAttachLabel?.(session.id, lid)}
					onDetach={(lid) => onDetachLabel?.(session.id, lid)}
					onUpdate={onUpdateLabel}
					onDelete={onDeleteLabel}
				/>
			{/if}
			<RoomBadge name={session.room_name} />
			{#if !compact}
				<CctuiverseLinkChips target={{ session: session.id }} />
			{/if}
		{/if}
	</div>
{/snippet}

<div class="dhead" class:compact data-journey="header">
	<div class="dbar" class:compact={collapsed} bind:clientWidth={barWidth}>
		<Toolbar collapseBelow="{COLLAPSE_BELOW}px" density={collapsed ? 'compact' : 'default'}>
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
			{#if titleOwnLine}
				<span class="spacer"></span>
			{:else}
				{@render titleBlock()}
			{/if}
			{#if renaming}
				<IconButton chip {box} variant="default" icon="check" label={m.common_save()} onclick={doRename} />
			{/if}
			{#if onmaximize}
				<IconButton
					chip
					{box}
					variant="default"
					icon={maximized ? 'grid' : 'external'}
					label={maximized ? m.tiles_restore() : m.tiles_maximize()}
					onclick={onmaximize}
				/>
			{/if}
			{#if onsearch}
				<IconButton
					data-overflow
					data-journey="find"
					chip
					{box}
					variant="default"
					icon="search"
					label={m.conversation_search_label()}
					title={m.conversation_search_title()}
					onclick={onsearch}
				/>
			{/if}
			{#if stoppable}
				<Button
					size={box}
					title={m.chrome_stop_title()}
					data-journey="stop"
					onclick={oninterrupt}
				>
					<Spinner size={12} label={undefined} />{m.chrome_stop()}
				</Button>
			{/if}
			<Menu label={m.drawer_more_actions()} items={overflowItems} placement="bottom-end" {box} style={CHIP_CHROME}>
				{#snippet trigger()}
					<span class="mtrig" data-journey="actions" title={m.drawer_more_actions()}>
						<Icon name="more" size={18} />
					</span>
				{/snippet}
			</Menu>
		</Toolbar>
	</div>
	{#if titleOwnLine}
		{@render titleBlock()}
	{/if}
	{#if session.labels.length > 0 && !compact}
		<div class="hlabels">
			<LabelBadge
				labels={session.labels}
				editable={labelEditable}
				{allLabels}
				onCreate={onCreateLabel}
				onAttach={(lid) => onAttachLabel?.(session.id, lid)}
				onDetach={(lid) => onDetachLabel?.(session.id, lid)}
				onUpdate={onUpdateLabel}
				onDelete={onDeleteLabel}
			/>
		</div>
	{/if}
	{#if !compact}
		<HeaderMeta {session} {archived} {isCodexSession} {showStatusBadge} {box} {onsetmodel} {onfork} />
	{/if}
</div>

<DrawerHeaderModals
	{session}
	title={plainTitle}
	bind:keepaliveOpen
	bind:roomOpen
	bind:inviteOpen
	bind:joinOpen
	{onsetroom}
	{onclearroom}
/>

<style>
	.dhead {
		position: sticky;
		top: 0;
		z-index: 2;
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		background: var(--bg-elevated);
		container: drawer-head / inline-size;
	}
	.dhead.compact {
		gap: 0;
		padding: var(--sp-1) var(--sp-2);
	}
	/* The kit sizes a chip IconButton from `--box-lg`; pinning it to the row's
	   box keeps every control square at the bar's density. */
	.dbar {
		--box-lg: var(--box-md);
		min-width: 0;
	}
	.dbar.compact {
		--box-lg: var(--box-sm);
	}
	/* Touch: every header and meta control is at least a full touch target. */
	@media (pointer: coarse) {
		.dhead {
			--touch-slab: min(-8px, calc((100% - var(--hit-min)) / 2));
			--box-sm: var(--hit-min);
			--box-md: var(--hit-min);
			--box-lg: var(--hit-min);
		}
		.dbar,
		.dbar.compact {
			--box-lg: var(--hit-min);
		}
	}
	.hlabels {
		display: flex;
		min-width: 0;
	}
	.mtrig {
		display: inline-flex;
		align-items: center;
		line-height: 1;
	}
	.dtitle {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.spacer {
		flex: 1;
	}
	.msep {
		flex: 1;
		border-top: 1px solid var(--border);
	}
	.mrow-label {
		flex: 1;
		font-size: var(--fs-sm);
	}
</style>
