<script lang="ts">
	// Settings › Context: the rules, memory notes and prompt templates a session
	// can be launched with. Skills are not here — a skill bundle is a plugin, so
	// it lives on the Plugins page.
	import { onMount } from 'svelte';
	import { Badge, Button, Checkbox, Input, Link, Select, Switch, Text, Textarea, Toggle } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import type { ContextItem } from '@bindings/ContextItem';
	import type { ContextItemSpec } from '@bindings/ContextItemSpec';
	import type { ContextMatch } from '@bindings/ContextMatch';
	import {
		useAllMachines,
		useMachineResources,
		useContextActions,
		useContextItems,
		useHarnesses,
		useLabels,
		useMe,
		useResolvedContext,
		useUsers
	} from '#lib/queries/index.ts';
	import { harnessTable } from '#lib/harnesses.svelte.ts';
	import { sessionHref } from '#lib/sessionFailureToast.ts';
	import { errMessage } from '#lib/api.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import {
		bodyBytes,
		bodyMax,
		bulkRequest,
		bulkResult,
		contextProblems,
		foreignMachines,
		groupMachines,
		itemFilter,
		lineDiff,
		matchSummary,
		newContextItem,
		parsePaths,
		pendingChanges,
		reachOf,
		slugify,
		toggleAll,
		toggleIn,
		toSpec,
		type BulkKey,
		type BulkOp,
		type BulkResult,
		type ContextKind,
		type OwnedMachine
	} from './context.logic';

	const itemsQ = useContextItems();
	const machinesQ = useMachineResources(() => true);
	const labelsQ = useLabels();
	useHarnesses();
	const actions = useContextActions();

	const me = useMe();
	const meId = $derived(me.data?.user_id ?? null);
	const isAdmin = $derived(me.data?.role === 'admin');
	const allMachinesQ = useAllMachines(() => isAdmin);
	const usersQ = useUsers(() => isAdmin);

	const machines = $derived(machinesQ.data ?? []);
	const labels = $derived(labelsQ.data?.labels ?? []);
	const harnesses = $derived(harnessTable());
	const items = $derived(itemsQ.data ?? []);

	const owned = $derived<OwnedMachine[]>(
		isAdmin && allMachinesQ.data
			? allMachinesQ.data.map((r) => ({ id: r.id, owner: r.user_id, name: r.display_name ?? r.name }))
			: machines.map((r) => ({ id: r.machine_id, owner: meId ?? '', name: r.display_name ?? r.name }))
	);
	const ownerName = (id: string) => usersQ.data?.find((u) => u.id === id)?.name ?? id.slice(0, 8);
	const groups = $derived(groupMachines(owned, meId, ownerName));

	const machineName = (id: string) => owned.find((x) => x.id === id)?.name ?? id;
	const labelName = (id: string) => labels.find((l) => l.id === id)?.name ?? id;
	const words = $derived({
		mine: m.settings_context_reach_mine(),
		allUsers: m.settings_context_reach_all_users(),
		dispatches: m.settings_context_reach_dispatches()
	});
	const summary = (mt: ContextMatch | null | undefined) =>
		matchSummary(mt, words, { machine: machineName, label: labelName });

	let filter = $state('');
	const shown = $derived(items.filter(itemFilter(filter)));
	const ofKind = (kind: ContextKind) => shown.filter((i) => i.kind === kind);
	let selected = $state<Set<string>>(new Set());
	const picked = $derived(items.filter((i) => selected.has(i.id)).map((i) => i.id));
	function toggleRow(id: string) {
		const next = new Set(selected);
		if (!next.delete(id)) next.add(id);
		selected = next;
	}
	let bulkOp = $state<'add' | 'remove'>('add');
	let bulkKey = $state<BulkKey>('machines');
	let bulkValue = $state('');
	let bulkOutcome = $state<BulkResult | null>(null);

	// The row being edited: a copy, so nothing lands until Save.
	let draft = $state<ContextItemSpec | null>(null);
	let editing = $state<string | null>(null);
	let pathsText = $state('');
	let busy = $state(false);
	let error = $state<string | null>(null);
	const problems = $derived(draft ? contextProblems(draft) : []);
	const foreign = $derived(
		draft && reachOf(draft.match) !== 'all_users' ? foreignMachines(draft.match, owned, meId) : []
	);
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
		if (!draft || problems.length || foreign.length || busy) return;
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
	const commaList = (text: string) =>
		text
			.split(',')
			.map((v) => v.trim())
			.filter(Boolean);
	function setReach(all: boolean) {
		if (draft) draft.match = { ...draft.match, reach: all ? 'all_users' : 'owner' };
	}
	function setDispatch(on: boolean) {
		if (draft) draft.match = { ...draft.match, dispatch: on };
	}
	function onNarrow(key: 'dispatchers' | 'profiles', text: string) {
		if (draft) draft.match = { ...draft.match, [key]: commaList(text) };
	}

	async function applyBulk(change: BulkOp) {
		if (!picked.length || busy) return;
		bulkOutcome = null;
		let outcomes: Awaited<ReturnType<typeof actions.bulk>> = [];
		if (await run(async () => (outcomes = await actions.bulk(bulkRequest(picked, change))))) {
			bulkOutcome = bulkResult(outcomes);
		}
	}
	const bulkValues = () => (bulkKey === 'paths' ? parsePaths(bulkValue) : bulkValue ? [bulkValue] : []);

	let previewMachine = $state('');
	let previewDir = $state('');
	let previewAdapter = $state('');
	let previewDispatch = $state(false);
	let previewDispatcher = $state('');
	let previewProfile = $state('');
	const preview = useResolvedContext(
		() => ({
			machine_id: previewMachine,
			working_dir: previewDir,
			adapter: previewAdapter,
			dispatch: previewDispatch,
			dispatcher: previewDispatcher,
			profile: previewProfile
		}),
		() => !!previewMachine || previewDispatch
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

	{#if items.length}
		<SettingGroup title={m.settings_context_filter_placeholder()}>
			<SettingRow label={m.settings_context_filter_placeholder()} wide selfLabelled>
				<div class="bulkbar">
					<Input
						style="flex:1 1 14rem"
						placeholder={m.settings_context_filter_placeholder()}
						aria-label={m.settings_context_filter_placeholder()}
						value={filter}
						oninput={(e) => (filter = inp(e))}
					/>
					<Button size="sm" onclick={() => (selected = toggleAll(selected, shown.map((i) => i.id)))}
						>{m.settings_context_select_all()}</Button
					>
				</div>
			</SettingRow>
		</SettingGroup>
	{/if}

	{#if picked.length}
		<SettingGroup title={m.settings_context_bulk_title({ count: picked.length })}>
			<SettingRow label={m.settings_context_bulk_title({ count: picked.length })} wide selfLabelled>
				<div class="match" data-bulk>
					<div class="bulkbar">
						<Select value={bulkOp} onchange={(e) => (bulkOp = sel(e) as 'add' | 'remove')}>
							<option value="add">{m.settings_context_bulk_add()}</option>
							<option value="remove">{m.settings_context_bulk_remove()}</option>
						</Select>
						<Select
							value={bulkKey}
							onchange={(e) => {
								bulkKey = sel(e) as BulkKey;
								bulkValue = '';
							}}
						>
							{#if hasFeature('machine_spawn')}
								<option value="machines">{m.settings_context_match_machines()}</option>
							{/if}
							<option value="paths">{m.settings_context_match_paths()}</option>
							{#if hasFeature('organising')}
								<option value="labels">{m.settings_context_match_labels()}</option>
							{/if}
							<option value="adapters">{m.settings_context_match_adapters()}</option>
						</Select>
						{#if bulkKey === 'machines'}
							<Select value={bulkValue} onchange={(e) => (bulkValue = sel(e))}>
								<option value=""></option>
								{#each groups as g (g.owner)}
									<optgroup
										label={g.mine ? m.settings_context_my_machines() : m.settings_context_another_user({ name: ownerName(g.owner) })}
									>
										{#each g.machines as r (r.id)}<option value={r.id}>{r.name}</option>{/each}
									</optgroup>
								{/each}
							</Select>
						{:else if bulkKey === 'labels'}
							<Select value={bulkValue} onchange={(e) => (bulkValue = sel(e))}>
								<option value=""></option>
								{#each labels as l (l.id)}<option value={l.id}>{l.name}</option>{/each}
							</Select>
						{:else if bulkKey === 'adapters'}
							<Select value={bulkValue} onchange={(e) => (bulkValue = sel(e))}>
								<option value=""></option>
								{#each harnesses as h (h.id)}<option value={h.id}>{h.label}</option>{/each}
							</Select>
						{:else}
							<Input mono placeholder="/home/me/project" value={bulkValue} oninput={(e) => (bulkValue = inp(e))} />
						{/if}
						<Button
							size="sm"
							variant="primary"
							disabled={busy || !bulkValues().length}
							onclick={() => applyBulk({ op: bulkOp, key: bulkKey, values: bulkValues() })}
							>{m.settings_context_bulk_apply()}</Button
						>
					</div>
					<div class="bulkbar">
						{#if hasFeature('machine_spawn')}
							<Button size="sm" disabled={busy} onclick={() => applyBulk({ op: 'clear_machines' })}
								>{m.settings_context_bulk_clear_machines()}</Button
							>
						{/if}
						{#if isAdmin}
							<Button size="sm" disabled={busy} onclick={() => applyBulk({ op: 'reach', reach: 'owner' })}
								>{m.settings_context_bulk_reach_mine()}</Button
							>
							<Button size="sm" disabled={busy} onclick={() => applyBulk({ op: 'reach', reach: 'all_users' })}
								>{m.settings_context_bulk_reach_all()}</Button
							>
						{/if}
						<Button size="sm" disabled={busy} onclick={() => applyBulk({ op: 'dispatch', on: true })}
							>{m.settings_context_bulk_dispatch_on()}</Button
						>
						<Button size="sm" disabled={busy} onclick={() => applyBulk({ op: 'dispatch', on: false })}
							>{m.settings_context_bulk_dispatch_off()}</Button
						>
						<Button size="sm" disabled={busy} onclick={() => (selected = new Set())}
							>{m.settings_context_bulk_clear_selection()}</Button
						>
					</div>
					{#if bulkOutcome}
						<Text size="sm" tone={bulkOutcome.failed.length ? 'danger' : 'faint'} data-bulk-result
							>{m.settings_context_bulk_result({ updated: bulkOutcome.updated, failed: bulkOutcome.failed.length })}</Text
						>
						{#each bulkOutcome.failed as f (f.id)}
							<Text size="xs" tone="danger"
								>{items.find((i) => i.id === f.id)?.title ?? f.id.slice(0, 8)}: {f.error}</Text
							>
						{/each}
					{:else if error && !draft}
						<Text size="sm" tone="danger">{error}</Text>
					{/if}
				</div>
			</SettingRow>
		</SettingGroup>
	{/if}

	{#each buckets as bucket (bucket.kind)}
		<SettingGroup title={bucket.title}>
			<SettingRow label={bucket.title} help={bucket.help} wide selfLabelled>
				{#if bucket.rows.length === 0}
					<Text size="sm" tone="faint">{m.settings_context_empty()}</Text>
				{:else}
					<ul class="list">
						{#each bucket.rows as item (item.id)}
							<li class="item" data-setting-row>
								<Checkbox
									label={m.settings_context_select_item({ title: item.title })}
									labelHidden
									checked={selected.has(item.id)}
									onchange={() => toggleRow(item.id)}
								/>
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
					{#if hasFeature('machine_spawn')}
						{@const wide = reachOf(draft.match) === 'all_users'}
						<Text size="xs" weight="medium">{m.settings_context_match_machines()}</Text>
						{#if isAdmin}
							<Switch
								bind:checked={() => wide, (v) => setReach(v)}
								label={m.settings_context_reach_all_users_toggle()}
								labelVisible
							/>
							<Text size="xs" tone="faint">{m.settings_context_reach_all_users_help()}</Text>
						{/if}
						<div class="toggles">
							<Toggle
								pill
								pressed={!draft.match.machines.length}
								onclick={() => draft && (draft.match = { ...draft.match, machines: [] })}
								>{m.settings_context_my_machines()}</Toggle
							>
							{#each groups.filter((g) => g.mine) as g (g.owner)}
								{#each g.machines as r (r.id)}
									<Toggle pill pressed={draft.match.machines.includes(r.id)} onclick={() => toggleMatch('machines', r.id)}
										>{r.name}</Toggle
									>
								{/each}
							{/each}
						</div>
						{#if isAdmin && wide}
							{#each groups.filter((g) => !g.mine) as g (g.owner)}
								<Text size="xs" tone="faint">{m.settings_context_another_user({ name: ownerName(g.owner) })}</Text>
								<div class="toggles">
									{#each g.machines as r (r.id)}
										<Toggle pill pressed={draft.match.machines.includes(r.id)} onclick={() => toggleMatch('machines', r.id)}
											>{r.name}</Toggle
										>
									{/each}
								</div>
							{/each}
						{/if}
						{#if foreign.length}
							<Text size="xs" tone="danger" data-foreign
								>{m.settings_context_foreign_machines({ count: foreign.length })}</Text
							>
						{/if}
					{/if}
					<Text size="xs" weight="medium">{m.settings_context_match_paths()}</Text>
					<Textarea
						style="width:100%"
						rows={2}
						placeholder="/home/me/project"
						value={pathsText}
						oninput={(e) => onPaths(inp(e))}
					/>
					{#if hasFeature('organising')}
						<Text size="xs" weight="medium">{m.settings_context_match_labels()}</Text>
						<div class="toggles">
							{#each labels as l (l.id)}
								<Toggle pill pressed={draft.match.labels.includes(l.id)} onclick={() => toggleMatch('labels', l.id)}
									>{l.name}</Toggle
								>
							{/each}
						</div>
					{/if}
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
					<Switch
						bind:checked={() => !!draft?.match.dispatch, (v) => setDispatch(v)}
						label={m.settings_context_dispatch_toggle()}
						labelVisible
					/>
					{#if draft.match.dispatch}
						<Text size="xs" tone="faint">{m.settings_context_dispatch_help()}</Text>
						<Input
							style="width:100%"
							placeholder={m.settings_context_dispatchers()}
							aria-label={m.settings_context_dispatchers()}
							value={(draft.match.dispatchers ?? []).join(', ')}
							onchange={(e) => onNarrow('dispatchers', inp(e))}
						/>
						<Input
							style="width:100%"
							placeholder={m.settings_context_profiles()}
							aria-label={m.settings_context_profiles()}
							value={(draft.match.profiles ?? []).join(', ')}
							onchange={(e) => onNarrow('profiles', inp(e))}
						/>
					{/if}
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
					<Button variant="primary" disabled={busy || problems.length > 0 || foreign.length > 0} onclick={save}
						>{m.common_save()}</Button
					>
					<Button disabled={busy} onclick={cancel}>{m.common_cancel()}</Button>
				</div>
			</SettingRow>
		</SettingGroup>
	{/if}

	{#if hasFeature('machine_spawn')}
		<SettingGroup title={m.settings_context_preview_title()}>
			<SettingRow label={m.spawn_machine_label()} help={m.settings_context_preview_help()}>
				<Select
					style="width:100%"
					value={previewMachine}
					disabled={previewDispatch}
					onchange={(e) => (previewMachine = sel(e))}
				>
					<option value="">{m.settings_macros_pick_machine()}</option>
					{#each groups as g (g.owner)}
						<optgroup
							label={g.mine ? m.settings_context_my_machines() : m.settings_context_another_user({ name: ownerName(g.owner) })}
						>
							{#each g.machines as r (r.id)}<option value={r.id}>{r.name}</option>{/each}
						</optgroup>
					{/each}
				</Select>
			</SettingRow>
			<SettingRow label={m.settings_context_preview_dispatch()}>
				<Switch bind:checked={previewDispatch} label={m.settings_context_preview_dispatch()} />
			</SettingRow>
			{#if previewDispatch}
				<SettingRow label={m.settings_context_preview_dispatcher()}>
					<Input style="width:100%" value={previewDispatcher} oninput={(e) => (previewDispatcher = inp(e))} />
				</SettingRow>
				<SettingRow label={m.settings_context_preview_profile()}>
					<Input style="width:100%" value={previewProfile} oninput={(e) => (previewProfile = inp(e))} />
				</SettingRow>
			{/if}
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
			{#if (previewMachine || previewDispatch) && preview.data}
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
	{/if}
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
		background: var(--bg);
		border-radius: var(--r-sm);
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
	.bulkbar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		width: 100%;
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
		background: var(--bg);
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
