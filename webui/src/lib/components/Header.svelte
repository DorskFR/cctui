<script lang="ts">
	import { ws } from '$lib/ws.svelte';
	import { useVersion, useSessions } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { theme, THEMES } from '$lib/theme.svelte';
	import { fontScale, SCALE_MIN, SCALE_MAX } from '$lib/fontscale.svelte';
	import { notify } from '$lib/notify.svelte';
	import { toasts } from '$lib/toast.svelte';

	const version = useVersion();

	// Global "needs input" watcher (CCT-170). Header is always mounted inside
	// the query provider, so it's the natural home for the cross-route watcher.
	const qc = useQueryClient();
	const sessions = useSessions(() => false);

	// Live ws changes → refetch the list even on routes other than /sessions.
	$effect(() => {
		void ws.changeTick;
		qc.invalidateQueries({ queryKey: ['sessions'] });
	});

	// Drive notifications + title badge off the list's attention flags.
	$effect(() => {
		const items = $sessions.data?.sessions ?? [];
		notify.reconcile(items.filter((s) => s.attention === 'needs_input'));
	});

	async function toggleNotify() {
		if (notify.enabled) {
			notify.disable();
			toasts.push('Notifications off', 'info');
			return;
		}
		if (!notify.supported) {
			toasts.err('Notifications not supported in this browser');
			return;
		}
		const ok = await notify.enable();
		if (ok) toasts.ok('Notifications on');
		else toasts.err('Notifications blocked by browser');
	}
</script>

<header class="hd">
	<div class="hd-inner container">
		<div class="brand">
			<span class="logo">»_</span>
			<span class="name">cctui</span>
		</div>
		<span
			class="conn"
			class:on={ws.status === 'open'}
			class:mid={ws.status === 'connecting'}
			title={`websocket: ${ws.status}`}
		></span>
		<div class="spacer"></div>
		{#if $version.data}
			<a class="ver mono" href={$version.data.commit_url} target="_blank" rel="noopener">
				srv v{$version.data.version} · ui v{__CLIENT_VERSION__}
			</a>
		{/if}
		<button
			class="btn btn-ghost btn-icon"
			class:notify-on={notify.enabled}
			title={notify.enabled ? 'Notifications on — click to mute' : 'Notify me when a session needs input'}
			onclick={toggleNotify}
			oncontextmenu={(e) => {
				e.preventDefault();
				notify.setSound(!notify.sound);
				toasts.push(notify.sound ? 'Sound on' : 'Sound off', 'info');
			}}
		>
			{notify.enabled ? '🔔' : '🔕'}
		</button>
		<label class="font-slider" title="UI font size">
			<span aria-hidden="true">A</span>
			<input
				type="range"
				min={SCALE_MIN}
				max={SCALE_MAX}
				step="0.05"
				value={fontScale.current}
				oninput={(e) => fontScale.set(Number((e.currentTarget as HTMLInputElement).value))}
				aria-label="UI font size"
			/>
		</label>
		<!-- Theme picker (CCT-250 item 5): pick any palette directly, not just
		     cycle. Falls back gracefully — the button still shows the active icon. -->
		<div class="theme-pick btn btn-ghost btn-icon" title={`Theme: ${theme.label}`}>
			<span aria-hidden="true">{theme.icon}</span>
			<select
				aria-label="Theme"
				value={theme.current}
				onchange={(e) => theme.set((e.currentTarget as HTMLSelectElement).value as typeof theme.current)}
			>
				{#each THEMES as t (t.id)}
					<option value={t.id}>{t.icon} {t.label}</option>
				{/each}
			</select>
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

		/* Detach the toolbar from the global UI scale (CCT-264). The app scales
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
	}
	/* The two header icon buttons size off hardcoded rem (min-height/min-width),
	   not the tokens above — pin them too so they don't grow with the scale. */
	.hd :global(.btn) {
		min-height: 40px;
	}
	.hd :global(.btn-icon) {
		min-height: 36px;
		min-width: 36px;
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
		font-weight: var(--fw-bold);
		font-size: var(--fs-lg);
	}
	.logo {
		font-family: var(--font-mono);
		color: var(--accent);
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
	.ver {
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.notify-on {
		color: var(--accent);
	}
	/* Top-bar font-size slider (CCT-250 item 3) — drives the global UI scale.
	   px geometry (not rem) like the rest of the header, so the track neither
	   resizes nor reflows under the cursor while dragging (see `.hd`). */
	.font-slider {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		color: var(--text-faint);
		font-size: 12px;
	}
	.font-slider input[type='range'] {
		width: 64px;
		accent-color: var(--accent);
	}
	/* Theme picker: a native <select> overlaid transparently on the icon button
	   so it gets the platform dropdown UI while keeping the icon affordance. */
	.theme-pick {
		position: relative;
		overflow: hidden;
	}
	.theme-pick select {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		opacity: 0;
		cursor: pointer;
		border: none;
		background: none;
	}
</style>
