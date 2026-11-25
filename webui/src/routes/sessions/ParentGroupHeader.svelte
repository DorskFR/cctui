<script lang="ts">
	import type { SessionParentStub } from '@bindings/SessionParentStub';
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { Dot } from '@dorsk/tsumikit';
	import SessionDot from '#lib/components/molecules/SessionDot.svelte';
	import { livenessClassOf } from '#lib/components/organisms/sessioncard/view.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { parentStubTitle } from './sessions.logic';

	let {
		parent,
		session,
		count,
		open,
		ontoggle,
		onopenparent
	}: {
		parent: SessionParentStub;
		/** The parent's full row when it is loaded: its dot gets the live tooltip. */
		session?: SessionListItem;
		count: number;
		open: boolean;
		ontoggle: () => void;
		onopenparent: (id: string) => void;
	} = $props();

	const title = $derived(parentStubTitle(parent));
	const live = $derived(parent.status !== 'archived');
</script>

<div class="parent-group-header" data-parent-id={parent.id}>
	<span class="slot">
		{#if session}
			<SessionDot {session} livenessClass={livenessClassOf(session, false)} />
		{:else}
			<span title={live ? m.sessions_parent_live() : m.sessions_status_archived()}
				><Dot status={live ? 'active' : 'dead'} /></span
			>
		{/if}
	</span>
	<button
		type="button"
		class="name"
		title={m.sessions_parent_open({ name: title })}
		onclick={() => onopenparent(parent.id)}>{title}</button
	>
	<button type="button" class="sub" aria-expanded={open} onclick={ontoggle}
		>{m.sessions_parent_children({ n: count })}</button
	>
</div>

<style>
	.parent-group-header {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
		padding: 7px 14px 7px calc(5px + 3px + 14px + 6px);
		border-bottom: 1px solid var(--border);
		font-size: var(--fs-xs);
		color: var(--text-muted);
	}
	.slot {
		display: inline-flex;
		justify-content: center;
		width: 16px;
		flex: none;
	}
	.name {
		appearance: none;
		background: none;
		border: 0;
		padding: 0;
		font: inherit;
		font-size: var(--fs-md);
		font-weight: var(--fw-semibold);
		color: var(--text-muted);
		cursor: pointer;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.name:hover,
	.name:focus-visible {
		color: var(--text);
		text-decoration: underline;
	}
	.sub {
		appearance: none;
		background: none;
		font: inherit;
		font-weight: var(--fw-medium);
		line-height: 18px;
		white-space: nowrap;
		cursor: pointer;
		flex: none;
		padding: 0 8px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--info) 60%, transparent);
		color: var(--info);
	}
	.sub:hover {
		background: color-mix(in srgb, var(--info) 16%, transparent);
	}
	.sub[aria-expanded='true'] {
		background: color-mix(in srgb, var(--info) 30%, transparent);
		color: var(--text);
	}
</style>
