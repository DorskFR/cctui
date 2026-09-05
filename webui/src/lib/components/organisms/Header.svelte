<script lang="ts">
	import { ws } from '$lib/ws.svelte';
	import { useVersion, useSessions, qk } from '$lib/queries';
	import type { SessionListResponse } from '@bindings/SessionListResponse';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { AUTO, theme, THEMES } from '$lib/theme.svelte';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { notify } from '$lib/notify.svelte';
	import { settings } from '$lib/settings.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { Button, IconButton, SelectButton, Text } from '@dorsk/tsumikit';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import NetStatsChip from '$lib/components/molecules/NetStatsChip.svelte';
	import UpdateModal from '$lib/components/organisms/UpdateModal.svelte';
	import { m } from '$lib/paraglide/messages';

	const version = useVersion();
	// The red ↑ chip opens the release-notes / update modal instead of
	// leaving for GitHub; the modal itself links to the release page.
	let updateOpen = $state(false);
	// Server-wide deployment label: "cctui (NAME)" in the brand + tab title.
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
</script>

<header class="hd">
	<div class="hd-inner container">
		<!-- The whole brand block is the way home: clicking "cctui (NAME)"
		     goes back to the session list from anywhere. -->
		<NavLink href="/sessions" title={m.nav_sessions()}>
			<div class="brand">
				<Text variant="code" tone="accent" size="lg" weight="bold">»_</Text>
				<Text size="lg" weight="bold">cctui</Text>
				{#if instanceName}
					<span class="inst">
						<Text size="lg" weight="bold" tone="faint" title={m.nav_instance_name()}>
							({instanceName})
						</Text>
					</span>
				{/if}
			</div>
		</NavLink>
		<span
			class="conn"
			class:on={ws.status === 'open'}
			class:mid={ws.status === 'connecting'}
			title={m.nav_ws_status({ status: ws.status })}
		></span>
		<div class="spacer"></div>
		<span class="net"><NetStatsChip /></span>
		{#if version.data}
			<span class="ver">
				<NavLink href={version.data.commit_url} target="_blank" rel="noopener">
					<Text size="xs" tone="faint" variant="code">
						<span class="ver-cluster">
							<span class="ver-part">srv v{version.data.version}</span>
							<span class="ver-part">ui v{__CLIENT_VERSION__}</span>
						</span>
					</Text>
				</NavLink>
			</span>
			{#if version.data.latest_version}
				<!-- Red up-arrow + the newer tag: only rendered when the server's
				     release probe found something strictly newer than itself.
				     Click → release notes + (admin) the update button.
				     NOT tsumikit's `chip`: that is a fixed 2.5rem square with
				     padding 0 meant for a lone glyph — the version text spilled
				     out of it over the neighbouring buttons, and its rem height
				     grew past this scale-immune (px-pinned) header whenever the
				     UI font slider went up. Pinned in px here for the same
				     reason the other toolbar controls are. -->
				<Button
					variant="ghost"
					size="sm"
					style="height: 28px; min-height: 28px; width: auto; min-width: 0; padding: 0 var(--sp-2); flex: none;"
					title={m.nav_update_available({ version: version.data.latest_version })}
					aria-label={m.nav_update_available({ version: version.data.latest_version })}
					onclick={() => (updateOpen = true)}
				>
					<Text size="xs" variant="code">
						<span class="ver-part ver-up">
							<span class="ver-up-arrow" aria-hidden="true">↑</span>
							v{version.data.latest_version}
						</span>
					</Text>
				</Button>
			{/if}
		{/if}
		<!-- Plain ghost button: the on-state accent tint comes from `pressed`
		     (aria-pressed → accent glyph), NOT a `tone` border — a tinted border
		     read blurry/strange against the header's backdrop blur. -->
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
		<!-- UI font size as 5 discrete levels: a native <select>
		     overlaid on an "A" glyph. Discrete steps avoid the live-reflow
		     "seizure" the continuous slider caused. -->
		<SelectButton
			glyph="A"
			label={m.nav_font_size()}
			title={m.nav_font_size()}
			value={fontScale.levelId}
			options={SCALE_LEVELS.map((l) => ({ value: l.id, label: l.label }))}
			onchange={(v) => settings.setFontScaleLevel(v)}
		/>
		<!-- Theme picker: pick any palette directly. Grouped into
		     light/dark sections so the long list stays scannable. -->
		<SelectButton
			glyph={theme.icon}
			label={m.nav_theme()}
			title={m.nav_theme_tooltip({ theme: theme.label })}
			value={theme.current}
			groups={[
				{
					label: m.nav_theme_system(),
					options: [{ value: AUTO.id, label: `${AUTO.icon}  ${m.nav_theme_auto()}` }]
				},
				{
					label: m.nav_theme_light(),
					options: THEMES.filter((t) => t.mode === 'light').map((t) => ({
						value: t.id,
						label: `${t.icon}  ${t.label}`
					}))
				},
				{
					label: m.nav_theme_dark(),
					options: THEMES.filter((t) => t.mode === 'dark').map((t) => ({
						value: t.id,
						label: `${t.icon}  ${t.label}`
					}))
				}
			]}
			onchange={(v) => settings.setTheme(v)}
		/>
	</div>
</header>

{#if updateOpen && version.data?.latest_version}
	<UpdateModal
		latestVersion={version.data.latest_version}
		latestUrl={version.data.latest_url ?? version.data.repo_url}
		selfUpdateReady={version.data.self_update_ready}
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

		/* Detach the toolbar from the global UI scale. The app scales
		   by changing the ROOT font-size, so every rem — including the header's
		   sizing tokens — tracks the slider this header hosts. Dragging it then
		   reflowed the whole header and slid the slider out from under the
		   cursor, oscillating the value. Redefining the size tokens here in px
		   (the exact equivalents of the :root rem values at the 1.0/16px base)
		   makes the header subtree scale-immune: content rescales live while the
		   toolbar — and the slider — stay put. Pixel-identical at scale 1. */
		--fs-xs: 12px;
		--fs-sm: 13px;
		--fs-base: 15px;
		--fs-lg: 18px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
		--sp-4: 16px;
		/* The header's inner is a `.container` (max-width: --content-max,
		   margin-inline:auto). --content-max is 56rem, so it ALSO tracked the
		   root font-size: scaling up widened the centered container and slid its
		   right-aligned contents (the slider) rightward under the cursor — the
		   real remaining cause of the drag "seizure" after the token pin above.
		   Pin it to px (56rem @16px = 896px) so the header's width is fully
		   scale-immune. */
		--content-max: 896px;
	}
	/* All three toolbar controls (bell IconButton + the two SelectButtons) are
	   icon-only `.btn-icon`s that size off hardcoded rem — pin them so they don't
	   grow with the global scale. Use a FIXED square (not min-*) so the differing
	   glyphs (bell SVG, "A", theme emoji) all share one footprint instead of each
	   sizing to its own content — the cause of the lopsided look on mobile. */
	.hd :global(.btn-icon) {
		flex: none;
		height: 36px;
		width: 36px;
		min-height: 36px;
		min-width: 36px;
		padding: 0;
	}
	/* SelectButton wraps its inner .btn-icon in this clip span; match the square
	   AND refuse to flex-shrink — otherwise the header's flex row squishes the
	   wrapper horizontally on narrow screens (the bell resists via its own
	   min-width; the wrapper needs it spelled out too). */
	.hd :global(.select-button) {
		flex: none;
		height: 36px;
		width: 36px;
		min-width: 36px;
	}
	/* The version chip never wraps: a two-line chip inside the 36px bar is
	   unreadable, so below 640px it (and the net-stats chip) is dropped
	   instead — the update arrow survives, it's the one that matters. */
	.ver {
		flex: none;
	}
	.ver-cluster {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.net {
		display: inline-flex;
	}
	@media (max-width: 639px) {
		.ver,
		.net {
			display: none;
		}
	}
	.ver-part {
		white-space: nowrap;
	}
	.ver-up {
		display: inline-flex;
		align-items: center;
		color: var(--danger);
		font-weight: 700;
	}
	.ver-up-arrow {
		display: inline-block;
		margin-right: 0.15em;
	}
	.inst {
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.hd-inner {
		height: var(--header-h);
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.brand {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.spacer {
		min-width: 0;
	}
	.conn {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--dot-dead);
	}
	.conn.on {
		background: var(--ok);
		box-shadow: 0 0 6px var(--ok);
	}
	.conn.mid {
		background: var(--warn);
	}
</style>
