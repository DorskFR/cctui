<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { settings } from '#lib/settings.svelte.ts';
	import RoomMenu from '#lib/components/molecules/RoomMenu.svelte';
	import KeepaliveModal from '#lib/components/molecules/KeepaliveModal.svelte';
	import CctuiverseInviteModal from '#lib/components/molecules/CctuiverseInviteModal.svelte';
	import CctuiverseJoinModal from '#lib/components/molecules/CctuiverseJoinModal.svelte';
	import { cctuiverseConfig, loadCctuiverseConfig } from '#lib/cctuiverseConfig.svelte.ts';
	import { Button, Icon, Menu, Modal, Text, type MenuItem } from '@dorsk/tsumikit';
	import { nameOf } from '#lib/sessionName.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { hasFeature } from '#lib/features.svelte.ts';

	let {
		session,
		archived,
		box,
		onrename,
		onsearch,
		oninterrupt,
		onarchive,
		oncopylink,
		oncopymarkdown,
		onexport,
		onfork,
		onfollowup,
		onforkselect,
		forkSelectActive = false,
		onsetroom,
		onclearroom
	}: {
		session: SessionListItem;
		archived: boolean;
		box: 'sm' | 'md';
		onrename: () => void;
		onsearch?: () => void;
		oninterrupt: () => void;
		onarchive: () => void;
		oncopylink: () => void;
		oncopymarkdown: () => void;
		onexport: () => void;
		onfork?: () => void;
		onfollowup?: () => void;
		onforkselect?: () => void;
		forkSelectActive?: boolean;
		onsetroom?: (sessionId: string, pick: { id: string } | { name: string }) => void;
		onclearroom?: (sessionId: string) => void;
	} = $props();

	const plainTitle = $derived(nameOf(session) || session.working_dir);

	let keepaliveOpen = $state(false);
	let roomOpen = $state(false);
	let inviteOpen = $state(false);
	let joinOpen = $state(false);

	$effect(() => {
		void loadCctuiverseConfig();
	});

	const forkAction = $derived(onforkselect ?? onfork);

	const followupItem = $derived<MenuItem | null>(
		onfollowup ? { label: m.drawer_followup_label(), icon: 'arrow-right', onselect: onfollowup } : null
	);

	// Each group opens with a non-focusable heading row; an empty group drops out.
	function group(label: string, items: MenuItem[]): MenuItem[] {
		return items.length ? [{ id: `group:${label}`, label, control: groupHeading }, ...items] : [];
	}

	const overflowItems = $derived<MenuItem[]>([
		...group(m.drawer_menu_group_share(), [
			{
				label: m.drawer_copy_link_label(),
				icon: 'link' as const,
				attrs: { title: m.drawer_copy_link_title() },
				onselect: oncopylink
			},
			{
				label: m.drawer_copy_markdown_label(),
				icon: 'markdown' as const,
				attrs: { title: m.drawer_copy_markdown_title() },
				onselect: oncopymarkdown
			},
			{
				label: m.drawer_export_label(),
				icon: 'download' as const,
				attrs: { title: m.drawer_export_title() },
				onselect: onexport
			}
		]),
		...group(m.drawer_menu_group_branch(), [
			...(followupItem && settings.preferFollowupOverFork ? [followupItem] : []),
			...(forkAction
				? [
						{
							label: m.drawer_fork_label(),
							icon: 'fork' as const,
							pressed: onforkselect ? forkSelectActive : undefined,
							attrs: { title: onforkselect ? m.drawer_fork_select_title() : m.drawer_fork_title() },
							onselect: forkAction
						}
					]
				: []),
			...(followupItem && !settings.preferFollowupOverFork ? [followupItem] : [])
		]),
		...group(m.drawer_menu_group_collaborate(), [
			...(onsetroom && hasFeature('rooms')
				? [
						{
							label: m.rooms_menu_action(),
							icon: 'grid' as const,
							attrs: { title: session.room_name ?? m.rooms_menu_label() },
							onselect: () => (roomOpen = true)
						}
					]
				: []),
			...(cctuiverseConfig.enabled && !archived
				? [
						{
							label: m.cctuiverse_menu_invite(),
							icon: 'link' as const,
							onselect: () => (inviteOpen = true)
						},
						{
							label: m.cctuiverse_menu_join(),
							icon: 'link' as const,
							onselect: () => (joinOpen = true)
						}
					]
				: [])
		]),
		...group(m.drawer_menu_group_session(), [
			...(onsearch
				? [
						{
							label: m.conversation_search_label(),
							icon: 'search' as const,
							attrs: { title: m.conversation_search_title() },
							onselect: onsearch
						}
					]
				: []),
			...(!archived
				? [
						{
							label: m.drawer_interrupt_label(),
							icon: 'stop' as const,
							attrs: { title: m.drawer_interrupt_title() },
							onselect: oninterrupt
						}
					]
				: []),
			{ label: m.drawer_rename(), icon: 'edit' as const, onselect: onrename },
			...(hasFeature('automation')
				? [
						{
							label: m.drawer_keepalive_label(),
							icon: 'recycle' as const,
							pressed: !!session.keepalive,
							onselect: () => (keepaliveOpen = true)
						}
					]
				: []),
			...(archived ? [] : [{ label: m.drawer_archive(), icon: 'archive' as const, onselect: onarchive }])
		])
	]);
</script>

{#snippet groupHeading(item: MenuItem)}
	<Text size="xs" tone="faint" weight="medium">{item.label}</Text>
{/snippet}

<Menu label={m.drawer_more_actions()} items={overflowItems} placement="bottom-end" variant="ghost" {box}>
	{#snippet trigger()}
		<span class="mtrig" data-journey="actions" title={m.drawer_more_actions()}>
			<Icon name="more" size={18} />
		</span>
	{/snippet}
</Menu>

{#if keepaliveOpen}
	<KeepaliveModal {session} onclose={() => (keepaliveOpen = false)} />
{/if}

{#if inviteOpen}
	<CctuiverseInviteModal
		target={{ session: session.id }}
		defaultLabel={plainTitle}
		onclose={() => (inviteOpen = false)}
	/>
{/if}

{#if joinOpen}
	<CctuiverseJoinModal sessionId={session.id} defaultLabel={plainTitle} onclose={() => (joinOpen = false)} />
{/if}

{#if roomOpen}
	<Modal title={m.rooms_menu_label()} onclose={() => (roomOpen = false)}>
		{#snippet body()}
			<RoomMenu
				current={session.room_id ?? null}
				onpick={(pick) => {
					onsetroom?.(session.id, pick);
					roomOpen = false;
				}}
				onclear={session.room_id
					? () => {
							onclearroom?.(session.id);
							roomOpen = false;
						}
					: undefined}
			/>
		{/snippet}
		{#snippet footer()}
			<Button size="sm" variant="ghost" onclick={() => (roomOpen = false)}>{m.common_close()}</Button>
		{/snippet}
	</Modal>
{/if}

<style>
	.mtrig {
		display: inline-flex;
		align-items: center;
		line-height: 1;
	}
</style>
