<script lang="ts">
	// Drawer header meta row: status badge, cwd, branch, token usage, langfuse
	// chip, the in-place codex model editor or the claude "fork to change model"
	// chip, and the adapter logo, which is the trigger of the popover holding
	// what a narrow row dropped.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { compact, modelShort, statusBadgeTone, usd } from '$lib/format';
	import { tokenUsageLayout, bustReasonKey } from '$lib/components/molecules/TokenUsage.logic';
	import { sessionEnd, sessionEndTitle } from '$lib/sessionEnd';
	import { branchOf } from '../../../../routes/sessions/sessions.logic';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import LangfuseChip from '$lib/components/molecules/LangfuseChip.svelte';
	import PluginChips from '$lib/components/molecules/PluginChips.svelte';
	import { YOUTRACK_PLUGIN_ID, resolveIssueSlot } from '$lib/plugins/issueLink';
	import { Badge, Icon, IconButton, Popover, Select, Text, WorkingDir } from '@dorsk/tsumikit';
	import { codexModelsFor, codexEffortsFor, preferCatalog } from '$lib/harnessModels';
	import {
		useCapabilities,
		useCodexModels,
		useMergedCodexModels,
		useSessionActions,
		useSessionLangfuse
	} from '$lib/queries';
	import ModelPicker from '$lib/components/molecules/ModelPicker.svelte';
	import CodexModelsRefresh from '$lib/components/molecules/CodexModelsRefresh.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		archived,
		isCodexSession,
		showStatusBadge,
		onsetmodel,
		onfork,
		detectedIssue = null
	}: {
		session: SessionListItem;
		archived: boolean;
		isCodexSession: boolean;
		showStatusBadge: boolean;
		onsetmodel: (model: string, effort: string) => void;
		onfork: () => void;
		/** An issue id detected for this session, offered as a one-click link
		 *  while nothing is stored. */
		detectedIssue?: string | null;
	} = $props();

	const end = $derived(sessionEnd(session));
	const usage = $derived(session.token_usage);
	const totals = $derived(tokenUsageLayout(usage));
	const bust = $derived(usage.cache_bust ?? null);
	const bustReason = $derived.by(() => {
		switch (bustReasonKey(bust?.reason ?? '')) {
			case 'ttl_expired':
				return m.sessions_token_bust_reason_ttl_expired();
			case 'gateway_rewrote_body':
				return m.sessions_token_bust_reason_gateway_rewrote_body();
			default:
				return m.sessions_token_bust_reason_unknown();
		}
	});
	const branch = $derived(branchOf(session));

	const actions = useSessionActions();
	async function linkIssue(issue: string) {
		const data = await resolveIssueSlot(issue);
		if (data) await actions.setPluginSlot(session.id, YOUTRACK_PLUGIN_ID, data);
	}

	// The details panel is the only place the full readout lives, so its Langfuse
	// figures are fetched on open rather than with the header.
	let detailsOpen = $state(false);
	const caps = useCapabilities();
	const langfuseAvailable = $derived(!!caps.data?.langfuse?.available);
	const lf = useSessionLangfuse(
		() => session.id,
		() => detailsOpen && langfuseAvailable
	);
	const calls = $derived(Number(lf.data?.trace_count ?? 0));

	// In-place model/effort editor, codex only.
	let modelEditing = $state(false);
	let pendingModel = $state('');
	let pendingEffort = $state('');

	// Codex catalog, fetched only while the editor is open: the session
	// machine's own report, else the cross-machine merge, else the static list.
	const machineCodexCatalog = useCodexModels(() =>
		isCodexSession && modelEditing ? session.machine_id : ''
	);
	const mergedCodexCatalog = useMergedCodexModels(() => isCodexSession && modelEditing);
	const codexCatalog = $derived(preferCatalog(machineCodexCatalog.data, mergedCodexCatalog.data));
	const codexModelOptions = $derived(codexModelsFor(codexCatalog));
	const codexEffortOptions = $derived(codexEffortsFor(codexCatalog, pendingModel));

	function openModelEditor() {
		pendingModel = session.model ?? '';
		pendingEffort = session.effort ?? '';
		modelEditing = true;
	}
	function applyModelChange() {
		const model = pendingModel.trim();
		const effort = pendingEffort.trim();
		modelEditing = false;
		if (!model && !effort) return;
		onsetmodel(model, effort);
	}
</script>

{#snippet modelText(model: string)}
	<span class="ellipsis"
		><span class="m-full">{model}</span><span class="m-short">{modelShort(model)}</span
		>{#if session.effort}<span class="m-effort"> · {session.effort}</span>{/if}</span
	>
{/snippet}

{#snippet modelMeta(idPrefix: string)}
	{#if isCodexSession && !archived}
		{#if modelEditing}
			<span class="model-edit">
				<Badge class="row" style="gap:var(--sp-1);padding:0.05rem var(--sp-1)">
					<ModelPicker
						id="{idPrefix}-model"
						compact
						variant="embedded"
						width="auto"
						bind:value={pendingModel}
						options={codexModelOptions}
						aria-label={m.drawer_model_aria()}
					/>
					<CodexModelsRefresh machineId={session.machine_id} size={14} />
					<Select
						variant="embedded"
						width="auto"
						size="sm"
						chevron={false}
						bind:value={pendingEffort}
						aria-label={m.drawer_effort_aria()}
					>
						{#each codexEffortOptions as e (e)}<option value={e}>{e || m.drawer_default_effort()}</option>{/each}
					</Select>
					<IconButton chip variant="default" icon="check" label={m.common_apply()} onclick={applyModelChange} />
					<IconButton chip variant="default" icon="x" label={m.common_cancel()} onclick={() => (modelEditing = false)} />
				</Badge>
			</span>
		{:else}
			<span class="model">
				<Badge
					as="button"
					mono
					title={m.drawer_change_model_title()}
					onclick={openModelEditor}
					style="min-width:0;max-width:100%"
					>{@render modelText(session.model ?? m.drawer_default_model())} ✎</Badge
				>
			</span>
		{/if}
	{:else if session.model || session.effort}
		<span class="model">
			<Badge
				as="button"
				mono
				title={m.drawer_no_inplace_model_title()}
				onclick={onfork}
				style="min-width:0;max-width:100%"
				>{@render modelText(session.model ?? '')} ⑂</Badge
			>
		</span>
	{/if}
{/snippet}

<div class="hmeta" class:editing={modelEditing} data-journey="head-meta">
	{#if showStatusBadge}<Badge tone={statusBadgeTone(session.status)}>{session.status}</Badge>{/if}
	{#if end}<Badge tone={end.tone} title={sessionEndTitle(end)} style={end.muted ? 'opacity:0.6' : undefined}>{end.label}</Badge>{/if}
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
	<div class="meta-trail">
	<span class="tokens"><TokenUsage usage={session.token_usage} /></span>
	<span class="langfuse"><LangfuseChip id={session.id} /></span>
	<span class="plugins">
		<PluginChips
			metadata={session.metadata}
			detected={detectedIssue}
			suggestable={!archived}
			onlink={(issue) => void linkIssue(issue)}
		/>
	</span>
	{@render modelMeta('drawer')}
	<Popover
		label={m.drawer_meta_details()}
		placement="bottom-end"
		box="sm"
		panelStyle="min-width: min(19rem, calc(100vw - 2 * var(--sp-3))); max-width: min(26rem, calc(100vw - 2 * var(--sp-3)))"
		data-journey="head-details"
		bind:open={detailsOpen}
	>
		{#snippet trigger()}<AdapterIcon adapter={session.adapter_id} size={20} />{/snippet}
		<div class="metapop">
			<div class="mp-row mp-model">
				<span class="mp-key"><Text size="xs" tone="faint">{m.drawer_details_model()}</Text></span>
				<span class="mp-val">{@render modelMeta('drawer-details')}</span>
			</div>
			<div class="mp-sep"></div>
			<div class="mp-grid">
				{#each [
					{ k: m.drawer_details_in(), v: compact(Number(usage.tokens_in)) },
					{ k: m.drawer_details_out(), v: compact(Number(usage.tokens_out)) },
					{ k: m.drawer_details_cache_read(), v: compact(Number(usage.cache_read_tokens)) },
					{ k: m.drawer_details_cache_write(), v: compact(Number(usage.cache_creation_tokens)) }
				] as row (row.k)}
					<span class="mp-key"><Text size="xs" tone="faint">{row.k}</Text></span>
					<span class="mp-num"><Text variant="code" size="xs" tone="muted">{row.v}</Text></span>
				{/each}
				<span class="mp-key"><Text size="xs" tone="muted" weight="semibold">{m.drawer_details_total()}</Text></span>
				<span class="mp-num"
					><Text variant="code" size="xs" tone="accent" weight="semibold"
						>{compact(totals.total)}</Text
					></span
				>
				<span class="mp-key"><Text size="xs" tone="muted" weight="semibold">{m.drawer_details_cost()}</Text></span>
				<span class="mp-num"
					><Text variant="code" size="xs" tone="success" weight="semibold">{usd(totals.cost)}</Text
					></span
				>
			</div>
			{#if bust}
				<div class="mp-row">
					<span class="mp-key"><Text size="xs" tone="faint">{m.drawer_details_bust()}</Text></span>
					<span class="mp-val"
						><Text size="xs" tone="danger"
							>💥 {m.sessions_token_bust_hint({
								tokens: compact(Number(bust.lost_tokens)),
								cost: usd(Number(bust.lost_usd)),
								reason: bustReason
							})}</Text
						></span
					>
				</div>
			{/if}
			{#if calls > 0}
				<div class="mp-row">
					<span class="mp-key"><Text size="xs" tone="faint">{m.drawer_details_calls()}</Text></span>
					<span class="mp-num"><Text variant="code" size="xs" tone="muted">{calls}</Text></span>
				</div>
				<div class="mp-row">
					<span class="mp-key"><Text size="xs" tone="faint">{m.drawer_details_langfuse()}</Text></span>
					<span class="mp-val"><LangfuseChip id={session.id} /></span>
				</div>
			{/if}
		</div>
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
	.model {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
	}
	.langfuse,
	.plugins,
	.tokens {
		display: contents;
	}
	.m-short {
		display: none;
	}
	.model-edit {
		display: contents;
	}
	@container drawer-head (max-width: 40rem) {
		.branch {
			max-width: 8rem;
		}
		.langfuse,
		.m-effort,
		.m-full {
			display: none;
		}
		.m-short {
			display: inline;
		}
	}
	/* Narrow: the model chip and the token sum collapse into the logo, which is
	   the row's one fixed, non-growing slot. */
	@container drawer-head (max-width: 26rem) {
		.model,
		.model-edit,
		.tokens {
			display: none;
		}
		.branch {
			min-width: 0;
			max-width: 6rem;
		}
	}
	/* The popover is the full readout, not a spill-over of the row: the meta row
	   degrades to a logo on a phone, so this panel is the only place the whole
	   token breakdown is reachable. Its width is clamped to the viewport by the
	   `panelStyle` so it cannot clip at 390px. */
	.metapop {
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: var(--sp-2);
		min-width: 0;
		max-width: 100%;
	}
	.mp-grid {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: 0.15rem var(--sp-3);
		align-items: baseline;
	}
	.mp-row {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--sp-3);
		min-width: 0;
	}
	.mp-key {
		min-width: 0;
	}
	.mp-num {
		justify-self: end;
		white-space: nowrap;
	}
	.mp-val {
		display: inline-flex;
		align-items: center;
		min-width: 0;
		text-align: right;
	}
	.mp-sep {
		border-top: 1px solid var(--border);
	}
	.metapop .m-full,
	.metapop .m-effort {
		display: inline;
	}
	.metapop .m-short {
		display: none;
	}
	.metapop .model-edit {
		display: contents;
	}
	.metapop .model {
		display: inline-flex;
		max-width: 100%;
	}
	.metapop .ellipsis {
		max-width: 100%;
	}
</style>
