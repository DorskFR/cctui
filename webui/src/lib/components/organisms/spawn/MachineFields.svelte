<script lang="ts">
	// The "Machine" branch of the spawn form: where (machine badge · cwd ·
	// branch), the session name, and the prompt. The harness / account / model /
	// effort / permission knobs come from the selected profile (ProfileList).
	import type { MachineRow } from '@bindings/MachineRow';
	import { useGitInfo, useSessions } from '$lib/queries';
	import SessionMention from '$lib/components/molecules/SessionMention.svelte';
	import type { GitInfo } from '@bindings/GitInfo';
	import MachinePicker from '$lib/components/molecules/MachinePicker.svelte';
	import { FilterInput, Icon, Input, Textarea, type Query } from '@dorsk/tsumikit';
	import { makeCwdSchema, cwdToQuery, dirFromQuery } from './cwdSchema';
	import { gitBadge, makeGitInfoWatcher } from './cwdGitInfo';
	import { submitChordLabel, isSubmitChord } from '$lib/platform';
	import { makeClipboardFiles } from '$lib/attachments';
	import type { Form } from './types';
	import { m } from '$lib/paraglide/messages';

	let {
		form = $bindable(),
		machines,
		recentDirs,
		onsubmit,
		onfiles
	}: {
		form: Form;
		machines: MachineRow[];
		recentDirs: string[];
		onsubmit?: () => void;
		// Files pasted into the prompt (a screenshot, a copied file) go to the
		// attachments; text pastes are left to the browser.
		onfiles?: (files: File[]) => void;
	} = $props();

	// `#` session-mention popover on the prompt (see SessionMention).
	const sessionsQuery = useSessions(() => false);
	const mentionSessions = $derived(sessionsQuery.data?.sessions ?? []);
	let promptEl = $state<HTMLTextAreaElement | null>(null);

	// The machine picker + working dir share one FilterInput; `form.working_dir`
	// is the source of truth and the raw query mirrors it both ways, `lastDir`
	// tracking what the query represents so the two syncs never loop.
	const cwdSchema = makeCwdSchema(
		() => form.machine_id,
		() => recentDirs,
		m.spawn_cwd_label()
	);
	const clipboardFiles = makeClipboardFiles();
	function onPromptPaste(e: ClipboardEvent) {
		if (!onfiles || !e.clipboardData) return;
		const files = clipboardFiles(e.clipboardData);
		if (files.length === 0) return;
		e.preventDefault();
		onfiles(files);
	}

	// svelte-ignore state_referenced_locally
	let cwdRaw = $state(cwdToQuery(form.working_dir));
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
			cwdRaw = cwdToQuery(dir);
		}
	});

	const fetchGitInfo = useGitInfo();
	let cwdGit = $state<GitInfo | null>(null);
	const cwdBadge = $derived(gitBadge(cwdGit));
	const gitWatcher = makeGitInfoWatcher(fetchGitInfo, (info) => (cwdGit = info));
	$effect(() => {
		gitWatcher.update(form.machine_id, form.working_dir);
		return gitWatcher.cancel;
	});
	const cwdBadgeTitle = $derived.by(() => {
		if (!cwdBadge) return '';
		if (cwdBadge.sha) return m.spawn_cwd_detached_title({ sha: cwdBadge.sha });
		if (cwdBadge.worktree) return m.spawn_cwd_worktree_title({ branch: cwdBadge.text });
		return m.spawn_cwd_branch_title({ branch: cwdBadge.text });
	});
</script>

<div class="where">
	<FilterInput
		schema={cwdSchema}
		bind:value={cwdRaw}
		icon={null}
		showClear={false}
		placeholder="/home/user/project"
		onchange={onCwdChange}
	>
		{#snippet inline()}
			<MachinePicker bind:value={form.machine_id} {machines} label={m.spawn_machine_label()} />
		{/snippet}
	</FilterInput>
	<!-- Always one line tall so the form doesn't jump when a branch resolves. -->
	<span class="branch" title={cwdBadge ? cwdBadgeTitle : undefined}>
		{#if cwdBadge}
			<Icon name="fork" size={12} label={m.sessions_branch_label()} />
			<span class="truncate">{cwdBadge.text}{cwdBadge.worktree ? ` · ${m.spawn_cwd_worktree_badge()}` : ''}</span>
		{/if}
	</span>
</div>

<Input
	id="sp-name"
	aria-label={m.spawn_session_name_aria()}
	placeholder={m.spawn_session_label_placeholder()}
	bind:value={form.name}
/>

<SessionMention bind:value={form.prompt} el={promptEl} sessions={mentionSessions}>
	<Textarea
		id="sp-prompt"
		rows={10}
		aria-label={m.spawn_prompt_label()}
		placeholder={m.spawn_prompt_placeholder_chord({ chord: submitChordLabel() })}
		bind:value={form.prompt}
		bind:el={promptEl}
		resize="bottom"
		onpaste={onPromptPaste}
		onkeydown={(e: KeyboardEvent) => {
			if (onsubmit && isSubmitChord(e)) {
				e.preventDefault();
				onsubmit();
			}
		}}
	/>
</SessionMention>

<style>
	.where {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
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
