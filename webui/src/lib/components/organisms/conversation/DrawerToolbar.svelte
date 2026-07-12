<script lang="ts">
	// Conversation toolbar (CCT-250 item 2 / CCT-311), extracted from
	// ConversationDrawer. Three visually-separated control groups — message-type
	// tag filter, formatting toggles, behavior (auto-approve) toggle — that on
	// mobile collapse behind three text-button tabs opening popovers.
	import { MSG_TYPES, type MsgType, type ViewOpts } from './types';
	import { Toggle } from '@dorsk/tsumikit';

	// The on-state tint for a message-type tag: its role color (result reuses
	// the tool color), or danger when excluded.
	const roleVar = (id: MsgType) => (id === 'result' ? 'var(--role-tool)' : `var(--role-${id})`);

	let {
		view = $bindable(),
		autoApprove,
		mobilePanel = $bindable(),
		ontoggleAuto,
		ondiagnose,
		onterminal,
		terminalOpen = false
	}: {
		view: ViewOpts;
		autoApprove: boolean;
		mobilePanel: 'filters' | 'format' | 'auto' | null;
		ontoggleAuto: () => void;
		/** Opens the session diagnose panel (CCT-547); omit to hide the button. */
		ondiagnose?: () => void;
		/** Toggles the read-only live terminal (CCT-545); omit to hide (codex). */
		onterminal?: () => void;
		terminalOpen?: boolean;
	} = $props();

	// Cycle a tag: off → include → exclude → off.
	// 'include' is EXCLUSIVE: selecting "only this" for a type clears any other
	// type's include so the active selection is unambiguous.
	// 'exclude' is additive — you can hide multiple types independently.
	function cycleTag(t: MsgType) {
		const order = ['off', 'include', 'exclude'] as const;
		const i = order.indexOf(view.typeFilter[t]);
		const next = order[(i + 1) % order.length];
		const updated = { ...view.typeFilter, [t]: next };
		if (next === 'include') {
			for (const m of MSG_TYPES) {
				if (m.id !== t && updated[m.id] === 'include') updated[m.id] = 'off';
			}
		}
		view.typeFilter = updated;
	}

	function togglePanel(p: 'filters' | 'format' | 'auto') {
		mobilePanel = mobilePanel === p ? null : p;
	}
</script>

<div class="toolbar" class:panel-active={mobilePanel !== null}>
	<!-- Mobile (CCT-311): collapse the three control groups into a single row
	     of text buttons that each open a popover. Hidden on desktop, where the
	     groups render inline below. -->
	<div class="mobile-tabs" role="group" aria-label="Chat controls">
		<Toggle
			class="mtab"
			pressed={mobilePanel === 'filters'}
			aria-expanded={mobilePanel === 'filters'}
			onclick={() => togglePanel('filters')}>Filters</Toggle
		>
		<Toggle
			class="mtab"
			pressed={mobilePanel === 'format'}
			aria-expanded={mobilePanel === 'format'}
			onclick={() => togglePanel('format')}>Format</Toggle
		>
		<Toggle
			class="mtab"
			pressed={mobilePanel === 'auto' || autoApprove}
			style={autoApprove ? '--toggle-accent: var(--warn)' : ''}
			aria-expanded={mobilePanel === 'auto'}
			onclick={() => togglePanel('auto')}>Auto-Approve</Toggle
		>
	</div>
	<!-- Message-type filters: click a tag to cycle off → include → exclude.
	     Active (include) tags wear their message-badge color; excluded tags
	     show a strike. (CCT-250 item 2) -->
	<div class="tagbar row row-wrap" class:panel-open={mobilePanel === 'filters'} role="group" aria-label="Message type filter">
		{#each MSG_TYPES as t (t.id)}
			<Toggle
				pill
				pressed={view.typeFilter[t.id] !== 'off'}
				struck={view.typeFilter[t.id] === 'exclude'}
				style={`--toggle-accent: ${view.typeFilter[t.id] === 'exclude' ? 'var(--danger)' : roleVar(t.id)}`}
				title={`${t.label}: ${view.typeFilter[t.id] === 'include' ? 'only this' : view.typeFilter[t.id] === 'exclude' ? 'hidden' : 'shown'} — click to cycle`}
				onclick={() => cycleTag(t.id)}
			>
				{#if view.typeFilter[t.id] === 'exclude'}✕ {/if}{t.label}
			</Toggle>
		{/each}
	</div>
	<!-- Formatting toggles: gray when off, colored when on. -->
	<div class="fmtbar row row-wrap" class:panel-open={mobilePanel === 'format'} role="group" aria-label="Formatting">
		<Toggle pressed={view.prettyJson} onclick={() => (view.prettyJson = !view.prettyJson)}>JSON</Toggle>
		<Toggle pressed={view.prettyDiff} onclick={() => (view.prettyDiff = !view.prettyDiff)}>Diff</Toggle>
		<Toggle pressed={view.prettyTables} onclick={() => (view.prettyTables = !view.prettyTables)} title="Render markdown tables as tables">Tables</Toggle>
	</div>
	<!-- Behavior toggle: distinct from filters/formatting. -->
	<div class="behbar row row-wrap" class:panel-open={mobilePanel === 'auto'} role="group" aria-label="Behavior">
		<Toggle
			pressed={autoApprove}
			style="--toggle-accent: var(--warn)"
			title="Auto-approve tool-use permission requests for this session — tool use only, plans still ask"
			aria-label="Auto-approve tool-use permissions — tool use only, plans still ask"
			onclick={ontoggleAuto}
		>⚡ Auto-approve</Toggle>
		{#if ondiagnose}
			<Toggle
				pressed={false}
				title="Everything the daemon knows about this session, dated (CCT-547)"
				onclick={ondiagnose}
			>🩺 Diagnose</Toggle>
		{/if}
		{#if onterminal}
			<Toggle
				pressed={terminalOpen}
				title="Read-only live view of the session's terminal (CCT-545)"
				onclick={onterminal}
			>🖥 Terminal</Toggle>
		{/if}
	</div>
</div>

<style>
	/* Toolbar (CCT-250 item 2): three visually-separated groups — message-type
	   tag filter, formatting toggles, behavior toggle — divided by thin rules. */
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2) var(--sp-3);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
		overflow-x: auto;
		font-size: var(--fs-xs);
		/* This bar hosts a UI-scale slider (CCT-265), and the app scales by
		   changing the ROOT font-size — so every rem here (button font-size +
		   horizontal padding) grew while dragging, widening the buttons left of
		   the slider and shoving the slider out from under the cursor → the same
		   "seizure" the header had. Pin the bar's size tokens to px (the exact
		   rem values at the 16px base) so the toolbar's geometry is scale-immune:
		   the chat messages in `.conv` still rescale live, the slider's row does
		   not move. Mirrors the `.hd` fix in Header.svelte. */
		--fs-xs: 12px;
		--fs-sm: 13px;
		--sp-1: 4px;
		--sp-2: 8px;
		--sp-3: 12px;
	}
	.tagbar,
	.fmtbar,
	.behbar {
		gap: var(--sp-1);
	}
	.fmtbar,
	.behbar {
		padding-left: var(--sp-3);
		border-left: 1px solid var(--border);
	}
	/* The filter tags, formatting + behavior toggles, and mobile tabs are all
	   <Toggle> chips now; their base/on-state styling lives in Toggle.svelte.
	   Per-use tint (role color / warm auto-approve) is set via --toggle-accent
	   inline. Only the mobile tabs need a layout override (full-width, larger). */
	/* Mobile-tab triggers (CCT-311): hidden on desktop where the groups inline. */
	.mobile-tabs {
		display: none;
	}
	.mobile-tabs :global(.toggle.mtab) {
		flex: 1;
		padding: 0.3rem var(--sp-2);
		font-size: var(--fs-sm);
	}
	@media (max-width: 959px) {
		.toolbar {
			position: relative;
			/* The popovers float above the message log; keep the bar itself a single
			   tidy row of triggers and let panels overlay rather than push content. */
			overflow: visible;
		}
		.mobile-tabs {
			display: flex;
			gap: var(--sp-2);
			width: 100%;
		}
		/* Collapse the inline groups; each reappears as an absolute popover when
		   its trigger is active. */
		.tagbar,
		.fmtbar,
		.behbar {
			display: none;
		}
		.tagbar.panel-open,
		.fmtbar.panel-open,
		.behbar.panel-open {
			display: flex;
			position: absolute;
			top: calc(100% + var(--sp-1));
			left: 0;
			right: 0;
			z-index: 5;
			padding: var(--sp-2);
			/* Drop the desktop divider that separated fmt/beh from the filters. */
			padding-left: var(--sp-2);
			border-left: none;
			background: var(--bg-elevated-2);
			border: 1px solid var(--border-strong);
			border-radius: var(--r-md);
			box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
		}
	}
</style>
