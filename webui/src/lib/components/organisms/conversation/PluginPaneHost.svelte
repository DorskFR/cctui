<script lang="ts">
	// Mounts an enabled plugin's pane against the left edge of the conversation
	// drawer: a fixed column with a resize grip (the stats-dock pattern) on wide
	// screens, the whole drawer below the mobile breakpoint.
	import { browser } from '$app/environment';
	import { setContext } from 'svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import { resizeHandle, Text } from '@dorsk/tsumikit';
	import { clampDockWidth, DOCK_MIN_PX, maxDockWidth, storedDockWidth } from '$lib/dock';
	import { composerFor } from '$lib/plugins/composerBridge.svelte';
	import {
		CCTUI_PLUGIN_API,
		HOST_CONTEXT_KEY,
		type CctuiPluginModule,
		type HostContext,
		type PluginInfo,
		type PluginSession
	} from '$lib/plugins/types';
	import { m } from '$lib/paraglide/messages';

	let {
		plugin,
		module,
		session,
		params = {},
		onclose
	}: {
		plugin: PluginInfo;
		/** Validated bundle; its `sessionPane` is what gets mounted. */
		module: CctuiPluginModule;
		session: PluginSession;
		/** From the message action that opened the pane; empty from the header. */
		params?: Record<string, string>;
		onclose: () => void;
	} = $props();

	const DEFAULT_PX = 480;
	const widthKey = $derived(`cctui_plugin_pane_width:${plugin.id}`);
	const composer = $derived(composerFor(session.id));
	const Pane = $derived(module.sessionPane);
	setContext<HostContext>(HOST_CONTEXT_KEY, { cctuiApi: CCTUI_PLUGIN_API, origin: browser ? location.origin : '' });

	let width = $derived((browser && storedDockWidth(localStorage.getItem(widthKey))) || DEFAULT_PX);
	function setWidth(px: number | undefined) {
		width = clampDockWidth(px) ?? DEFAULT_PX;
		if (px === undefined) localStorage.removeItem(widthKey);
		else localStorage.setItem(widthKey, String(width));
	}
	let dragging = $state(false);
	let viewportWidth = $state(0);
	const maxPx = $derived(maxDockWidth(viewportWidth));

	// Same breakpoint as the drawer's fullWidthBelow: under it the pane takes
	// the whole drawer, so there is nothing to resize.
	const narrowQuery = new MediaQuery('(max-width: 959px)');
	const narrow = $derived(narrowQuery.current);
</script>

<svelte:window bind:innerWidth={viewportWidth} />

<aside
	class="host"
	class:narrow
	style:--plugin-pane-w="{width}px"
	aria-label={plugin.name}
	data-journey="plugin-pane"
	data-plugin={plugin.id}
>
	{#if Pane}
		<Pane {session} {composer} {params} {onclose} />
	{:else}
		<div class="pad"><Text size="sm" tone="danger">{m.plugins_pane_failed()}</Text></div>
	{/if}
	{#if !narrow}
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			class="grip"
			class:dragging
			role="separator"
			tabindex="0"
			aria-orientation="vertical"
			aria-valuemin={DOCK_MIN_PX}
			aria-valuemax={maxPx}
			aria-valuenow={width}
			aria-label={m.dock_resize_grip()}
			title={m.dock_resize_grip()}
			use:resizeHandle={{
				side: 'left',
				min: DOCK_MIN_PX,
				max: maxPx,
				onwidth: setWidth,
				onreset: () => setWidth(undefined),
				onactive: (a) => (dragging = a)
			}}
		></div>
	{/if}
</aside>

<style>
	/* Flush against the drawer's outer edge: the drawer is a fixed panel whose
	   width the kit publishes as --panel-current-width. */
	.host {
		position: fixed;
		top: 0;
		bottom: 0;
		right: min(var(--panel-current-width), 100vw);
		width: min(var(--plugin-pane-w), calc(100vw - var(--panel-current-width)));
		background: var(--bg);
		box-shadow: var(--shadow-lg);
	}
	.host.narrow {
		position: relative;
		right: auto;
		width: 100%;
		height: 100%;
		box-shadow: none;
	}
	.pad {
		padding: var(--sp-3);
	}
	.grip {
		position: absolute;
		top: 0;
		bottom: 0;
		right: -5px;
		width: 10px;
		cursor: ew-resize;
		touch-action: none;
		z-index: 1;
	}
	.grip::before {
		content: '';
		position: absolute;
		top: 50%;
		left: 3px;
		width: 4px;
		height: 2.5rem;
		margin-top: -1.25rem;
		border-radius: var(--r-pill);
		background: var(--border-strong);
	}
	.grip:hover::before,
	.grip.dragging::before {
		background: var(--accent);
	}
	.grip::after {
		content: '';
		position: absolute;
		top: 0;
		bottom: 0;
		left: 4px;
		width: 2px;
		background: var(--accent);
		opacity: 0;
		transition: opacity 0.12s var(--ease);
	}
	.grip:hover::after,
	.grip:focus-visible::after,
	.grip.dragging::after {
		opacity: 1;
	}
</style>
