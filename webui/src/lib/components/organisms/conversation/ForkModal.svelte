<script lang="ts">
	// Fork-conversation dialog (CCT-302), extracted from ConversationDrawer. Forks
	// the current conversation into a new session, optionally changing model/effort
	// — also the "reopen as a new conversation" path for archived sessions and the
	// supported "switch model" substitute for claude (no in-place switch, CCT-303).
	import { compact } from '$lib/format';
	import { Button, Heading, Select, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		archived,
		isCodexSession,
		parentTokens,
		models,
		efforts,
		forking,
		extractLabel = null,
		model = $bindable(),
		effort = $bindable(),
		oncancel,
		onsubmit
	}: {
		archived: boolean;
		isCodexSession: boolean;
		// Parent's total tokens — shown so the user knows the opening turn re-bills
		// this much context (CCT-345).
		parentTokens: number;
		models: { v: string; label: string }[];
		efforts: string[];
		forking: boolean;
		// Non-null → subset fork (CCT-553): the slice of the conversation to keep.
		extractLabel?: string | null;
		model: string;
		effort: string;
		oncancel: () => void;
		onsubmit: () => void;
	} = $props();
</script>

<div
	class="fork-scrim"
	role="button"
	tabindex="-1"
	aria-label={m.fork_cancel_aria()}
	onclick={oncancel}
	onkeydown={(e) => e.key === 'Escape' && oncancel()}
></div>
<div class="fork-modal" role="dialog" aria-modal="true" aria-label={m.fork_dialog_aria()}>
	<Heading level={3}
		>{extractLabel
			? m.fork_title_extract()
			: archived
				? m.fork_title_reopen()
				: m.fork_title()}</Heading
	>
	{#if extractLabel}
		<Text as="p" class="fork-p fork-extract" tone="accent" size="sm">
			{m.fork_extract_desc({ label: extractLabel })}
		</Text>
	{:else}
		<Text as="p" class="fork-p" tone="muted" size="sm">
			{m.fork_desc({ target: isCodexSession ? m.fork_target_codex() : m.fork_target_claude() })}
		</Text>
	{/if}
	{#if !extractLabel}
		<Text as="p" class="fork-p fork-cost" tone="muted" size="sm">
			{m.fork_cost({ tokens: compact(parentTokens) })}
		</Text>
	{/if}
	<label class="fork-field">
		<Text class="fork-label">{m.fork_model()}</Text>
		<Select class="fork-select" bind:value={model}>
			{#each models as opt (opt.v)}<option value={opt.v}>{opt.label}</option>{/each}
		</Select>
	</label>
	<label class="fork-field">
		<Text class="fork-label">{m.fork_effort()}</Text>
		<Select class="fork-select" bind:value={effort}>
			{#each efforts as e (e)}<option value={e}>{e || m.fork_default()}</option>{/each}
		</Select>
	</label>
	<div class="fork-actions row">
		<Button onclick={oncancel} disabled={forking}>{m.common_cancel()}</Button>
		<Button variant="primary" onclick={onsubmit} disabled={forking}>
			{forking ? m.fork_forking() : archived ? m.fork_reopen() : m.fork_fork()}
		</Button>
	</div>
</div>

<style>
	.fork-scrim {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		z-index: 200;
		border: 0;
	}
	.fork-modal {
		position: fixed;
		top: 50%;
		left: 50%;
		transform: translate(-50%, -50%);
		z-index: 201;
		width: min(420px, calc(100vw - 2rem));
		background: var(--bg, #1a1a1a);
		border: 1px solid var(--border, #333);
		border-radius: 10px;
		padding: 1.1rem 1.2rem;
		box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
	}
	/* h3/p/label-span typography is now the Heading/Text atoms'. These classes ride
	   on atom children, so the residual layout (margins, line-height, label width)
	   must be :global to reach them. */
	.fork-modal :global(.heading) {
		margin: 0 0 0.4rem;
	}
	.fork-modal :global(.fork-p) {
		margin: 0 0 0.9rem;
		line-height: 1.4;
	}
	.fork-field {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		margin-bottom: 0.7rem;
		font-size: 0.9rem;
	}
	.fork-field :global(.fork-label) {
		width: 4.5rem;
		flex: 0 0 auto;
	}
	/* tsumikit's default Select wraps the <select> in .select-wrap; grow that
	   wrapper to fill the flex row (the old bare select carried flex:1 itself). */
	.fork-field :global(.fork-select) {
		flex: 1;
	}
	.fork-actions {
		justify-content: flex-end;
		gap: 0.8rem;
		margin-top: 0.4rem;
	}
</style>
