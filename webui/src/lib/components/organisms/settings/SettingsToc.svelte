<script lang="ts">
	// Navigation of the Settings screen: a search box that reaches every page and
	// one link per page — each entry is a route, the current one marked with an
	// accent bar. On narrow screens it turns into a horizontally scrolling strip
	// of pills above the content (the search box moves to the page head there).
	import { Badge, Icon, Input, Text } from '@dorsk/tsumikit';
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
</script>

<nav class="toc" aria-label={m.settings_title()}>
	<div class="search">
		<span class="search-icon"><Icon name="search" size={14} /></span>
		<Input
			type="search"
			bind:value={query}
			size="sm"
			placeholder={m.settings_filter_placeholder()}
			aria-label={m.settings_filter_placeholder()}
			style="width:100%; padding-left: var(--sp-8)"
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
	@media (max-width: 47.999rem) {
		.toc {
			top: var(--header-h);
			z-index: 1;
			flex-direction: row;
			overflow-x: auto;
			gap: var(--sp-2);
			background: var(--bg);
			padding: var(--sp-2) var(--sp-4);
			margin: 0 calc(-1 * var(--sp-4));
			border-bottom: 1px solid var(--border);
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
