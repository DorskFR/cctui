<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { goto } from '$app/navigation';
	import { Avatar, Dot, Menu } from '@dorsk/tsumikit';
	import type { MenuItem } from '@dorsk/tsumikit';
	import { auth } from '$lib/auth.svelte';
	import { notify } from '$lib/notify.svelte';
	import { settings } from '$lib/settings.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { endpoints, qk, useMe, useVersion } from '$lib/queries';
	import { releaseChannel, versionLine } from '$lib/releaseChannel';
	import UpdateModal from '$lib/components/organisms/UpdateModal.svelte';
	import AppearanceModal from './AppearanceModal.svelte';
	import { m } from '$lib/paraglide/messages';

	const qc = useQueryClient();
	const version = useVersion();
	const me = useMe();
	let { updateOpen = $bindable(false) }: { updateOpen?: boolean } = $props();
	let appearanceOpen = $state(false);
	let checking = $state(false);

	const userName = $derived(me.data?.user_name ?? '');
	const userRole = $derived(me.data?.role ?? '');
	const roleSuffix = $derived(
		userRole && userRole.toLowerCase() !== userName.toLowerCase() ? userRole : ''
	);
	const isAdmin = $derived(userRole === 'admin');
	const latest = $derived(version.data?.latest_version ?? null);
	const srvVersion = $derived(version.data?.version);

	async function toggleNotify() {
		if (notify.enabled) {
			notify.disable();
			settings.recordNotifyEnabled();
			toasts.info(m.nav_notify_off());
			return;
		}
		if (!notify.supported) {
			toasts.error(m.nav_notify_unsupported());
			return;
		}
		const ok = await notify.enable();
		settings.recordNotifyEnabled();
		if (ok) toasts.ok(m.nav_notify_on());
		else toasts.error(m.nav_notify_blocked());
	}

	async function checkForUpdate() {
		checking = true;
		try {
			const info = await endpoints.refreshVersion();
			qc.setQueryData(qk.version, info);
			if (info.latest_version) updateOpen = true;
			else toasts.ok(m.settings_version_up_to_date());
		} catch (e) {
			toasts.error(
				m.settings_version_check_failed({ error: e instanceof Error ? e.message : String(e) })
			);
		} finally {
			checking = false;
		}
	}

	const items = $derived<MenuItem[]>([
		latest
			? {
					label: m.nav_update_available({ version: latest }),
					icon: 'arrow-up' as const,
					tag: `v${latest}`,
					tagTone: 'warn' as const,
					onselect: () => (updateOpen = true)
				}
			: {
					label: checking
						? m.settings_version_checking()
						: m.shell_version_check({
								version: versionLine(__CLIENT_VERSION__, srvVersion).replace(/^v/, '')
							}),
					icon: 'git-commit' as const,
					disabled: checking,
					tag:
						releaseChannel(srvVersion ?? __CLIENT_VERSION__) === 'beta'
							? m.release_channel_beta()
							: undefined,
					onselect: () => void checkForUpdate()
				},
		{
			label: m.shell_appearance(),
			icon: 'palette' as const,
			onselect: () => (appearanceOpen = true)
		},
		...(isAdmin
			? [{ label: m.shell_events(), icon: 'list' as const, onselect: () => void goto('/events') }]
			: []),
		{
			label: notify.enabled ? m.nav_notify_on_label() : m.nav_notify_off_label(),
			icon: 'bell' as const,
			pressed: notify.enabled,
			onselect: () => void toggleNotify()
		},
		{ label: m.nav_log_out(), icon: 'log-out' as const, danger: true, onselect: () => void auth.logout() }
	]);
</script>

{#snippet alertDot()}
	<Dot color="var(--warn)" />
{/snippet}

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
		</span>
	{/snippet}
</Menu>

{#if appearanceOpen}
	<AppearanceModal onclose={() => (appearanceOpen = false)} />
{/if}

{#if updateOpen && version.data?.latest_version}
	<UpdateModal
		latestVersion={version.data.latest_version}
		latestUrl={version.data.latest_url ?? version.data.repo_url}
		selfUpdateReady={version.data.self_update_ready}
		selfUpdateHook={version.data.self_update_hook}
		onclose={() => (updateOpen = false)}
	/>
{/if}

<style>
	/* Lives in the px-pinned header: every length is px, never rem. */
	.pill {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		height: 32px;
		min-height: var(--hit-min, 24px);
		min-width: var(--hit-min, 24px);
		padding: 0 12px 0 4px;
		border-radius: var(--r-pill);
		color: var(--text);
		font-size: 13px;
		max-width: 16rem;
	}
	.pill:hover {
		background: var(--bg-elevated-2);
	}
	.who {
		display: inline-flex;
		align-items: baseline;
		gap: 4px;
		min-width: 0;
		white-space: nowrap;
	}
	.who-name {
		font-weight: 600;
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
			justify-content: center;
			padding: 0 10px;
		}
	}
</style>
