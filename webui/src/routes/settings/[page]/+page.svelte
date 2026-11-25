<script lang="ts">
	// The shell only owns the navigation: the page nav, the cross-page search
	// and the save indicator. Every page stays mounted (SettingsPages) so the
	// search can filter across all of them and jump to the one that matches.
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { page as route } from '$app/state';
	import { Icon, Input, Text } from '@dorsk/tsumikit';
	import PageHead from '#lib/components/molecules/PageHead.svelte';
	import SettingsToc, { type TocEntry } from '#lib/components/organisms/settings/SettingsToc.svelte';
	import SettingsPages from '#lib/components/organisms/settings/SettingsPages.svelte';
	import EventsPanel from '#lib/components/organisms/events/EventsPanel.svelte';
	import {
		applySettingsFilter,
		availableSettingsPages,
		DEFAULT_SETTINGS_PAGE,
		EVENTS_PAGE,
		firstMatchingPage,
		firstMatchingRow,
		isSettingsPage,
		NON_ADMIN_INSTANCE_FALLBACK,
		SETTINGS_SECTIONS,
		settingsHref,
		type SettingsPage
	} from '#lib/components/organisms/settings/settings.logic.ts';
	import { sectionTitle } from '#lib/components/organisms/settings/sectionTitles.ts';
	import { latestSave, serverSave } from '#lib/components/organisms/settings/serverSave.svelte.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { useMe } from '#lib/queries/index.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');

	const slug = $derived(route.params.page ?? '');
	const isEvents = $derived(slug === EVENTS_PAGE);
	const current = $derived<SettingsPage>(isSettingsPage(slug) ? slug : DEFAULT_SETTINGS_PAGE);
	$effect(() => {
		if (current === 'instance' && me.data && !isAdmin)
			void goto(NON_ADMIN_INSTANCE_FALLBACK, { replace: true });
	});

	const PAGE_NAV: Record<SettingsPage, { icon: TocEntry['icon']; label: () => string }> = {
		interface: { icon: 'palette', label: () => m.settings_nav_interface() },
		accounts: { icon: 'key-round', label: () => m.settings_nav_accounts() },
		sessions: { icon: 'list', label: () => m.settings_nav_sessions() },
		notifications: { icon: 'bell', label: () => m.settings_nav_notifications_voice() },
		plugins: { icon: 'grid', label: () => m.settings_nav_plugins() },
		instance: { icon: 'settings', label: () => m.settings_nav_instance() }
	};
	const entries = $derived<TocEntry[]>(
		availableSettingsPages(hasFeature, isAdmin).map((page) => ({
			page,
			icon: PAGE_NAV[page].icon,
			label: PAGE_NAV[page].label(),
			sections: SETTINGS_SECTIONS[page].map((id) => ({ id, label: sectionTitle(id) }))
		}))
	);

	// The filter reads the rendered rows back instead of keeping a parallel
	// catalogue of localized copy. It runs once per typing pause; the
	// navigation keeps focus in the search box (SvelteKit resets it to the
	// body otherwise, and the next keystrokes would land nowhere).
	const FILTER_DEBOUNCE_MS = 120;
	let query = $state('');
	let content = $state<HTMLElement | null>(null);
	let visibleRows = $state(-1);
	$effect(() => {
		const q = query;
		const here = current;
		void isAdmin;
		if (!content) return;
		const root = content;
		const timer = setTimeout(() => {
			void tick().then(() => {
				visibleRows = applySettingsFilter(root, q);
				if (!q.trim()) return;
				const target = firstMatchingPage(root);
				if (target && target !== here) {
					void goto(settingsHref(target), { replace: true, reset: false });
					return;
				}
				firstMatchingRow(root, here)?.scrollIntoView({ block: 'nearest' });
			});
		}, FILTER_DEBOUNCE_MS);
		return () => clearTimeout(timer);
	});

	// "Saved just now" for a short while after the server acknowledged a write,
	// then plain "Saved"; "Saving…" while a write is debounced or in flight.
	// Covers the per-user blob and the auto-saved instance settings alike.
	let now = $state(Date.now());
	$effect(() => {
		const t = setInterval(() => (now = Date.now()), 5_000);
		return () => clearInterval(t);
	});
	const save = $derived(
		latestSave(
			{ status: settings.saveStatus, savedAt: settings.savedAt, error: settings.saveError },
			serverSave
		)
	);
	const saveLabel = $derived.by(() => {
		switch (save.status) {
			case 'pending':
				return m.settings_saving();
			case 'error':
				return save.error ?? m.settings_save_failed();
			case 'saved':
				return save.savedAt && now - save.savedAt < 15_000
					? m.settings_saved_now()
					: m.settings_saved();
			default:
				return '';
		}
	});
</script>

{#if isEvents}
	<EventsPanel />
{:else}
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
				<span class="saved" class:ok={save.status === 'saved'} aria-live="polite">
					{#if save.status === 'saved'}
						<Icon name="check" size={14} />
					{:else if save.status === 'pending'}
						<Icon name="loader" size={14} spin />
					{:else}
						<Icon name="alert-circle" size={14} />
					{/if}
					<Text size="xs" tone={save.status === 'error' ? 'danger' : 'faint'}>{saveLabel}</Text>
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
{/if}

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
