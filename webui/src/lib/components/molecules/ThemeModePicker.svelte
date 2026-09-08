<script lang="ts">
	// Header theme switcher: the trigger shows the painted theme's palette
	// swatch and opens a popover grid of swatches, light then dark, like the
	// kit's ThemePicker. Below the grids sits the "Auto (system)" row, which
	// recalls the two remembered palettes (last light and last dark picked) as
	// swatches: choosing it follows prefers-color-scheme with those two.
	// Each swatch is scoped with data-theme so its four quadrants resolve that
	// theme's raw palette (--c-bg, --c-surface, --c-text, --c-accent).
	import { Popover } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { settings } from '$lib/settings.svelte';
	import { theme } from '$lib/theme.svelte';
	import { themeMode } from '$lib/themeMode.svelte';
	import { AUTO_THEME } from '$lib/themeMode';
	import { themeLabel, themePickerTitle } from './themePicker.logic';

	let { class: klass = '' }: { class?: string } = $props();

	let hovered = $state<string | null>(null);
	let root: HTMLDivElement | undefined = $state();
	const isAuto = $derived(themeMode.pref.mode === 'auto');
	const groups = $derived(
		(['light', 'dark'] as const).map((mode) => ({
			mode,
			label: mode === 'light' ? m.theme_group_light() : m.theme_group_dark(),
			themes: theme.all.filter((t) => t.mode === mode)
		}))
	);
	const title = $derived(
		themePickerTitle(theme.all, themeMode.pref, themeMode.resolved, m.theme_auto_label())
	);
	const autoCaption = $derived(
		`${m.theme_auto_label()} · ${themeLabel(theme.all, themeMode.pref.light)} / ${themeLabel(theme.all, themeMode.pref.dark)}`
	);
	const caption = $derived(
		hovered === AUTO_THEME
			? autoCaption
			: hovered
				? themeLabel(theme.all, hovered)
				: isAuto
					? autoCaption
					: themeLabel(theme.all, themeMode.resolved)
	);

	function pick(id: string) {
		settings.setTheme(id);
		(root?.closest('[popover]') as HTMLElement | null)?.hidePopover?.();
	}
</script>

{#snippet swatch(id: string)}
	<span class="swatch" data-theme={id} aria-hidden="true">
		<i class="q bg"></i><i class="q surface"></i><i class="q text"></i><i class="q accent"></i>
	</span>
{/snippet}

<Popover label={title} placement="bottom-end" triggerClass={klass} box="md">
	{#snippet trigger()}<span class="trigger" data-tsu="ThemePicker" {title}
			>{@render swatch(themeMode.resolved)}{#if isAuto}<span class="auto-dot" aria-hidden="true">◐</span>{/if}</span
		>{/snippet}
	<div class="panel" bind:this={root}>
		{#each groups as g (g.mode)}
			<div class="group-label">{g.label}</div>
			<div class="grid" role="group" aria-label={g.label}>
				{#each g.themes as t (t.id)}
					<button
						type="button"
						class="cell"
						class:current={!isAuto && t.id === themeMode.resolved}
						class:remembered={isAuto && t.id === themeMode.pref[g.mode]}
						aria-pressed={!isAuto && t.id === themeMode.resolved}
						aria-label={t.label}
						title="{t.icon ?? theme.fallbackIcon} {t.label}"
						onclick={() => pick(t.id)}
						onpointerenter={() => (hovered = t.id)}
						onpointerleave={() => (hovered = null)}
						onfocus={() => (hovered = t.id)}
						onblur={() => (hovered = null)}
					>
						{@render swatch(t.id)}
					</button>
				{/each}
			</div>
		{/each}
		<button
			type="button"
			class="auto"
			class:current={isAuto}
			aria-pressed={isAuto}
			data-journey="theme-auto"
			title={autoCaption}
			onclick={() => pick(AUTO_THEME)}
			onpointerenter={() => (hovered = AUTO_THEME)}
			onpointerleave={() => (hovered = null)}
			onfocus={() => (hovered = AUTO_THEME)}
			onblur={() => (hovered = null)}
		>
			<span class="auto-glyph" aria-hidden="true">◐</span>
			<span class="auto-text">
				<span class="auto-name">{m.theme_auto_label()}</span>
				<span class="auto-help">{m.theme_auto_help()}</span>
			</span>
			<span class="auto-pair" aria-hidden="true">
				{@render swatch(themeMode.pref.light)}
				<span class="slash">/</span>
				{@render swatch(themeMode.pref.dark)}
			</span>
		</button>
		<div class="caption" aria-live="polite">{caption}</div>
	</div>
</Popover>

<style>
	.trigger {
		position: relative;
		display: inline-flex;
	}
	.auto-dot {
		position: absolute;
		right: -0.35rem;
		bottom: -0.35rem;
		font-size: 0.6rem;
		line-height: 1;
		color: var(--text-muted);
		background: var(--bg);
		border-radius: 50%;
	}
	.swatch {
		display: grid;
		grid-template-columns: 1fr 1fr;
		width: 1.25rem;
		height: 1.25rem;
		overflow: hidden;
		border-radius: var(--r-sm);
		box-shadow: inset 0 0 0 1px var(--border);
	}
	.swatch[data-theme='dark'] {
		--c-bg: #0f1115;
		--c-surface: #21262f;
		--c-text: #e6e9ef;
		--c-accent: #5ad6a0;
	}
	.q {
		display: block;
	}
	.q.bg {
		background: var(--c-bg);
	}
	.q.surface {
		background: var(--c-surface);
	}
	.q.text {
		background: var(--c-text);
	}
	.q.accent {
		background: var(--c-accent);
	}
	.panel {
		padding: var(--sp-2);
		width: max-content;
		max-width: min(22rem, calc(100vw - 2rem));
	}
	.group-label {
		margin: var(--sp-1) var(--sp-1) var(--sp-1);
		font-size: var(--fs-xs);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-faint);
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(6, auto);
		gap: var(--sp-1);
	}
	.cell {
		display: inline-flex;
		padding: 3px;
		border: 2px solid transparent;
		border-radius: var(--r-md);
		background: none;
		cursor: pointer;
	}
	.cell .swatch {
		width: 1.6rem;
		height: 1.6rem;
	}
	.cell:hover {
		background: var(--bg-elevated-2);
	}
	.cell.current {
		border-color: var(--accent);
	}
	.cell.remembered {
		border-color: var(--accent);
		border-style: dashed;
	}
	.cell:focus-visible,
	.auto:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 1px;
	}
	.auto {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
		margin-top: var(--sp-2);
		padding: var(--sp-1) var(--sp-2);
		border: 2px solid transparent;
		border-top: 1px solid var(--border);
		border-radius: var(--r-md);
		background: none;
		color: inherit;
		text-align: left;
		cursor: pointer;
	}
	.auto:hover {
		background: var(--bg-elevated-2);
	}
	.auto.current {
		border-color: var(--accent);
	}
	.auto-glyph {
		font-size: 1.1rem;
		line-height: 1;
	}
	.auto-text {
		display: flex;
		flex-direction: column;
		flex: 1 1 auto;
		min-width: 0;
	}
	.auto-name {
		font-size: var(--fs-sm);
	}
	.auto-help {
		font-size: var(--fs-xs);
		color: var(--text-faint);
		white-space: normal;
	}
	.auto-pair {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.slash {
		color: var(--text-faint);
		font-size: var(--fs-xs);
	}
	.caption {
		margin-top: var(--sp-2);
		text-align: center;
		font-size: var(--fs-sm);
		color: var(--text-muted);
		white-space: normal;
	}
</style>
