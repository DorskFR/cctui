<script lang="ts">
	// The "Machine" branch of the spawn form: where (machine badge · cwd ·
	// branch), the session name, and the prompt. The harness / account / model /
	// effort / permission knobs come from the selected profile (ProfileList).
	import type { Snippet } from 'svelte';
	import type { MachineResourcesRow } from '@bindings/MachineResourcesRow';
	import { useGitInfo } from '$lib/queries';
	import PromptField from '$lib/components/organisms/PromptField.svelte';
	import type { PromptAttachments } from '$lib/promptAttachments.svelte';
	import type { GitInfo } from '@bindings/GitInfo';
	import MachinePicker from '$lib/components/molecules/MachinePicker.svelte';
	import {
		Callout,
		Field,
		FilterInput,
		Icon,
		Input,
		Kbd,
		Link,
		WorkingDir,
		type Query
	} from '@dorsk/tsumikit';
	import { makeCwdSchema, dirFromQuery } from './cwdSchema';
	import { fsLiveness, gitBadge, makeGitInfoWatcher } from './cwdGitInfo';
	import type { Form } from './types';
	import { m } from '$lib/paraglide/messages';
	import { promptHistory } from '$lib/drafts';
	import { HistoryNav } from '$lib/historyNav';
	import PromptHistoryMenu from '$lib/components/molecules/PromptHistoryMenu.svelte';
	import RedactedChips from '$lib/components/organisms/conversation/RedactedChips.svelte';

	let {
		form = $bindable(),
		machines,
		recentDirs,
		onsubmit,
		att,
		promptEl = $bindable(null),
		tools: extraTools
	}: {
		form: Form;
		machines: MachineResourcesRow[];
		recentDirs: string[];
		onsubmit?: () => void;
		att?: PromptAttachments;
		promptEl?: HTMLTextAreaElement | null;
		/** Extra controls for the prompt field's toolbar, after prompt history. */
		tools?: Snippet;
	} = $props();

	let revealed = $state<ReadonlySet<string>>(new Set());

	const nav = new HistoryNav({
		list: () => promptHistory.get(),
		value: () => form.prompt,
		setValue: (v) => (form.prompt = v),
		el: () => promptEl
	});

	// The machine picker and the path share one control; `form.working_dir` is
	// the source of truth and the field's bare value mirrors it both ways,
	// `lastDir` tracking what the field holds so the two syncs never loop.
	const cwdSchema = makeCwdSchema(
		() => (liveness === 'offline' ? '' : form.machine_id),
		() => recentDirs,
		m.spawn_cwd_label()
	);
	// svelte-ignore state_referenced_locally
	let cwdRaw = $state(form.working_dir);
	// svelte-ignore state_referenced_locally
	let lastDir = form.working_dir;
	function onCwdChange(q: Query) {
		const dir = dirFromQuery(q);
		// The input re-emits its unchanged query on mount and on rerenders; only
		// a real move away from what the field held is a user edit.
		if (dir === lastDir) return;
		lastDir = dir;
		form.working_dir = dir;
	}
	$effect(() => {
		const dir = form.working_dir;
		if (dir !== lastDir) {
			lastDir = dir;
			cwdRaw = dir;
		}
	});

	const fetchGitInfo = useGitInfo();
	let cwdGit = $state<GitInfo | null>(null);
	const cwdBadge = $derived(gitBadge(cwdGit));
	const gitWatcher = makeGitInfoWatcher(fetchGitInfo, (info) => (cwdGit = info));
	const liveness = $derived(fsLiveness(machines.find((mc) => mc.machine_id === form.machine_id)));
	$effect(() => {
		gitWatcher.update(form.machine_id, form.working_dir, liveness);
		return gitWatcher.cancel;
	});
	const cwdBadgeTitle = $derived.by(() => {
		if (!cwdBadge) return '';
		if (cwdBadge.sha) return m.spawn_cwd_detached_title({ sha: cwdBadge.sha });
		if (cwdBadge.worktree) return m.spawn_cwd_worktree_title({ branch: cwdBadge.text });
		return m.spawn_cwd_branch_title({ branch: cwdBadge.text });
	});
</script>

{#if machines.length === 0}
	<Callout tone="warn" icon="info">
		{m.spawn_no_machines_hint()}
		<Link href="/">{m.nav_overview()}</Link>
	</Callout>
{/if}

<div class="where" data-journey="where">
	<Field label={m.spawn_cwd_label()} for="sp-cwd">
		<!-- Two sibling controls, never one nested in the other's box. -->
		<div class="where-row">
			<MachinePicker bind:value={form.machine_id} {machines} label={m.spawn_machine_label()} />
			<div class="cwd">
				<FilterInput
					id="sp-cwd"
					key="cwd"
					schema={cwdSchema}
					bind:value={cwdRaw}
					icon={null}
					showClear={false}
					placeholder="/home/user/project"
					title={form.working_dir || undefined}
					onchange={onCwdChange}
				>
					{#snippet display()}
						<WorkingDir path={form.working_dir} shrink minLeaf={12} />
					{/snippet}
				</FilterInput>
			</div>
		</div>
	</Field>
	<!-- Always one line tall so the form doesn't jump when a branch resolves. -->
	<span class="branch" title={cwdBadge ? cwdBadgeTitle : undefined}>
		{#if cwdBadge}
			<Icon name="fork" size={12} label={m.sessions_branch_label()} />
			<span class="truncate">{cwdBadge.text}{cwdBadge.worktree ? ` · ${m.spawn_cwd_worktree_badge()}` : ''}</span>
		{/if}
	</span>
</div>

<Input
	data-journey="label"
	id="sp-name"
	aria-label={m.spawn_session_name_aria()}
	placeholder={m.spawn_session_label_placeholder()}
	bind:value={form.name}
/>

<Field for="sp-prompt" label={m.spawn_prompt_label()}>
	{#snippet hint()}
		<span class="chord-hint"><Kbd keys="mod+enter" /> {m.spawn_submit_hint_spawn()}</span>
	{/snippet}
	<PromptField
		{att}
		bind:value={form.prompt}
		bind:el={promptEl}
		data-journey="prompt"
		id="sp-prompt"
		{revealed}
		rows={10}
		placeholder={m.spawn_prompt_placeholder()}
		resize="bottom"
		submitOn="mod-enter"
		onsubmit={() => onsubmit?.()}
		onkeydown={(e: KeyboardEvent) => {
			nav.handleKey(e);
		}}
	>
		{#snippet tools()}
			<PromptHistoryMenu
				onpick={(v) => {
					nav.recall(v);
					promptEl?.focus();
				}}
			/>
			{@render extraTools?.()}
		{/snippet}
	</PromptField>
	<RedactedChips text={form.prompt} bind:revealed />
</Field>

<style>
	@media (pointer: coarse) {
		.chord-hint {
			display: none;
		}
	}
	.where {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
	.where-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		min-width: 0;
	}
	.cwd {
		flex: 1 1 12rem;
		min-width: 0;
	}
	.branch {
		display: inline-flex;
		align-items: center;
		gap: 0.25em;
		min-height: 1.25rem;
		min-width: 0;
		max-width: 100%;
		padding: 0 var(--sp-1);
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		color: var(--text-faint);
	}
	.truncate {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
</style>
