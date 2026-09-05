<script lang="ts">
	// Navigation of the Settings screen: a search box that reaches every page and
	// one link per page — each entry is a route, the current one marked with an
	// accent bar. On narrow screens the list gives way to a row of tabs (the
	// search box moves to the page head there).
	import { Badge, FilterInput, Tabs, Text, type Schema, type TabItem } from '@dorsk/tsumikit';
	import { goto } from '$app/navigation';
	import NavLink from '$lib/components/atoms/NavLink.svelte';
	import { settingsHref, type SettingsPage } from './settings.logic';
	import { m } from '$lib/paraglide/messages';

	export interface TocEntry {
		page: SettingsPage;
		icon: string;
		label: string;
		admin?: boolean;
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
	const PLAIN: Schema = { fields: [] };

	// Narrow screens: the same pages as kit tabs; picking one routes.
	const tabs = $derived<TabItem[]>(entries.map((e) => ({ id: e.page, label: e.label })));
	let tab = $derived(active as string);
	$effect(() => {
		if (tab !== active) void goto(settingsHref(tab as SettingsPage), { noScroll: true });
	});
</script>

<div class="tabs">
	<Tabs {tabs} bind:value={tab} label={m.settings_title()}>
		{#snippet panel()}{/snippet}
	</Tabs>
</div>

<nav class="toc" aria-label={m.settings_title()}>
	<div class="search">
		<label for="settings-filter" class="sr-only">{m.settings_filter_placeholder()}</label>
		<FilterInput
			id="settings-filter"
			schema={PLAIN}
			bind:value={query}
			placeholder={m.settings_filter_placeholder()}
		/>
	</div>
	{#each entries as e (e.page)}
		<NavLink
			href={settingsHref(e.page)}
			class="toc-link"
			aria-current={active === e.page ? 'page' : undefined}
		>
			<span class="toc-item" class:active={active === e.page}>
				<span class="ico"><Text tone={active === e.page ? 'accent' : 'faint'}>{e.icon}</Text></span>
				<Text size="sm" tone={active === e.page ? 'default' : 'muted'}>{e.label}</Text>
				{#if e.admin}
					<span class="tag"><Badge tone="warn" size="sm" border>{m.settings_scope_admin()}</Badge></span>
				{/if}
			</span>
		</NavLink>
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
	.search-icon {
		position: absolute;
		left: var(--sp-3);
		top: 50%;
		transform: translateY(-50%);
		color: var(--text-faint);
		display: inline-flex;
		pointer-events: none;
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
		width: 1.25rem;
		text-align: center;
		flex: none;
	}
	.tag {
		margin-left: auto;
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
		.tag {
			display: none;
		}
	}
</style>
