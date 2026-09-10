<script lang="ts">
	// The "Machine" branch of the spawn form: where (machine badge · cwd ·
	// branch), the session name, and the prompt. The harness / account / model /
	// effort / permission knobs come from the selected profile (ProfileList).
	import type { MachineRow } from '@bindings/MachineRow';
	import { useGitInfo, useSessions } from '$lib/queries';
	import SessionMention from '$lib/components/molecules/SessionMention.svelte';
	import type { GitInfo } from '@bindings/GitInfo';
	import MachinePicker from '$lib/components/molecules/MachinePicker.svelte';
	import { Callout, Field, Icon, Input, Kbd, Link, Textarea } from '@dorsk/tsumikit';
	import { cwdSuggestions } from './cwdComplete';
	import { gitBadge, makeGitInfoWatcher } from './cwdGitInfo';
	import { makeClipboardFiles } from '$lib/attachments';
	import type { Form } from './types';
	import { m } from '$lib/paraglide/messages';
	import { promptHistory } from '$lib/drafts';
	import { HistoryNav } from '$lib/historyNav';
	import PromptHistoryMenu from '$lib/components/molecules/PromptHistoryMenu.svelte';

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

	const nav = new HistoryNav({
		list: () => promptHistory.get(),
		value: () => form.prompt,
		setValue: (v) => (form.prompt = v),
		el: () => promptEl
	});

	const clipboardFiles = makeClipboardFiles();
	function onPromptPaste(e: ClipboardEvent) {
		if (!onfiles || !e.clipboardData) return;
		const files = clipboardFiles(e.clipboardData);
		if (files.length === 0) return;
		e.preventDefault();
		onfiles(files);
	}

	let cwdOptions = $state<string[]>([]);
	let cwdToken = 0;
	async function loadCwdOptions(query: string) {
		const token = ++cwdToken;
		const opts = await cwdSuggestions(form.machine_id, query, recentDirs);
		if (token === cwdToken) cwdOptions = opts;
	}
	$effect(() => {
		void loadCwdOptions(form.working_dir);
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

{#if machines.length === 0}
	<Callout tone="warn" icon="info">
		{m.spawn_no_machines_hint()}
		<Link href="/">{m.nav_overview()}</Link>
	</Callout>
{/if}

<div class="where" data-journey="where">
	<Field label={m.spawn_cwd_label()} for="sp-cwd">
		<div class="cwd-row">
			<MachinePicker bind:value={form.machine_id} {machines} label={m.spawn_machine_label()} />
			<span class="cwd-in">
				<Input
					id="sp-cwd"
					grow
					mono
					list="sp-cwd-options"
					autocomplete="off"
					spellcheck={false}
					placeholder="/home/user/project"
					bind:value={form.working_dir}
				/>
			</span>
			<datalist id="sp-cwd-options" aria-label={m.spawn_cwd_suggestions_aria()}>
				{#each cwdOptions as d (d)}<option value={d}></option>{/each}
			</datalist>
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

<Field label={m.spawn_prompt_label()} for="sp-prompt">
	{#snippet hint()}
		<Kbd keys="mod+enter" />
		{m.spawn_submit_hint_spawn()}
	{/snippet}
	<div class="prompt-bar">
		<PromptHistoryMenu
			onpick={(v) => {
				nav.recall(v);
				promptEl?.focus();
			}}
		/>
	</div>
	<SessionMention bind:value={form.prompt} el={promptEl} sessions={mentionSessions}>
		<Textarea
			data-journey="prompt"
			id="sp-prompt"
			rows={10}
			placeholder={m.spawn_prompt_placeholder()}
			bind:value={form.prompt}
			bind:el={promptEl}
			resize="bottom"
			submitOn="mod-enter"
			onsubmit={() => onsubmit?.()}
			onpaste={onPromptPaste}
			onkeydown={(e: KeyboardEvent) => {
				nav.handleKey(e);
			}}
		/>
	</SessionMention>
</Field>

<style>
	.prompt-bar {
		display: flex;
		justify-content: flex-end;
		margin-bottom: var(--sp-1);
	}
	.cwd-row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	/* Below ~12rem of room the path drops to its own full-width line rather
	   than being squeezed to a few characters beside the machine picker. */
	.cwd-in {
		display: flex;
		flex: 1 1 12rem;
		min-width: 0;
	}
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
