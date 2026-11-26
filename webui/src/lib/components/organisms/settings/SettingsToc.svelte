<script lang="ts">
	// Navigation of the Settings screen: a search box that reaches every page,
	// one link per page, and the current page's sub-areas as anchors. On narrow
	// screens the list gives way to a row of tabs (the search box moves to the
	// page head there).
	import { Icon, Input, Tabs, Text, type IconName, type TabItem } from '@dorsk/tsumikit';
	import { goto } from '$app/navigation';
	import NavLink from '#lib/components/atoms/NavLink.svelte';
	import {
		SETTINGS_SCOPES,
		settingsHref,
		settingsScope,
		type SettingsPage
	} from './settings.logic';
	import { m } from '#lib/paraglide/messages.js';

	export interface TocEntry {
		page: SettingsPage;
		icon: IconName;
		label: string;
		sections?: { id: string; label: string }[];
	}

	let {
		entries,
		active,
		query = $bindable('')
	}: {
		entries: TocEntry[];
		active: SettingsPage;
		query?: string;
	} = $props();

	const groups = $derived(
		SETTINGS_SCOPES.map((scope) => ({
			scope,
			entries: entries.filter((e) => settingsScope(e.page) === scope)
		})).filter((g) => g.entries.length > 0)
	);

	// Narrow screens: the same pages as kit tabs; picking one routes.
	const tabs = $derived<TabItem[]>(
		entries.map((e) => ({
			id: e.page,
			label: e.label,
			attrs: { 'data-journey': 'settings-tab', 'data-journey-key': e.page }
		}))
	);
	let tab = $derived(active as string);
	$effect(() => {
		if (tab !== active) void goto(settingsHref(tab as SettingsPage), { reset: false });
	});
</script>

<!-- `settings-goto` is on whichever of these two is visible at this width, so a
     guide can ask for "the page switcher" once instead of per viewport. -->
<div class="tabs" data-journey="settings-goto">
	<Tabs {tabs} bind:value={tab} label={m.settings_title()}>
		{#snippet panel()}{/snippet}
	</Tabs>
</div>

<nav class="toc" aria-label={m.settings_title()} data-journey="settings-goto">
	<div class="search">
		<Input
			icon="search"
			type="search"
			aria-label={m.settings_filter_placeholder()}
			bind:value={query}
			placeholder={m.settings_filter_placeholder()}
		/>
	</div>
	{#each groups as g (g.scope)}
		{#if g.scope === 'instance'}
			<div class="scope">
				<Text size="xs" tone="faint" weight="semibold">{m.settings_scope_admin_only()}</Text>
			</div>
		{/if}
		{#each g.entries as e (e.page)}
			<NavLink
				href={settingsHref(e.page)}
				class="toc-link"
				aria-current={active === e.page ? 'page' : undefined}
				data-journey="settings-nav"
				data-journey-key={e.page}
			>
				<span class="toc-item" class:active={active === e.page}>
					<span class="ico" class:on={active === e.page}><Icon name={e.icon} size={16} /></span>
					<Text size="sm" tone={active === e.page ? 'default' : 'muted'}>{e.label}</Text>
				</span>
			</NavLink>
			{#if active === e.page && e.sections && e.sections.length > 1}
				{#each e.sections as sec (sec.id)}
					<NavLink
						href={settingsHref(e.page, sec.id)}
						class="toc-link"
						data-journey="settings-section"
						data-journey-key={sec.id}
					>
						<span class="toc-sub"><Text size="xs" tone="muted">{sec.label}</Text></span>
					</NavLink>
				{/each}
			{/if}
		{/each}
	{/each}
</nav>

<style>
	.toc {
		position: sticky;
		top: calc(var(--header-h) + var(--sp-4));
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.search {
		position: relative;
		margin-bottom: var(--sp-3);
	}
	.scope {
		padding: var(--sp-3) var(--sp-3) var(--sp-1);
		text-transform: uppercase;
		letter-spacing: 0.06em;
	}
	.toc-item {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-radius: var(--r-sm);
		border-left: 2px solid transparent;
	}
	.toc-item:hover {
		background: var(--bg-elevated);
	}
	.toc-item.active {
		background: var(--bg-elevated);
		border-left-color: var(--accent);
	}
	.ico {
		display: inline-flex;
		width: 1.25rem;
		justify-content: center;
		flex: none;
		color: var(--text-faint);
	}
	.ico.on {
		color: var(--accent);
	}
	.toc-sub {
		display: flex;
		align-items: center;
		min-height: 1.5rem;
		padding: var(--sp-1) var(--sp-3) var(--sp-1) calc(var(--sp-3) + 1.25rem + var(--sp-2));
		border-radius: var(--r-sm);
	}
	@media (pointer: coarse) {
		.toc-sub {
			min-height: 2.75rem;
		}
	}
	.toc-sub:hover {
		background: var(--bg-elevated);
	}
	.tabs {
		display: none;
	}
	@media (max-width: 47.999rem) {
		.tabs {
			display: block;
			overflow-x: auto;
		}
		.toc {
			display: none;
		}
		.search {
			display: none;
		}
		.toc-item {
			white-space: nowrap;
			border: 1px solid var(--border);
			border-radius: var(--r-pill);
			padding: var(--sp-1) var(--sp-3);
		}
		.toc-item.active {
			border-color: var(--accent-dim, var(--accent));
		}
	}
</style>
