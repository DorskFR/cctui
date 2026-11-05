<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { modelShort } from '$lib/format';
	import { Badge, IconButton, Select } from '@dorsk/tsumikit';
	import { harnessModelsFallback } from '$lib/domainTables';
	import { harnessCapabilities } from '$lib/harnesses';
	import { harnessTable } from '$lib/harnesses.svelte';
	import { useHarnessModels } from '$lib/queries';
	import ModelPicker from '$lib/components/molecules/ModelPicker.svelte';
	import CodexModelsRefresh from '$lib/components/molecules/CodexModelsRefresh.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		archived,
		isCodexSession,
		idPrefix,
		inPopover = false,
		editing = $bindable(false),
		onsetmodel,
		onfork
	}: {
		session: SessionListItem;
		archived: boolean;
		isCodexSession: boolean;
		idPrefix: string;
		/** The details popover always shows the full form; the row degrades with its width. */
		inPopover?: boolean;
		editing?: boolean;
		onsetmodel: (model: string, effort: string) => void;
		onfork: () => void;
	} = $props();

	const canSetModel = $derived(harnessCapabilities(session.adapter_id, harnessTable()).set_model);
	let pendingModel = $state('');
	let pendingEffort = $state('');

	// Fetched only while the editor is open; the server narrows the catalog to
	// the session's machine and the effort list to the pending model.
	const codexModels = useHarnessModels(
		() => 'codex',
		() => session.machine_id,
		() => pendingModel,
		() => isCodexSession && editing
	);
	const modelOptions = $derived(codexModels.data?.models ?? harnessModelsFallback('codex').models);
	const effortOptions = $derived(codexModels.data?.efforts ?? harnessModelsFallback('codex').efforts);

	function openEditor() {
		pendingModel = session.model ?? '';
		pendingEffort = session.effort ?? '';
		editing = true;
	}
	function apply() {
		const model = pendingModel.trim();
		const effort = pendingEffort.trim();
		editing = false;
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

<span class="hm" class:strip={!inPopover}>
	{#if canSetModel && !archived}
		{#if editing}
			<span class="model-edit">
				<Badge class="row" style="gap:var(--sp-1);padding:0.05rem var(--sp-1)">
					<ModelPicker
						id="{idPrefix}-model"
						compact
						variant="embedded"
						width="auto"
						bind:value={pendingModel}
						options={modelOptions}
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
						{#each effortOptions as e (e)}<option value={e}>{e || m.drawer_default_effort()}</option>{/each}
					</Select>
					<IconButton chip variant="default" icon="check" label={m.common_apply()} onclick={apply} />
					<IconButton chip variant="default" icon="x" label={m.common_cancel()} onclick={() => (editing = false)} />
				</Badge>
			</span>
		{:else}
			<span class="model">
				<Badge
					as="button"
					mono
					title={m.drawer_change_model_title()}
					onclick={openEditor}
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
				style="min-width:0;max-width:100%">{@render modelText(session.model ?? '')} ⑂</Badge
			>
		</span>
	{/if}
</span>

<style>
	.hm {
		display: contents;
	}
	.model {
		display: inline-flex;
		flex: 0 1 auto;
		min-width: 0;
		max-width: 100%;
	}
	.model-edit {
		display: contents;
	}
	.ellipsis {
		min-width: 0;
		max-width: 100%;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.m-short {
		display: none;
	}
	@container drawer-head (max-width: 40rem) {
		.strip .m-effort,
		.strip .m-full {
			display: none;
		}
		.strip .m-short {
			display: inline;
		}
	}
	@container drawer-head (max-width: 26rem) {
		.strip .model,
		.strip .model-edit {
			display: none;
		}
	}
</style>
