<script lang="ts">
	// Fork-conversation dialog (CCT-302), extracted from ConversationDrawer. Forks
	// the current conversation into a new session, optionally changing model/effort
	// — also the "reopen as a new conversation" path for archived sessions and the
	// supported "switch model" substitute for claude (no in-place switch, CCT-303).
	import { compact } from '$lib/format';
	import { Button, Heading, Select, Text } from '@dorsk/tsumikit';

	let {
		archived,
		isCodexSession,
		parentTokens,
		models,
		efforts,
		forking,
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
	aria-label="Cancel fork"
	onclick={oncancel}
	onkeydown={(e) => e.key === 'Escape' && oncancel()}
></div>
<div class="fork-modal" role="dialog" aria-modal="true" aria-label="Fork conversation">
	<Heading level={3}>{archived ? 'Reopen as a new conversation' : 'Fork conversation'}</Heading>
	<Text as="p" class="fork-p" tone="muted" size="sm">
		Creates a new {isCodexSession ? 'codex thread' : 'claude session'} seeded from this
		conversation's history. The original is left untouched. Adjust the model/effort below,
		or keep them to fork as-is.
	</Text>
	<Text as="p" class="fork-p fork-cost" tone="muted" size="sm">
		Your first message on the fork re-sends this conversation's history (~{compact(parentTokens)}
		tokens from the parent), so the opening turn re-bills that context.
	</Text>
	<label class="fork-field">
		<Text class="fork-label">Model</Text>
		<Select class="fork-select" bind:value={model}>
			{#each models as m (m.v)}<option value={m.v}>{m.label}</option>{/each}
		</Select>
	</label>
	<label class="fork-field">
		<Text class="fork-label">Effort</Text>
		<Select class="fork-select" bind:value={effort}>
			{#each efforts as e (e)}<option value={e}>{e || 'default'}</option>{/each}
		</Select>
	</label>
	<div class="fork-actions row">
		<Button onclick={oncancel} disabled={forking}>Cancel</Button>
		<Button variant="primary" onclick={onsubmit} disabled={forking}>
			{forking ? 'Forking…' : archived ? 'Reopen' : 'Fork'}
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
