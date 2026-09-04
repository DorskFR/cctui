<script lang="ts">
	// User settings. Server-persisted via the `settings` singleton (GET/PUT
	// /api/v1/settings, localStorage-mirrored). Seven anchored sections —
	// Appearance · Sessions · Execution · Privacy · Notifications · Security ·
	// Instance (admin) — behind a sticky table of contents with a free-text
	// filter and a save indicator. Each section is its own organism; this page
	// only assembles them and runs the filter / scroll spy.
	import { onMount, tick } from 'svelte';
	import { Heading, Icon, Input, Text } from '@dorsk/tsumikit';
	import SettingsToc, { type TocEntry } from '$lib/components/organisms/settings/SettingsToc.svelte';
	import AppearanceSection from '$lib/components/organisms/settings/AppearanceSection.svelte';
	import SessionsSection from '$lib/components/organisms/settings/SessionsSection.svelte';
	import ExecutionSection from '$lib/components/organisms/settings/ExecutionSection.svelte';
	import PrivacySection from '$lib/components/organisms/settings/PrivacySection.svelte';
	import NotificationsSection from '$lib/components/organisms/settings/NotificationsSection.svelte';
	import SecuritySection from '$lib/components/organisms/settings/SecuritySection.svelte';
	import InstanceSection from '$lib/components/organisms/settings/InstanceSection.svelte';
	import { applySettingsFilter, isFiltered } from '$lib/components/organisms/settings/settings.logic';
	import { settings } from '$lib/settings.svelte';
	import { useMe } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');

	const entries = $derived<TocEntry[]>([
		{ id: 'appearance', icon: '◐', label: m.settings_nav_appearance() },
		{ id: 'sessions', icon: '◰', label: m.settings_nav_sessions() },
		{ id: 'execution', icon: '▶', label: m.settings_nav_execution() },
		{ id: 'privacy', icon: '◈', label: m.settings_nav_privacy() },
		{ id: 'notifications', icon: '🔔', label: m.settings_notifications_title() },
		{ id: 'security', icon: '⚿', label: m.settings_nav_security() },
		...(isAdmin
			? [{ id: 'instance', icon: '⚙', label: m.settings_nav_instance(), admin: true }]
			: [])
	]);

	// ── Filter ──────────────────────────────────────────────────────────
	// DOM-driven: rows carry their localized copy, so the filter reads it back
	// instead of keeping a parallel catalogue. Re-applied whenever the query or
	// the admin flag (which adds/removes a section) changes.
	let query = $state('');
	let content = $state<HTMLElement | null>(null);
	let visibleRows = $state(-1);
	$effect(() => {
		const q = query;
		void isAdmin;
		if (!content) return;
		const root = content;
		tick().then(() => {
			visibleRows = applySettingsFilter(root, q);
		});
	});

	// ── Scroll spy ──────────────────────────────────────────────────────
	// The active TOC entry is the last section whose top passed the sticky
	// header; a click scrolls smoothly and pins the choice until the scroll
	// settles so the highlight does not flicker through the sections between.
	let active = $state('appearance');
	let pinned: string | null = null;
	let pinTimer: ReturnType<typeof setTimeout> | null = null;

	function spy() {
		if (!content) return;
		const headerH = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--header-h')) || 52;
		const line = headerH + 96;
		let current: string | null = null;
		for (const sec of content.querySelectorAll<HTMLElement>('[data-setting-section]')) {
			if (isFiltered(sec)) continue;
			if (current === null || sec.getBoundingClientRect().top <= line) current = sec.id;
		}
		if (current === null) return;
		if (pinned) {
			if (current === pinned) pinned = null;
			else return;
		}
		active = current;
	}

	function pick(id: string) {
		const el = document.getElementById(id);
		if (!el) return;
		active = id;
		pinned = id;
		if (pinTimer) clearTimeout(pinTimer);
		pinTimer = setTimeout(() => (pinned = null), 1200);
		el.scrollIntoView({ behavior: 'smooth', block: 'start' });
		history.replaceState(null, '', `#${id}`);
	}

	onMount(() => {
		window.addEventListener('scroll', spy, { passive: true });
		spy();
		const hash = location.hash.slice(1);
		if (hash) tick().then(() => document.getElementById(hash)?.scrollIntoView({ block: 'start' }));
		return () => window.removeEventListener('scroll', spy);
	});

	// ── Save indicator ──────────────────────────────────────────────────
	// "Saved just now" for a short while after the server acknowledged the
	// PUT, then plain "Saved"; "Saving…" while a write is debounced/in flight.
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
	<header class="head">
		<div class="head-text">
			<Heading level={1}>{m.settings_title()}</Heading>
			<Text tone="faint" as="p">{m.settings_subtitle()}</Text>
		</div>
		<div class="head-side">
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
			<div class="mobile-search">
				<Input
					type="search"
					bind:value={query}
					size="sm"
					placeholder={m.settings_filter_placeholder()}
					aria-label={m.settings_filter_placeholder()}
					style="width:100%"
				/>
			</div>
		</div>
	</header>

	<div class="layout">
		<SettingsToc {entries} {active} bind:query onpick={pick} />

		<div class="content" bind:this={content}>
			<AppearanceSection />
			<SessionsSection />
			<ExecutionSection />
			<PrivacySection />
			<NotificationsSection />
			<SecuritySection {isAdmin} />
			{#if isAdmin}
				<InstanceSection />
			{/if}
			{#if visibleRows === 0}
				<div class="empty">
					<Text tone="faint">{m.settings_filter_empty({ query })}</Text>
				</div>
			{/if}
		</div>
	</div>
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-5);
	}
	.head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--sp-4);
	}
	.head-text {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.head-side {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: var(--sp-2);
	}
	.saved {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		color: var(--text-faint);
		white-space: nowrap;
	}
	.saved.ok {
		color: var(--accent);
	}
	.mobile-search {
		display: none;
	}
	.layout {
		display: grid;
		grid-template-columns: 13rem minmax(0, 1fr);
		gap: var(--sp-8);
		align-items: start;
	}
	.content {
		display: flex;
		flex-direction: column;
		gap: var(--sp-10);
		min-width: 0;
	}
	.empty {
		padding: var(--sp-6) 0;
		text-align: center;
	}
	@media (max-width: 47.999rem) {
		.head {
			flex-direction: column;
			align-items: stretch;
		}
		.head-side {
			align-items: stretch;
		}
		.saved {
			justify-content: flex-end;
		}
		.mobile-search {
			display: block;
		}
		.layout {
			grid-template-columns: minmax(0, 1fr);
			gap: var(--sp-3);
		}
		.content {
			gap: var(--sp-8);
		}
	}
</style>
