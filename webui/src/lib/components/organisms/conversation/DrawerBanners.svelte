<script lang="ts">
	import { Button, Text } from '@dorsk/tsumikit';
	import { useAccounts } from '$lib/queries';
	import { useToolGuard, useToolGuardActions } from '$lib/queries/toolGuard';
	import { canOfferBypass, nextBypassStep, type BypassStep } from '$lib/tool-guard-bypass.logic';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import AccountSwitchModal from './AccountSwitchModal.svelte';
	import type { ConversationStream } from './stream.svelte';
	import { m } from '$lib/paraglide/messages';

	let {
		sessionId,
		stream,
		acctModalOpen = $bindable(false)
	}: {
		sessionId: string;
		/** Unused: the activity line shows the needs-input state. */
		needsInput?: boolean;
		stream: ConversationStream;
		acctModalOpen?: boolean;
	} = $props();

	const softLimit = $derived(stream.softLimit);
	const toolBlock = $derived(stream.toolBlock);

	// Accounts are fetched lazily, only while the switcher is open or a block
	// is active, so the common case pays nothing.
	const accounts = useAccounts(() => acctModalOpen || softLimit !== null);

	const guard = useToolGuard(
		() => sessionId,
		() => toolBlock !== null
	);
	const guardActions = useToolGuardActions();
	const offerBypass = $derived(toolBlock !== null && canOfferBypass(guard.data));
	let bypassStep = $state<BypassStep>('idle');
	$effect(() => {
		if (!toolBlock) bypassStep = 'idle';
	});

	async function bypass() {
		bypassStep = nextBypassStep(bypassStep, 'commit');
		try {
			await guardActions.set(sessionId, true);
			stream.dismissToolBlock();
			toasts.ok(m.tool_guard_bypassed());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			bypassStep = nextBypassStep(bypassStep, 'settled');
		}
	}

	// Auto-open the switcher the first time a given soft-limit block lands, so
	// the stalled chat surfaces a way out without hunting for the key glyph.
	let lastSoftLimitId = $state<string | null>(null);
	$effect(() => {
		const sl = softLimit;
		if (sl && sl.account_id !== lastSoftLimitId) {
			lastSoftLimitId = sl.account_id;
			acctModalOpen = true;
		} else if (!sl) {
			lastSoftLimitId = null;
		}
	});
</script>

{#if softLimit}
	<!-- Slim notice once the auto-opened modal is dismissed, so the stalled chat
	     keeps an obvious way back to the switcher. -->
	<div class="attn-banner soft-limit-notice">
		<span>{m.conversation_soft_limit_reached({ account: softLimit.account_name })}</span>
		<Button size="sm" tone="warn" onclick={() => (acctModalOpen = true)}>
			{m.conversation_switch_account()}
		</Button>
	</div>
{/if}

{#if toolBlock}
	<div class="attn-banner tool-block-notice" role="alert">
		<Text size="sm">
			{bypassStep === 'idle'
				? m.conversation_tool_call_blocked({ tool: toolBlock.tool_name, rule: toolBlock.rule })
				: m.tool_guard_bypass_confirm()}
		</Text>
		<span class="tool-block-actions">
			{#if bypassStep === 'idle'}
				{#if offerBypass}
					<Button
						size="sm"
						tone="warn"
						data-testid="tool-guard-bypass"
						onclick={() => (bypassStep = nextBypassStep(bypassStep, 'ask'))}
					>
						{m.tool_guard_bypass()}
					</Button>
				{/if}
				<Button size="sm" variant="ghost" onclick={() => stream.dismissToolBlock()}>
					{m.conversation_tool_block_dismiss()}
				</Button>
			{:else}
				<Button
					size="sm"
					tone="warn"
					data-testid="tool-guard-bypass-confirm"
					disabled={bypassStep === 'busy'}
					onclick={bypass}
				>
					{m.tool_guard_bypass_confirm_yes()}
				</Button>
				<Button
					size="sm"
					variant="ghost"
					disabled={bypassStep === 'busy'}
					onclick={() => (bypassStep = nextBypassStep(bypassStep, 'cancel'))}
				>
					{m.common_cancel()}
				</Button>
			{/if}
		</span>
	</div>
{/if}

{#if acctModalOpen}
	<AccountSwitchModal
		{sessionId}
		accounts={accounts.data ?? []}
		{softLimit}
		onswitch={(acct) => stream.switchAccount(acct)}
		onclose={() => (acctModalOpen = false)}
	/>
{/if}

<style>
	.attn-banner {
		padding: var(--sp-2) var(--sp-3);
		background: var(--attention-bg);
		border-bottom: 1px solid var(--attention-bar);
		color: var(--warn);
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
	}
	.soft-limit-notice,
	.tool-block-notice {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
	}
	.tool-block-actions {
		display: inline-flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--sp-2);
	}
</style>
