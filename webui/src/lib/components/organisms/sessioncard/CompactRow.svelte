<script lang="ts">
	import { Badge, Cluster, Icon, Text, Timestamp, WorkingDir } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { statusBadgeTone } from '$lib/format';
	import { sessionEndTitle } from '$lib/sessionEnd';
	import Badges from './Badges.svelte';
	import DraftActions from './DraftActions.svelte';
	import Lead from './Lead.svelte';
	import Readout from './Readout.svelte';
	import { type SessionActions, type SessionView, statusLabel } from './view';

	// One real row, no wrap: lead · preview (takes the slack) · cwd · branch ·
	// perm · unread · Σ $ · model · effort · logo · time.
	let { view, actions }: { view: SessionView; actions: SessionActions } = $props();
	const s = $derived(view.s);
</script>

<Cluster wrap={false} gap="var(--sp-2)">
	<Lead {view} {actions} row />
	{#if s.match_snippet || view.lastMsg}
		<Text
			truncate
			tone={s.match_snippet ? 'default' : 'muted'}
			size="xs"
			style="flex:1 1 0;min-width:0"
			>{s.match_snippet ? `🔍 ${s.match_snippet}` : view.lastMsg}</Text
		>
	{:else}
		<span style="flex:1 1 auto"></span>
	{/if}
	<span class="cwd">
		<WorkingDir path={s.working_dir} copy title={m.sessions_workdir_copy_title({ path: s.working_dir })} style="min-width:0;max-width:100%" />
	</span>
	{#if view.branch}
		<span class="branch" title={m.sessions_branch_title({ branch: view.branch })}>
			<Badge mono size="xs" style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%">
				<Icon name="fork" size={12} label={m.sessions_branch_label()} />
				<span class="branch-name">{view.branch}</span>
			</Badge>
		</span>
	{/if}
	{#if view.showStatusBadge}<Badge tone={statusBadgeTone(s.status)} size="xs" style="flex:none"
			>{statusLabel(s.status)}</Badge
		>{/if}
	{#if view.end}<span
			class="end-badge"
			class:end-muted={view.end.muted}
			title={sessionEndTitle(view.end)}><Badge tone={view.end.tone} size="xs">{view.end.badge}</Badge></span
		>{/if}
	<Badges {view} />
	{#if view.draft}
		<DraftActions {view} {actions} />
	{:else}
		<Readout {view} compact />
	{/if}
	{#if s.last_message_at}<span class="time"
			><Timestamp value={s.last_message_at} mode="relative" tone="faint" size="xs" /></span
		>{/if}
</Cluster>

<style>
	.cwd {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
		max-width: 20rem;
		font-size: var(--fs-xs);
		line-height: 1.2;
	}
	.branch {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
		max-width: 14rem;
	}
	.branch-name {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	@container sess-row (max-width: 48rem) {
		.cwd,
		.branch {
			display: none;
		}
	}
	.end-badge {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
		max-width: 22ch;
		overflow: hidden;
	}
	.end-muted {
		opacity: 0.6;
	}
	.time {
		flex: none;
		min-width: 30px;
		text-align: right;
		white-space: nowrap;
	}
</style>
