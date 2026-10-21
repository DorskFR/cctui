<script lang="ts">
	// Settings › Context: the rules, memory notes and prompt templates a session
	// can be launched with. Skills are not here — a skill bundle is a plugin, so
	// it lives on the Plugins page.
	import { onMount } from 'svelte';
	import { Badge, Button, Input, Link, Select, Switch, Text, Textarea, Toggle } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import type { ContextItem } from '@bindings/ContextItem';
	import type { ContextItemSpec } from '@bindings/ContextItemSpec';
	import type { ContextMatch } from '@bindings/ContextMatch';
	import {
		useMachineResources,
		useContextActions,
		useContextItems,
		useHarnesses,
		useLabels,
		useResolvedContext
	} from '$lib/queries';
	import { harnessTable } from '$lib/harnesses.svelte';
	import { sessionHref } from '$lib/sessionFailureToast';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import {
		bodyBytes,
		bodyMax,
		contextProblems,
		lineDiff,
		matchSummary,
		newContextItem,
		parsePaths,
		pendingChanges,
		slugify,
		toggleIn,
		toSpec,
		type ContextKind
	} from './context.logic';

	const itemsQ = useContextItems();
	const machinesQ = useMachineResources(() => true);
	const labelsQ = useLabels();
	useHarnesses();
	const actions = useContextActions();

	const machines = $derived(machinesQ.data ?? []);
	const labels = $derived(labelsQ.data?.labels ?? []);
	const harnesses = $derived(harnessTable());
	const items = $derived(itemsQ.data ?? []);
	const ofKind = (kind: ContextKind) => items.filter((i) => i.kind === kind);

	const machineName = (id: string) => {
		const r = machines.find((x) => x.machine_id === id);
		return r ? (r.display_name ?? r.name) : id;
	};
	const labelName = (id: string) => labels.find((l) => l.id === id)?.name ?? id;
	const summary = (mt: ContextMatch | null | undefined) =>
		matchSummary(mt, m.settings_context_match_always(), { machine: machineName, label: labelName });

	// The row being edited: a copy, so nothing lands until Save.
	let draft = $state<ContextItemSpec | null>(null);
	let editing = $state<string | null>(null);
	let pathsText = $state('');
	let busy = $state(false);
	let error = $state<string | null>(null);
	const problems = $derived(draft ? contextProblems(draft) : []);
	const bytes = $derived(draft ? bodyBytes(draft.body ?? '') : 0);

	let reviewId = $state<string | null>(null);
	const reviewing = $derived(items.find((i) => i.id === reviewId && i.pending) ?? null);

	onMount(() => {
		reviewId = new URL(location.href).searchParams.get('review');
	});

	function startNew(kind: ContextKind) {
		editing = null;
		error = null;
		reviewId = null;
		draft = newContextItem(kind);
		pathsText = '';
	}
	function edit(item: ContextItem) {
		editing = item.id;
		error = null;
		reviewId = null;
		draft = toSpec(item);
		pathsText = draft.match.paths.join('\n');
	}
	function cancel() {
		draft = null;
		editing = null;
		error = null;
	}
	async function run(fn: () => Promise<unknown>) {
		busy = true;
		error = null;
		try {
			await fn();
			return true;
		} catch (e) {
			error = errMessage(e);
			return false;
		} finally {
			busy = false;
		}
	}
	async function save() {
		if (!draft || problems.length || busy) return;
		const spec = draft;
		if (await run(() => (editing ? actions.update(editing, spec) : actions.create(spec)))) cancel();
	}
	async function remove(id: string) {
		if (await run(() => actions.remove(id)) && editing === id) cancel();
	}
	async function decide(id: string, approve: boolean) {
		const ok = await run(() => (approve ? actions.approvePending(id) : actions.rejectPending(id)));
		if (ok) reviewId = null;
	}

	const inp = (e: Event) => (e.currentTarget as HTMLInputElement | HTMLTextAreaElement).value;
	const sel = (e: Event) => (e.currentTarget as HTMLSelectElement).value;

	function onTitle(v: string) {
		if (!draft) return;
		const wasSuggested = draft.name === slugify(draft.title ?? '');
		draft.title = v;
		if (!editing && (!draft.name || wasSuggested)) draft.name = slugify(v);
	}
	function toggleMatch(key: 'machines' | 'labels' | 'adapters', v: string) {
		if (draft) draft.match = { ...draft.match, [key]: toggleIn(draft.match[key], v) };
	}
	function onPaths(text: string) {
		pathsText = text;
		if (draft) draft.match = { ...draft.match, paths: parsePaths(text) };
	}

	let previewMachine = $state('');
	let previewDir = $state('');
	let previewAdapter = $state('');
	const preview = useResolvedContext(
		() => ({ machine_id: previewMachine, working_dir: previewDir, adapter: previewAdapter }),
		() => !!previewMachine
	);

	const buckets = $derived([
		{
			kind: 'rule' as const,
			rows: ofKind('rule'),
			title: m.settings_context_rules_title(),
			help: m.settings_context_rules_help(),
			add: m.settings_context_add_rule()
		},
		{
			kind: 'memory' as const,
			rows: ofKind('memory'),
			title: m.settings_context_memory_title(),
			help: m.settings_context_memory_help(),
			add: m.settings_context_add_memory()
		},
		{
			kind: 'prompt' as const,
			rows: ofKind('prompt'),
			title: m.settings_context_prompts_title(),
			help: m.settings_context_prompts_help(),
			add: m.settings_context_add_prompt()
		}
	]);
</script>

{#snippet provenance(item: ContextItem)}
	{@const by = item.updated_by_session ?? item.created_by_session}
	{#if by}
		<Text size="xs" tone="faint">
			<Link href={sessionHref(by)}>{m.settings_context_saved_by({ session: by.slice(0, 8) })}</Link>
		</Text>
	{/if}
{/snippet}

<SettingSection id="context" icon="❖" title={m.settings_nav_context()}>
	{#snippet descriptionSlot()}
		<Text size="sm" tone="faint">{m.settings_context_intro()}</Text>
	{/snippet}

	{#each buckets as bucket (bucket.kind)}
		<SettingGroup title={bucket.title}>
			<SettingRow label={bucket.title} help={bucket.help} wide selfLabelled>
				{#if bucket.rows.length === 0}
					<Text size="sm" tone="faint">{m.settings_context_empty()}</Text>
				{:else}
					<ul class="list">
						{#each bucket.rows as item (item.id)}
							<li class="item" data-setting-row>
								<span class="main">
									<span class="title">{item.title}</span>
									<span class="meta">
										<Text size="xs" tone="faint"
											>{item.name} · {summary(item.match)} · v{item.version}{item.enabled
												? ''
												: ` · ${m.settings_context_disabled()}`}</Text
										>
										{@render provenance(item)}
										{#if item.pending}
											<Badge size="xs" tone="warn">{m.settings_context_pending_badge()}</Badge>
										{/if}
									</span>
								</span>
								<span class="acts">
									{#if item.pending}
										<Button
											size="sm"
											variant="primary"
											disabled={busy}
											onclick={() => {
												cancel();
												reviewId = item.id;
											}}>{m.settings_context_review()}</Button
										>
									{/if}
									<Button size="sm" disabled={busy} onclick={() => edit(item)}>{m.common_edit()}</Button>
									<Button size="sm" variant="danger" disabled={busy} onclick={() => remove(item.id)}
										>{m.common_delete()}</Button
									>
								</span>
							</li>
						{/each}
					</ul>
				{/if}
				{#if !draft}
					<div class="add">
						<Button variant="primary" onclick={() => startNew(bucket.kind)}>{bucket.add}</Button>
					</div>
				{/if}
			</SettingRow>
		</SettingGroup>
	{/each}

	{#if reviewing && reviewing.pending}
		{@const pending = reviewing.pending}
		<SettingGroup title={m.settings_context_review_title({ name: reviewing.name })}>
			<SettingRow label={m.settings_context_review_title({ name: reviewing.name })} wide selfLabelled>
				<div class="review" data-review={reviewing.id}>
					{@render provenance(reviewing)}
					{#each pendingChanges(reviewing, pending, summary) as c (c.field)}
						<div class="change">
							<Text size="xs" weight="medium">{c.field}</Text>
							<span class="d-del">{c.before || '—'}</span>
							<span class="d-add">{c.after || '—'}</span>
						</div>
					{/each}
					<pre class="diff">{#each lineDiff(reviewing.body ?? '', pending.body ?? '') as l, i (i)}<span
								class="d-{l.op}">{l.op === 'add' ? '+ ' : l.op === 'del' ? '- ' : '  '}{l.text}
</span>{/each}</pre>
					<div class="acts">
						<Button variant="primary" disabled={busy} onclick={() => decide(reviewing.id, true)}
							>{m.settings_context_approve()}</Button
						>
						<Button variant="danger" disabled={busy} onclick={() => decide(reviewing.id, false)}
							>{m.settings_context_reject()}</Button
						>
						<Button disabled={busy} onclick={() => (reviewId = null)}>{m.common_cancel()}</Button>
					</div>
					{#if error}
						<Text size="sm" tone="danger">{error}</Text>
					{/if}
				</div>
			</SettingRow>
		</SettingGroup>
	{/if}

	{#if draft}
		<SettingGroup
			title={editing ? m.settings_context_edit_title() : m.settings_context_new_title()}
		>
			<SettingRow label={m.settings_context_field_title()}>
				<Input style="width:100%" value={draft.title} oninput={(e) => onTitle(inp(e))} />
			</SettingRow>
			<SettingRow label={m.settings_context_field_name()} help={m.settings_context_field_name_help()}>
				<Input style="width:100%" value={draft.name} oninput={(e) => (draft!.name = inp(e))} />
			</SettingRow>
			<SettingRow
				label={m.settings_context_field_body()}
				help={draft.kind === 'rule' ? m.settings_context_rule_body_help() : undefined}
				wide
			>
				<Textarea
					style="width:100%"
					rows={8}
					value={draft.body}
					oninput={(e) => (draft!.body = inp(e))}
				/>
				<Text size="xs" tone={bytes > bodyMax(draft.kind) ? 'danger' : 'faint'}
					>{m.settings_context_body_bytes({ used: bytes, max: bodyMax(draft.kind) })}</Text
				>
			</SettingRow>
			{#if draft.kind === 'prompt'}
				<SettingRow label={m.settings_context_vars_label()} help={m.settings_context_vars_help()} wide>
					<Text size="xs" tone="faint">{'{{cwd}} · {{name}} · {{topic}}'}</Text>
				</SettingRow>
			{/if}
			<SettingRow
				label={m.settings_context_field_scope()}
				help={m.settings_context_match_help()}
				wide
				selfLabelled
			>
				<div class="match">
					<Text size="xs" weight="medium">{m.settings_context_match_machines()}</Text>
					<div class="toggles">
						{#each machines as r (r.machine_id)}
							<Toggle
								pill
								pressed={draft.match.machines.includes(r.machine_id)}
								onclick={() => toggleMatch('machines', r.machine_id)}>{r.display_name ?? r.name}</Toggle
							>
						{/each}
					</div>
					<Text size="xs" weight="medium">{m.settings_context_match_paths()}</Text>
					<Textarea
						style="width:100%"
						rows={2}
						placeholder="/home/me/project"
						value={pathsText}
						oninput={(e) => onPaths(inp(e))}
					/>
					<Text size="xs" weight="medium">{m.settings_context_match_labels()}</Text>
					<div class="toggles">
						{#each labels as l (l.id)}
							<Toggle pill pressed={draft.match.labels.includes(l.id)} onclick={() => toggleMatch('labels', l.id)}
								>{l.name}</Toggle
							>
						{/each}
					</div>
					<Text size="xs" weight="medium">{m.settings_context_match_adapters()}</Text>
					<div class="toggles">
						{#each harnesses as h (h.id)}
							<Toggle
								pill
								pressed={draft.match.adapters.includes(h.id)}
								onclick={() => toggleMatch('adapters', h.id)}>{h.label}</Toggle
							>
						{/each}
					</div>
					<Text size="xs" tone="faint">{summary(draft.match)}</Text>
				</div>
			</SettingRow>
			<SettingRow label={m.settings_context_field_enabled()} help={m.settings_context_field_enabled_help()}>
				<Switch
					bind:checked={() => draft!.enabled ?? true, (v) => (draft!.enabled = v)}
					label={m.settings_context_field_enabled()}
				/>
			</SettingRow>
			{#if problems.length || error}
				<SettingRow label={m.settings_context_problems()} wide selfLabelled>
					<Text size="sm" tone="danger"
						>{error ??
							(problems.includes('body_size')
								? m.settings_context_too_long({ max: bodyMax(draft.kind) })
								: m.settings_context_invalid())}</Text
					>
				</SettingRow>
			{/if}
			<SettingRow label={m.common_save()} wide selfLabelled>
				<div class="acts">
					<Button variant="primary" disabled={busy || problems.length > 0} onclick={save}
						>{m.common_save()}</Button
					>
					<Button disabled={busy} onclick={cancel}>{m.common_cancel()}</Button>
				</div>
			</SettingRow>
		</SettingGroup>
	{/if}

	<SettingGroup title={m.settings_context_preview_title()}>
		<SettingRow label={m.spawn_machine_label()} help={m.settings_context_preview_help()}>
			<Select style="width:100%" value={previewMachine} onchange={(e) => (previewMachine = sel(e))}>
				<option value="">{m.settings_macros_pick_machine()}</option>
				{#each machines as r (r.machine_id)}
					<option value={r.machine_id}>{r.display_name ?? r.name}</option>
				{/each}
			</Select>
		</SettingRow>
		<SettingRow label={m.spawn_cwd_label()}>
			<Input
				style="width:100%"
				mono
				placeholder="/home/me/project"
				value={previewDir}
				oninput={(e) => (previewDir = inp(e))}
			/>
		</SettingRow>
		<SettingRow label={m.spawn_field_harness()}>
			<Select style="width:100%" value={previewAdapter} onchange={(e) => (previewAdapter = sel(e))}>
				<option value="">{m.settings_context_preview_any_harness()}</option>
				{#each harnesses as h (h.id)}
					<option value={h.id}>{h.label}</option>
				{/each}
			</Select>
		</SettingRow>
		{#if previewMachine && preview.data}
			<SettingRow label={m.settings_context_preview_result()} wide selfLabelled>
				<div class="preview" data-preview>
					<Text size="xs" weight="medium"
						>{m.context_rules_apply({ count: preview.data.rules.length })}</Text
					>
					<div class="toggles">
						{#each preview.data.rules as r (r.id)}<Badge size="sm">{r.title}</Badge>{/each}
					</div>
					<Text size="xs" weight="medium"
						>{m.settings_context_preview_memories({ count: preview.data.memories.length })}</Text
					>
					<div class="toggles">
						{#each preview.data.memories as r (r.id)}<Badge size="sm">{r.title}</Badge>{/each}
					</div>
				</div>
			</SettingRow>
		{/if}
	</SettingGroup>
</SettingSection>

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		margin: 0;
		padding: 0;
		list-style: none;
		width: 100%;
	}
	.item {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.75rem;
		padding: 0.4rem 0.6rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
	}
	.main {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
		min-width: 0;
	}
	.title {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
	}
	.acts {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		flex: 0 0 auto;
	}
	.add {
		margin-top: 0.6rem;
	}
	.match,
	.review,
	.preview {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		width: 100%;
		min-width: 0;
	}
	.toggles {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-1);
	}
	.change {
		display: grid;
		grid-template-columns: 6rem 1fr 1fr;
		gap: var(--sp-2);
		font-size: var(--fs-xs);
	}
	.diff {
		margin: 0;
		padding: var(--sp-2);
		max-height: 24rem;
		overflow: auto;
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		white-space: pre-wrap;
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
	}
	.d-del {
		color: var(--danger);
		text-decoration: line-through;
	}
	.d-add {
		color: var(--ok);
	}
	.d-same {
		color: var(--text-muted);
	}
</style>
