<script lang="ts">
	import { ws } from '$lib/ws.svelte';
	import { useVersion, useSessions } from '$lib/queries';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { theme, THEMES } from '$lib/theme.svelte';
	import { fontScale, SCALE_LEVELS } from '$lib/fontscale.svelte';
	import { notify } from '$lib/notify.svelte';
	import { toasts } from '$lib/toast.svelte';
	import Button from '$lib/components/atoms/Button.svelte';
	import SelectButton from '$lib/components/molecules/SelectButton.svelte';
	import Text from '$lib/components/atoms/Text.svelte';
	import NavLink from '$lib/components/atoms/NavLink.svelte';

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
			<Text variant="code" tone="accent" size="lg" weight="bold">»_</Text>
			<Text size="lg" weight="bold">cctui</Text>
		</div>
		<span
			class="conn"
			class:on={ws.status === 'open'}
			class:mid={ws.status === 'connecting'}
			title={`websocket: ${ws.status}`}
		></span>
		<div class="spacer"></div>
		{#if $version.data}
			<NavLink class="ver mono" href={$version.data.commit_url} target="_blank" rel="noopener">
				srv v{$version.data.version} · ui v{__CLIENT_VERSION__}
			</NavLink>
		{/if}
		<Button
			variant="ghost"
			class={`btn-icon${notify.enabled ? ' notify-on' : ''}`}
			title={notify.enabled ? 'Notifications on — click to mute' : 'Notify me when a session needs input'}
			onclick={toggleNotify}
			oncontextmenu={(e: MouseEvent) => {
				e.preventDefault();
				notify.setSound(!notify.sound);
				toasts.push(notify.sound ? 'Sound on' : 'Sound off', 'info');
			}}
		>
			{notify.enabled ? '🔔' : '🔕'}
		</Button>
		<!-- UI font size as 5 discrete levels (CCT-297 #11): a native <select>
		     overlaid on an "A" glyph. Discrete steps avoid the live-reflow
		     "seizure" the continuous slider caused. -->
		<SelectButton
			glyph="A"
			label="UI font size"
			title="UI font size"
			value={fontScale.levelId}
			options={SCALE_LEVELS.map((l) => ({ value: l.id, label: l.label }))}
			onchange={(v) => fontScale.set(v)}
		/>
		<!-- Theme picker (CCT-250 item 5): pick any palette directly. -->
		<SelectButton
			glyph={theme.icon}
			label="Theme"
			title={`Theme: ${theme.label}`}
			value={theme.current}
			options={THEMES.map((t) => ({ value: t.id, label: `${t.icon} ${t.label}` }))}
			onchange={(v) => theme.set(v as typeof theme.current)}
		/>
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
		/* The header's inner is a `.container` (max-width: --content-max,
		   margin-inline:auto). --content-max is 56rem, so it ALSO tracked the
		   root font-size: scaling up widened the centered container and slid its
		   right-aligned contents (the slider) rightward under the cursor — the
		   real remaining cause of the drag "seizure" after the token pin above.
		   Pin it to px (56rem @16px = 896px) so the header's width is fully
		   scale-immune. */
		--content-max: 896px;
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
	/* ver is the class on the NavLink atom, so reach it via :global. */
	:global(.ver) {
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	/* Notify button tint when enabled — passed as a class to the Button child, so
	   it must be :global to reach it. The font-size + theme pickers are now the
	   SelectButton primitive (which owns the overlay-select styling). */
	:global(.notify-on) {
		color: var(--accent);
	}
</style>
