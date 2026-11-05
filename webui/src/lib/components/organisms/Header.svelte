<script lang="ts">
	import { updated } from '$app/state';
	import { ws } from '$lib/ws.svelte';
	import { useVersion, useSessions, qk } from '$lib/queries';
	import { setUploadCaps } from '$lib/uploadCaps.svelte';
	import { versionLine } from '$lib/releaseChannel';
	import type { SessionListResponse } from '@bindings/SessionListResponse';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { fontScale } from '$lib/fontscale.svelte';
	import { notify } from '$lib/notify.svelte';
	import { settings } from '$lib/settings.svelte';
	import { Text } from '@dorsk/tsumikit';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import MainNav from '$lib/components/organisms/MainNav.svelte';
	import AccountMenu from '$lib/components/organisms/header/AccountMenu.svelte';
	import HeaderStatus from '$lib/components/organisms/header/HeaderStatus.svelte';
	import { m } from '$lib/paraglide/messages';

	const version = useVersion();
	const instanceName = $derived(version.data?.instance_name ?? null);
	$effect(() => notify.setInstanceName(instanceName));
	$effect(() => setUploadCaps(version.data?.upload_caps));

	// Header is always mounted inside the query provider, so it hosts the
	// cross-route list watcher that drives notifications.
	const qc = useQueryClient();
	const sessions = useSessions(() => false);

	$effect(() => {
		void ws.changeTick;
		qc.invalidateQueries({ queryKey: qk.sessionsAll });
	});

	$effect(() =>
		ws.onListPatch((p) => {
			const { session_id, ...fields } = p;
			for (const key of [qk.sessions(false), qk.sessions(true), qk.sessions(false, true)]) {
				qc.setQueryData<SessionListResponse>(key, (old) =>
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

	const needsInput = $derived(
		(sessions.data?.sessions ?? []).filter((s) => s.attention === 'needs_input')
	);
	$effect(() => notify.reconcile(needsInput));

	// The kit font picker writes the kit store; the blob follows so the choice
	// round-trips across devices.
	$effect(() => {
		const f = fontScale.current;
		const d = settings.state.display;
		if (d.fontScale !== f) settings.setDisplay({ fontScale: f });
	});

	const srvVersion = $derived(version.data?.version);
	const mismatch = $derived(!!srvVersion && srvVersion !== __CLIENT_VERSION__);
	const latest = $derived(version.data?.latest_version ?? null);
	let updateOpen = $state(false);
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
			{#if mismatch || latest || updated.current}
				<span class="vers">
					{#if updated.current}
						<button
							type="button"
							class="chip"
							title={m.reload_new_ui_title()}
							onclick={() => location.reload()}>{m.reload_new_ui_hint()}</button
						>
					{:else if latest}
						<button type="button" class="chip" onclick={() => (updateOpen = true)}
							>{m.shell_update_chip({ version: latest })}</button
						>
					{:else if mismatch}
						<span class="chip static" title={m.shell_version_mismatch_title()}>
							<Text size="xs" tone="muted" numeric
								>{versionLine(__CLIENT_VERSION__, srvVersion)}</Text
							>
						</span>
					{/if}
				</span>
			{/if}
		</div>
		<div class="tabs">
			{#if settings.nav === 'top'}
				<MainNav placement="top" />
			{/if}
		</div>
		<div class="tail">
			<HeaderStatus />
			<AccountMenu bind:updateOpen />
		</div>
	</div>
</header>

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
		--box-xs: 24px;
		--box-sm: 30px;
		--box-md: 36px;
		--box-lg: 40px;
	}
	.hd-inner {
		width: 100%;
		padding-inline: max(var(--sp-4), var(--safe-left)) max(var(--sp-4), var(--safe-right));
		height: var(--header-h);
		/* The brand and the account cluster take the width they need; the nav
		   centres in what is left. */
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
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
		display: inline-flex;
		flex: none;
		min-width: 0;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		min-height: var(--hit-min, 24px);
		padding: 0 8px;
		border: 0;
		border-radius: var(--r-pill);
		background: var(--bg-elevated-2);
		color: var(--text);
		font: inherit;
		font-size: 12px;
		font-weight: 600;
		white-space: nowrap;
		cursor: pointer;
	}
	.chip:hover {
		background: color-mix(in srgb, var(--bg-elevated-2) 80%, var(--text) 8%);
	}
	.chip.static {
		cursor: default;
		background: none;
	}
	@media (max-width: 47.999rem) {
		.chip.static {
			display: none;
		}
	}
</style>
