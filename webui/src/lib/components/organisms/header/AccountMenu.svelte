<script lang="ts">
	import { useMe, useVersion } from '#lib/queries/index.ts';
	import { releaseChannel, versionLine } from '#lib/releaseChannel.ts';
	import { auth } from '#lib/auth.svelte.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { notify } from '#lib/notify.svelte.ts';
	import { notifications } from '#lib/notifications.svelte.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { Avatar, Badge, Dot, Menu } from '@dorsk/tsumikit';
	import type { MenuItem } from '@dorsk/tsumikit';
	import UpdateModal from '#lib/components/organisms/UpdateModal.svelte';
	import NotificationBell from '#lib/components/organisms/NotificationBell.svelte';
	import { m } from '#lib/paraglide/messages.js';

	const version = useVersion();
	const me = useMe();
	let updateOpen = $state(false);
	let inboxOpen = $state(false);
	const unread = $derived(notifications.unread);

	async function toggleNotify() {
		if (settings.notifyEnabled) {
			await settings.setNotifyEnabled(false);
			toasts.info(m.nav_notify_off());
			return;
		}
		if (!notify.supported) {
			toasts.error(m.nav_notify_unsupported());
			return;
		}
		const ok = await settings.setNotifyEnabled(true);
		if (ok) toasts.ok(m.nav_notify_on());
		else toasts.error(m.nav_notify_blocked());
	}

	const userName = $derived(me.data?.user_name ?? '');
	const userRole = $derived(me.data?.role ?? '');
	const roleSuffix = $derived(
		userRole && userRole.toLowerCase() !== userName.toLowerCase() ? userRole : ''
	);
	const latest = $derived(version.data?.latest_version ?? null);
	const canUpdate = $derived(hasFeature('self_update'));
	const notifyLabel = $derived(
		settings.notifyEnabled ? m.nav_notify_on_label() : m.nav_notify_off_label()
	);

	const srvVersion = $derived(version.data?.version);
	const items = $derived<MenuItem[]>([
		{
			label: versionLine(__CLIENT_VERSION__, srvVersion),
			icon: 'git-commit' as const,
			disabled: true,
			tag:
				releaseChannel(srvVersion ?? __CLIENT_VERSION__) === 'beta'
					? m.release_channel_beta()
					: undefined
		},
		...(latest && canUpdate
			? [
					{
						label: m.nav_update_available({ version: latest }),
						icon: 'arrow-up' as const,
						tag: `v${latest}`,
						tagTone: 'danger' as const,
						onselect: () => (updateOpen = true)
					}
				]
			: latest
				? [{ label: m.nav_update_available({ version: latest }), icon: 'arrow-up' as const, disabled: true }]
				: []),
		{
			label: m.notif_center_title(),
			icon: 'bell' as const,
			tag: unread > 0 ? String(unread) : undefined,
			tagTone: 'accent' as const,
			onselect: () => (inboxOpen = true)
		},
		{
			label: notifyLabel,
			icon: 'bell' as const,
			pressed: settings.notifyEnabled,
			onselect: () => void toggleNotify()
		},
		{ label: m.nav_log_out(), icon: 'log-out' as const, danger: true, onselect: () => void auth.logout() }
	]);
</script>

{#snippet alertDot()}
	<Dot color="var(--danger)" />
{/snippet}

<span class="acct">
	<Menu label={m.nav_user_menu()} {items} bare placement="bottom-end">
		{#snippet trigger()}
			<span class="pill">
				<Avatar
					name={userName || userRole}
					tone="accent"
					size={24}
					decorative
					status={latest ? alertDot : undefined}
				/>
				<span class="who">
					{#if userName}<span class="who-name">{userName}</span>{/if}
					{#if userName && roleSuffix}<span class="who-sep">·</span>{/if}
					{#if roleSuffix}<span class="who-role">{roleSuffix}</span>{/if}
				</span>
				{#if unread > 0}<Badge size="xs" numeric tone="accent" border={false}>{unread}</Badge>{/if}
			</span>
		{/snippet}
	</Menu>
</span>

<NotificationBell bind:open={inboxOpen} bell={false} />

{#if updateOpen && canUpdate && version.data?.latest_version}
	<UpdateModal
		latestVersion={version.data.latest_version}
		latestUrl={version.data.latest_url ?? version.data.repo_url}
		selfUpdateReady={version.data.self_update_ready}
		selfUpdateHook={version.data.self_update_hook}
		onclose={() => (updateOpen = false)}
	/>
{/if}

<style>
	.acct {
		display: inline-flex;
	}
	.pill {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		height: 32px;
		padding: 0 var(--sp-3) 0 var(--sp-1);
		border-radius: var(--r-pill);
		background: var(--bg-elevated-2);
		color: var(--text);
		font-size: var(--fs-sm);
		max-width: 16rem;
	}
	.pill:hover {
		background: color-mix(in srgb, var(--bg-elevated-2) 85%, var(--text));
	}
	@media (pointer: coarse), (max-width: 47.999rem) {
		.pill {
			min-height: 44px;
			justify-content: center;
			min-width: 44px;
		}
	}
	.who {
		display: inline-flex;
		align-items: baseline;
		gap: var(--sp-1);
		min-width: 0;
		white-space: nowrap;
	}
	.who-name {
		font-weight: var(--fw-medium);
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.who-sep,
	.who-role {
		color: var(--text-muted);
	}
	@media (max-width: 47.999rem) {
		.who {
			display: none;
		}
		.pill {
			padding-right: var(--sp-1);
		}
	}
</style>
