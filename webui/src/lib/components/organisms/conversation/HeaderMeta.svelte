<script lang="ts">
	// Drawer header meta row: status, thread links, cwd, branch, one token total,
	// one cost, the model, and the adapter logo, which opens the full readout.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { statusBadgeTone, usd } from '$lib/format';
	import { tokenUsageLayout } from '$lib/components/molecules/TokenUsage.logic';
	import { sessionEnd, sessionEndTitle } from '$lib/sessionEnd';
	import { branchOf } from '../../../../routes/sessions/sessions.logic';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import StatText from '$lib/components/atoms/StatText.svelte';
	import ThreadMenu from './ThreadMenu.svelte';
	import LangfuseChip from '$lib/components/molecules/LangfuseChip.svelte';
	import HeaderModel from './HeaderModel.svelte';
	import HeaderMetaDetails from './HeaderMetaDetails.svelte';
	import { headerCost } from './headerMeta.logic';
	import { Badge, Icon, Popover, Text, WorkingDir } from '@dorsk/tsumikit';
	import { useCapabilities, useSessionActions, useSessionLangfuse } from '$lib/queries';
	import { guardOff } from '$lib/tool-guard-bypass.logic';
	import GuardStepChip from '$lib/components/molecules/GuardStepChip.svelte';
	import type { GuardStepState } from '@bindings/GuardStepState';
	import { ws } from '$lib/ws.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		archived,
		isCodexSession,
		showStatusBadge,
		box = 'md',
		onsetmodel,
		onfork
	}: {
		session: SessionListItem;
		archived: boolean;
		isCodexSession: boolean;
		showStatusBadge: boolean;
		box?: 'sm' | 'md';
		onsetmodel: (model: string, effort: string) => void;
		onfork: () => void;
	} = $props();

	const end = $derived(sessionEnd(session));
	const totals = $derived(tokenUsageLayout(session.token_usage));
	const branch = $derived(branchOf(session));

	let detailsOpen = $state(false);
	let modelEditing = $state(false);

	// Read with the header, not on open: which cost the row shows depends on it.
	const caps = useCapabilities();
	const langfuseAvailable = $derived(!!caps.data?.langfuse?.available);
	const lf = useSessionLangfuse(
		() => session.id,
		() => langfuseAvailable
	);
	const calls = $derived(Number(lf.data?.trace_count ?? 0));
	const cost = $derived(headerCost(Number(session.token_usage.cost_usd) || 0, lf.data));

	// Workflow-guard step: seeded by one HTTP read, then live off the socket.
	// The pod's guard stays authoritative; a request is answered by its verdict.
	const sessionActions = useSessionActions();
	let guardStep = $state<GuardStepState | null>(null);
	let guardBusy = $state(false);
	$effect(() => {
		const id = session.id;
		guardStep = ws.guardStep(id);
		const off = ws.onGuardStep(id, (ev) => (guardStep = ev.state));
		ws.seedGuardStep(id, sessionActions.guardStep(id)).catch(() => {});
		return off;
	});
	async function requestGuardTransition(step: string) {
		guardBusy = true;
		try {
			const res = await sessionActions.guardTransition(session.id, step);
			const ack = await ws.awaitCommand(res.command_id, 30_000);
			if (ack.ok) toasts.ok(m.guard_step_moved({ step }));
			else if (ack.timedOut) toasts.info(m.guard_step_unconfirmed());
			else toasts.error(m.guard_step_refused({ error: ack.error ?? '?' }));
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			guardBusy = false;
		}
	}
</script>

<div class="hmeta" class:editing={modelEditing} data-journey="head-meta">
	{#if showStatusBadge}<Badge tone={statusBadgeTone(session.status)}>{session.status}</Badge>{/if}
	{#if end}<Badge tone={end.tone} title={sessionEndTitle(end)} style={end.muted ? 'opacity:0.6' : undefined}>{end.label}</Badge>{/if}
	<ThreadMenu {session} {box} />
	<span class="cwd">
		<WorkingDir
			path={session.working_dir}
			copy
			shrink
			title={m.sessions_workdir_copy_title({ path: session.working_dir })}
			style="min-width:min(9rem,40%)"
		/>
	</span>
	{#if branch}
		<span class="branch">
			<Badge mono title={m.sessions_branch_title({ branch })} style="display:inline-flex;align-items:center;gap:0.25em;min-width:0;max-width:100%">
				<Icon name="fork" size={12} label={m.sessions_branch_label()} />
				<span class="ellipsis">{branch}</span>
			</Badge>
		</span>
	{/if}
	{#if guardStep && !session.ended_at}
		<GuardStepChip
			step={guardStep}
			disabled={guardBusy}
			ontransition={archived ? undefined : requestGuardTransition}
		/>
	{/if}
	{#if guardOff(session)}
		<Badge tone="warn" shrink={false} title={m.tool_guard_badge_title()} data-testid="tool-guard-off"
			>{m.tool_guard_badge()}</Badge
		>
	{/if}
	<div class="meta-trail">
		{#if totals.total > 0}
			<span class="tokens"><Text size="xs" tone="muted"><StatText value={totals.total} kind="tokens" /></Text></span>
		{/if}
		{#if cost}
			<span class="cost" data-testid="header-cost-chip">
				{#if cost.source === 'langfuse'}
					<LangfuseChip id={session.id} />
				{:else}
					<Text numeric size="xs" tone="muted" title={m.chrome_cost_estimate_title()}
						>{m.chrome_cost_estimate({ cost: usd(cost.value) })}</Text
					>
				{/if}
			</span>
		{/if}
		<HeaderModel {session} {archived} {isCodexSession} idPrefix="drawer" bind:editing={modelEditing} {onsetmodel} {onfork} />
		<Popover
			label={m.drawer_meta_details()}
			placement="bottom-end"
			box="sm"
			panelStyle="min-width: min(19rem, calc(100vw - 2 * var(--sp-3))); max-width: min(26rem, calc(100vw - 2 * var(--sp-3)))"
			data-journey="head-details"
			bind:open={detailsOpen}
		>
			{#snippet trigger()}<AdapterIcon adapter={session.adapter_id} size={20} />{/snippet}
			<HeaderMetaDetails
				{session}
				{archived}
				{isCodexSession}
				open={detailsOpen}
				{cost}
				{calls}
				{onsetmodel}
				{onfork}
			/>
		</Popover>
	</div>
</div>

<style>
	/* One row: the model gives way first, then the branch; the cwd holds out
	   longest. Widths come from the `drawer-head` container DrawerHeader declares. */
	.hmeta {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.hmeta.editing {
		flex-wrap: wrap;
	}
	.cwd {
		display: contents;
	}
	.branch {
		display: inline-flex;
		flex: 0 3 auto;
		min-width: 4.5rem;
		max-width: 14rem;
	}
	.ellipsis {
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.meta-trail {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex: 0 8 auto;
		min-width: 0;
		margin-left: auto;
	}
	.tokens,
	.cost {
		display: inline-flex;
		flex: none;
	}
	@container drawer-head (max-width: 40rem) {
		.branch {
			max-width: 8rem;
		}
		.cost {
			display: none;
		}
	}
	@container drawer-head (max-width: 26rem) {
		.tokens {
			display: none;
		}
		.branch {
			min-width: 0;
			max-width: 6rem;
		}
	}
</style>
