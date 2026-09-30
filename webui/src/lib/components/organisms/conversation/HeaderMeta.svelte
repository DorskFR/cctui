<script lang="ts">
	// Drawer header meta row: status badge, cwd, branch, token usage, langfuse
	// chip, the in-place codex model editor or the claude "fork to change model"
	// chip, and the adapter logo, which is the trigger of the popover holding
	// what a narrow row dropped.
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { modelShort, statusBadgeTone } from '$lib/format';
	import { sessionEnd, sessionEndTitle } from '$lib/sessionEnd';
	import { branchOf } from '../../../../routes/sessions/sessions.logic';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import TokenUsage from '$lib/components/molecules/TokenUsage.svelte';
	import LangfuseChip from '$lib/components/molecules/LangfuseChip.svelte';
	import PluginChips from '$lib/components/molecules/PluginChips.svelte';
	import { YOUTRACK_PLUGIN_ID, resolveIssueSlot } from '$lib/plugins/issueLink';
	import { Badge, Icon, IconButton, Popover, Select, WorkingDir } from '@dorsk/tsumikit';
	import { codexModelsFor, codexEffortsFor, preferCatalog } from '$lib/harnessModels';
	import { useCodexModels, useMergedCodexModels, useSessionActions } from '$lib/queries';
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
	const branch = $derived(branchOf(session));

	const actions = useSessionActions();
	async function linkIssue(issue: string) {
		const data = await resolveIssueSlot(issue);
		if (data) await actions.setPluginSlot(session.id, YOUTRACK_PLUGIN_ID, data);
	}

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
		data-journey="head-details"
	>
		{#snippet trigger()}<AdapterIcon adapter={session.adapter_id} size={20} />{/snippet}
		<div class="metapop">
			<span class="tokens"><TokenUsage usage={session.token_usage} /></span>
			<span class="langfuse"><LangfuseChip id={session.id} /></span>
			{@render modelMeta('drawer-details')}
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
	/* The popover holds what the row dropped, so it always shows the full model
	   text the narrow row degrades. */
	.metapop {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--sp-2);
		min-width: 0;
		max-width: 100%;
	}
	.metapop .m-full,
	.metapop .m-effort {
		display: inline;
	}
	.metapop .m-short {
		display: none;
	}
	.metapop .langfuse,
	.metapop .tokens,
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
