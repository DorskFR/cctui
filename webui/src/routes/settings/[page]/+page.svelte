<script lang="ts">
	// User settings, one page per topic. The blob behind them is server-persisted
	// (GET/PUT /api/v1/settings, localStorage-mirrored); this shell only owns the
	// navigation: the page nav, the cross-page search, the save indicator and the
	// pager. Every page stays mounted (SettingsPages) so the search can filter
	// across all of them and jump to the one that still matches.
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { page as route } from '$app/state';
	import { Icon, Input, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import SettingsToc, { type TocEntry } from '$lib/components/organisms/settings/SettingsToc.svelte';
	import SettingsPages from '$lib/components/organisms/settings/SettingsPages.svelte';
	import {
		applySettingsFilter,
		DEFAULT_SETTINGS_PAGE,
		firstMatchingPage,
		firstMatchingRow,
		isSettingsPage,
		settingsHref,
		type SettingsPage
	} from '$lib/components/organisms/settings/settings.logic';
	import { settings } from '$lib/settings.svelte';
	import { useMe } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');

	const current = $derived<SettingsPage>(
		isSettingsPage(route.params.page) ? route.params.page : DEFAULT_SETTINGS_PAGE
	);
	$effect(() => {
		if (!isSettingsPage(route.params.page))
			void goto(settingsHref(DEFAULT_SETTINGS_PAGE), { replaceState: true });
	});

	const entries = $derived<TocEntry[]>([
		{ page: 'appearance', icon: '◐', label: m.settings_nav_appearance() },
		{ page: 'sessions', icon: '◰', label: m.settings_nav_sessions() },
		{ page: 'execution', icon: '▶', label: m.settings_nav_execution() },
		{ page: 'privacy', icon: '◈', label: m.settings_nav_privacy() },
		{ page: 'notifications', icon: '🔔', label: m.settings_notifications_title() },
		{ page: 'monitoring', icon: '▥', label: m.settings_nav_monitoring() },
		{ page: 'security', icon: '⚿', label: m.settings_nav_security() },
		{ page: 'instance', icon: '⚙', label: m.settings_nav_instance(), admin: true }
	]);

	// The filter reads the rendered rows back instead of keeping a parallel
	// catalogue of localized copy. Every page is in the DOM, so a query that
	// matches nothing here but something elsewhere navigates to that page and
	// brings its first match into view.
	let query = $state('');
	let content = $state<HTMLElement | null>(null);
	let visibleRows = $state(-1);
	$effect(() => {
		const q = query;
		const here = current;
		void isAdmin;
		if (!content) return;
		const root = content;
		tick().then(() => {
			visibleRows = applySettingsFilter(root, q);
			if (!q.trim()) return;
			const target = firstMatchingPage(root);
			if (target && target !== here) {
				void goto(settingsHref(target), { replaceState: true, noScroll: true });
				return;
			}
			firstMatchingRow(root, here)?.scrollIntoView({ block: 'nearest' });
		});
	});

	// "Saved just now" for a short while after the server acknowledged the PUT,
	// then plain "Saved"; "Saving…" while a write is debounced or in flight.
	let now = $state(Date.now());
	$effect(() => {
		const t = setInterval(() => (now = Date.now()), 5_000);
		return () => clearInterval(t);
	});
	const saveLabel = $derived.by(() => {
		switch (settings.saveStatus) {
			case 'pending':
				return m.settings_saving();
			case 'error':
				return m.settings_save_failed();
			case 'saved':
				return settings.savedAt && now - settings.savedAt < 15_000
					? m.settings_saved_now()
					: m.settings_saved();
			default:
				return '';
		}
	});
</script>

<div class="page">
	<div class="layout">
		<aside class="side">
			<PageHead title={m.settings_title()} />
			<div class="mobile-search">
				<Input
					icon="search"
					type="search"
					aria-label={m.settings_filter_placeholder()}
					bind:value={query}
					placeholder={m.settings_filter_placeholder()}
				/>
			</div>
			<SettingsToc {entries} active={current} bind:query />
		</aside>

		<main class="main">
			{#if saveLabel}
				<span class="saved" class:ok={settings.saveStatus === 'saved'} aria-live="polite">
					{#if settings.saveStatus === 'saved'}
						<Icon name="check" size={14} />
					{:else if settings.saveStatus === 'pending'}
						<Icon name="loader" size={14} spin />
					{:else}
						<Icon name="alert-circle" size={14} />
					{/if}
					<Text size="xs" tone={settings.saveStatus === 'error' ? 'danger' : 'faint'}>{saveLabel}</Text>
				</span>
			{/if}

			<div class="content" bind:this={content}>
				<SettingsPages {current} {isAdmin} />
			</div>

			{#if visibleRows === 0}
				<div class="empty">
					<Text tone="faint">{m.settings_filter_empty({ query })}</Text>
				</div>
			{/if}

		</main>
	</div>
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.mobile-search {
		display: none;
	}
	.layout {
		display: grid;
		grid-template-columns: 13.75rem minmax(0, 1fr);
		gap: var(--sp-8);
		align-items: start;
	}
	.side {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.main {
		position: relative;
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
		max-width: 47.5rem;
		min-width: 0;
	}
	.saved {
		position: absolute;
		top: 0;
		right: 0;
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		color: var(--text-faint);
		white-space: nowrap;
	}
	.saved.ok {
		color: var(--accent);
	}
	.content {
		min-width: 0;
	}
	.empty {
		padding: var(--sp-6) 0;
		text-align: center;
	}
	@media (max-width: 47.999rem) {
		.mobile-search {
			display: block;
		}
		.layout {
			grid-template-columns: minmax(0, 1fr);
			gap: var(--sp-3);
		}
		.side {
			gap: var(--sp-2);
		}
		.saved {
			position: static;
			align-self: flex-end;
		}
	}
</style>
