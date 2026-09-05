<script lang="ts">
	import { goto } from '$app/navigation';
	import { ws } from '$lib/ws.svelte';
	import { useMe, useVersion, useSessions, qk } from '$lib/queries';
	import type { SessionListResponse } from '@bindings/SessionListResponse';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { theme } from '$lib/theme.svelte';
	import { fontScale } from '$lib/fontscale.svelte';
	import { auth } from '$lib/auth.svelte';
	import { notify } from '$lib/notify.svelte';
	import { settings } from '$lib/settings.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { FontScalePicker, IconButton, Menu, Text, ThemePicker } from '@dorsk/tsumikit';
	import type { MenuItem } from '@dorsk/tsumikit';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import MainNav from '$lib/components/organisms/MainNav.svelte';
	import UsageBattery from '$lib/components/molecules/UsageBattery.svelte';
	import UpdateModal from '$lib/components/organisms/UpdateModal.svelte';
	import { m } from '$lib/paraglide/messages';

	const version = useVersion();
	const me = useMe();
	let updateOpen = $state(false);
	const instanceName = $derived(version.data?.instance_name ?? null);
	$effect(() => notify.setInstanceName(instanceName));

	// Global "needs input" watcher. Header is always mounted inside
	// the query provider, so it's the natural home for the cross-route watcher.
	const qc = useQueryClient();
	const sessions = useSessions(() => false);

	// Live ws changes → refetch the list even on routes other than /sessions.
	$effect(() => {
		void ws.changeTick;
		qc.invalidateQueries({ queryKey: ['sessions'] });
	});

	// Cheap per-session ws patches applied to both list caches in place —
	// no refetch. The 15s poll reconciles anything the patch can't know.
	$effect(() =>
		ws.onListPatch((p) => {
			const { session_id, ...fields } = p;
			for (const archived of [false, true]) {
				qc.setQueryData<SessionListResponse>(qk.sessions(archived), (old) =>
					old
						? {
								...old,
								sessions: old.sessions.map((s) =>
									s.id === session_id ? { ...s, ...fields } : s
								)
							}
						: old
				);
			}
		})
	);

	// Drive notifications + title badge off the list's attention flags.
	$effect(() => {
		const items = sessions.data?.sessions ?? [];
		notify.reconcile(items.filter((s) => s.attention === 'needs_input'));
	});

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

	const userName = $derived(me.data?.user_name ?? '');
	const userRole = $derived(me.data?.role ?? '');
	const userInitial = $derived((userName || userRole || '?').slice(0, 1).toUpperCase());
	const roleSuffix = $derived(
		userRole && userRole.toLowerCase() !== userName.toLowerCase() ? userRole : ''
	);
	const latest = $derived(version.data?.latest_version ?? null);

	// The kit pickers write the kit stores; the blob follows so the choice
	// round-trips across devices like it did through the old header select.
	$effect(() => {
		const t = theme.current;
		const f = fontScale.current;
		const d = settings.state.display;
		if (d.theme !== t || d.fontScale !== f) settings.setDisplay({ theme: t, fontScale: f });
	});

	const userMenu = $derived<MenuItem[]>([
		...(latest
			? [
					{
						label: m.nav_update_available({ version: latest }),
						icon: 'arrow-up' as const,
						tag: `v${latest}`,
						tagTone: 'danger' as const,
						onselect: () => (updateOpen = true)
					}
				]
			: []),
		{ label: m.nav_settings(), onselect: () => void goto('/settings') },
		{ label: m.nav_log_out(), icon: 'log-out' as const, danger: true, onselect: () => void auth.logout() }
	]);
</script>

<header class="hd">
	<div class="hd-inner">
		<div class="lead">
			<NavLink href="/sessions" title={m.nav_sessions()}>
				<div class="brand">
					<Text variant="code" tone="accent" size="lg" weight="bold">»_</Text>
					<Text size="lg" weight="bold">cctui</Text>
				</div>
			</NavLink>
			<span
				class="conn"
				class:on={ws.status === 'open'}
				class:mid={ws.status === 'connecting'}
				title={m.nav_ws_status({ status: ws.status })}
			></span>
			<span class="vers">
				<Text size="xs" tone="faint" variant="code">ui v{__CLIENT_VERSION__}</Text>
				{#if version.data}
					<NavLink href={version.data.commit_url} target="_blank" rel="noopener">
						<Text size="xs" tone="faint" variant="code">srv v{version.data.version}</Text>
					</NavLink>
				{/if}
			</span>
			{#if latest}
				<button
					class="upd"
					type="button"
					title={m.nav_update_available({ version: latest })}
					onclick={() => (updateOpen = true)}
				>
					<span class="upd-dot" aria-hidden="true"></span>
					<Text size="xs" tone="danger" variant="code">v{latest}</Text>
				</button>
			{/if}
		</div>
		<div class="tabs">
			{#if settings.nav === 'top'}
				<MainNav placement="top" />
			{/if}
		</div>
		<div class="tail">
			<span class="batt"><UsageBattery /></span>
			<span class="divider" aria-hidden="true"></span>
			<IconButton
				emoji={notify.enabled ? '🔔' : '🔕'}
				size={12}
				label={notify.enabled ? m.nav_notify_on_label() : m.nav_notify_off_label()}
				pressed={notify.enabled}
				onclick={toggleNotify}
				oncontextmenu={(e: MouseEvent) => {
					e.preventDefault();
					settings.setNotifySound(!notify.sound);
					toasts.info(notify.sound ? m.nav_sound_on() : m.nav_sound_off());
				}}
			/>
			<span class="prefs">
				<ThemePicker />
				<FontScalePicker />
			</span>
			<Menu label={m.nav_user_menu()} items={userMenu} bare placement="bottom-end">
				{#snippet trigger()}
					<span class="pill">
						<span class="avatar" class:alert={!!latest} aria-hidden="true">{userInitial}</span>
						<span class="who">
							{#if userName}<span class="who-name">{userName}</span>{/if}
							{#if userName && roleSuffix}<span class="who-sep">·</span>{/if}
							{#if roleSuffix}<span class="who-role">{roleSuffix}</span>{/if}
						</span>
					</span>
				{/snippet}
			</Menu>
		</div>
	</div>
</header>

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
	.hd {
		position: fixed;
		top: 0;
		left: 0;
		right: 0;
		z-index: var(--z-header);
		background: color-mix(in srgb, var(--bg-elevated) 92%, transparent);
		backdrop-filter: blur(8px);
		border-bottom: 1px solid var(--border);
		padding-top: var(--safe-top);

		/* The app scales by changing the ROOT font-size; the header is chrome
		   that must not reflow with it, so its size tokens are pinned in px
		   (the :root rem values at the 16px base). */
		--fs-xs: 12px;
		--fs-sm: 13px;
		--fs-base: 15px;
		--fs-lg: 18px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
		--sp-4: 16px;
	}
	/* Icon-only controls size off rem: pin them to one fixed square. */
	.hd :global(.btn-icon) {
		flex: none;
		height: 36px;
		width: 36px;
		min-height: 36px;
		min-width: 36px;
		padding: 0;
	}
	.hd :global(.select-button) {
		flex: none;
		height: 36px;
		width: 36px;
		min-width: 36px;
	}
	.hd-inner {
		width: 100%;
		padding-inline: max(var(--sp-4), var(--safe-left)) max(var(--sp-4), var(--safe-right));
		height: var(--header-h);
		/* Three equal tracks so the nav sits on the header's true centre,
		   whatever the brand and the right-hand cluster weigh. */
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		align-items: center;
		gap: var(--sp-2);
	}
	.lead,
	.tail {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.tail {
		justify-content: flex-end;
	}
	.brand {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.tabs {
		display: none;
		min-width: 0;
		align-self: stretch;
		justify-content: center;
	}
	@media (min-width: 48rem) {
		.tabs {
			display: flex;
		}
	}
	/* No room for the centre track: the brand keeps its width, the cluster takes the rest. */
	@media (max-width: 47.999rem) {
		.hd-inner {
			grid-template-columns: auto minmax(0, 1fr);
		}
	}
	.conn {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--dot-dead);
		flex: none;
	}
	.conn.on {
		background: var(--ok);
		box-shadow: 0 0 6px var(--ok);
	}
	.conn.mid {
		background: var(--warn);
	}
	.vers {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		flex: none;
		line-height: 1.15;
		white-space: nowrap;
	}
	.upd {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
		height: 20px;
		padding: 0 var(--sp-2);
		border: 1px solid var(--danger);
		border-radius: var(--r-pill);
		background: none;
		cursor: pointer;
	}
	.upd-dot {
		width: 6px;
		height: 6px;
		border-radius: 50%;
		background: var(--danger);
	}
	.prefs {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		flex: none;
	}
	.batt {
		display: inline-flex;
		flex: none;
	}
	.batt:empty {
		display: none;
	}
	.divider {
		flex: none;
		width: 1px;
		height: 20px;
		background: var(--border);
	}
	.batt:empty + .divider {
		display: none;
	}
	.pill {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		height: 32px;
		padding: 0 var(--sp-3) 0 var(--sp-1);
		border: 1px solid var(--border);
		border-radius: var(--r-pill);
		color: var(--text);
		font-size: var(--fs-sm);
		max-width: 16rem;
	}
	.pill:hover {
		background: var(--bg-elevated-2);
	}
	.avatar {
		position: relative;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 24px;
		height: 24px;
		border-radius: 50%;
		background: color-mix(in srgb, var(--accent) 18%, transparent);
		color: var(--accent);
		font-size: var(--fs-xs);
		font-weight: var(--fw-semibold);
		flex: none;
	}
	.avatar.alert::after {
		content: '';
		position: absolute;
		top: -1px;
		right: -1px;
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--danger);
		border: 2px solid var(--bg-elevated);
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
