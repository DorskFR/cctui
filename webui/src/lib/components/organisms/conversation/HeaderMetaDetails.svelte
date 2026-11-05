<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import { compact, usd } from '$lib/format';
	import { tokenUsageLayout, bustReasonKey } from '$lib/components/molecules/TokenUsage.logic';
	import LangfuseChip from '$lib/components/molecules/LangfuseChip.svelte';
	import { Badge, Switch, Text } from '@dorsk/tsumikit';
	import { useToolGuard, useToolGuardActions } from '$lib/queries/toolGuard';
	import { useSessionHookPolicy } from '$lib/queries/hookPolicy';
	import { describeRule, hookChip } from './hook-policy.logic';
	import { canOfferBypass } from '$lib/tool-guard-bypass.logic';
	import { showHarnessEstimate, type HeaderCost } from './headerMeta.logic';
	import HeaderModel from './HeaderModel.svelte';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';

	let {
		session,
		archived,
		isCodexSession,
		open,
		cost,
		calls,
		onsetmodel,
		onfork
	}: {
		session: SessionListItem;
		archived: boolean;
		isCodexSession: boolean;
		/** Gates the guard and hook reads to while the panel is showing. */
		open: boolean;
		cost: HeaderCost | null;
		calls: number;
		onsetmodel: (model: string, effort: string) => void;
		onfork: () => void;
	} = $props();

	const usage = $derived(session.token_usage);
	const totals = $derived(tokenUsageLayout(usage));
	const harnessUsd = $derived(Number(usage.cost_usd) || 0);
	const bust = $derived(usage.cache_bust ?? null);
	const bustReason = $derived.by(() => {
		switch (bustReasonKey(bust?.reason ?? '')) {
			case 'ttl_expired':
				return m.sessions_token_bust_reason_ttl_expired();
			case 'provider_miss':
				return m.sessions_token_bust_reason_provider_miss();
			default:
				return m.sessions_token_bust_reason_unknown();
		}
	});

	const toolGuard = useToolGuard(
		() => session.id,
		() => open
	);
	const toolGuardActions = useToolGuardActions();
	const hookPolicy = useSessionHookPolicy(
		() => session.id,
		() => open
	);
	const hooks = $derived(hookChip(hookPolicy.data));
	const toolGuardSwitch = $derived(canOfferBypass(toolGuard.data, archived));
	let toolGuardSaving = $state(false);
	async function setToolGuard(on: boolean) {
		toolGuardSaving = true;
		try {
			await toolGuardActions.set(session.id, !on);
			toasts.ok(on ? m.tool_guard_restored() : m.tool_guard_bypassed());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			toolGuardSaving = false;
		}
	}
</script>

<div class="metapop">
	<div class="mp-row">
		<span class="mp-key"><Text size="xs" tone="muted">{m.drawer_details_model()}</Text></span>
		<span class="mp-val">
			<HeaderModel {session} {archived} {isCodexSession} idPrefix="drawer-details" inPopover {onsetmodel} {onfork} />
		</span>
	</div>
	<div class="mp-sep"></div>
	<div class="mp-grid">
		{#each [
			{ k: m.drawer_details_in(), v: compact(Number(usage.tokens_in)) },
			{ k: m.drawer_details_out(), v: compact(Number(usage.tokens_out)) },
			{ k: m.drawer_details_cache_read(), v: compact(Number(usage.cache_read_tokens)) },
			{ k: m.drawer_details_cache_write(), v: compact(Number(usage.cache_creation_tokens)) }
		] as row (row.k)}
			<span class="mp-key"><Text size="xs" tone="muted">{row.k}</Text></span>
			<span class="mp-num"><Text numeric size="xs">{row.v}</Text></span>
		{/each}
		<span class="mp-key"><Text size="xs" weight="semibold">{m.drawer_details_total()}</Text></span>
		<span class="mp-num"><Text numeric size="xs" weight="semibold">{compact(totals.total)}</Text></span>
		<span class="mp-key"><Text size="xs" weight="semibold">{m.drawer_details_cost()}</Text></span>
		<span class="mp-num" data-testid="header-cost">
			{#if cost?.source === 'langfuse'}
				<LangfuseChip id={session.id} />
			{:else if cost}
				<Text numeric size="xs" weight="semibold">{m.chrome_cost_estimate({ cost: usd(cost.value) })}</Text>
			{:else}
				<Text size="xs" tone="muted">{m.chrome_cost_plan()}</Text>
			{/if}
		</span>
		{#if showHarnessEstimate(cost, harnessUsd)}
			<span class="mp-key"><Text size="xs" tone="muted">{m.chrome_cost_harness_row()}</Text></span>
			<span class="mp-num"><Text numeric size="xs" tone="muted">{usd(harnessUsd)}</Text></span>
		{/if}
	</div>
	{#if bust}
		<div class="mp-row">
			<span class="mp-key"><Text size="xs" tone="muted">{m.drawer_details_bust()}</Text></span>
			<span class="mp-val"
				><Text size="xs" tone="danger"
					>{m.sessions_token_bust_hint({
						tokens: compact(Number(bust.lost_tokens)),
						cost: usd(Number(bust.lost_usd)),
						reason: bustReason
					})}</Text
				></span
			>
		</div>
	{/if}
	{#if calls > 0}
		<div class="mp-row">
			<span class="mp-key"><Text size="xs" tone="muted">{m.drawer_details_calls()}</Text></span>
			<span class="mp-num"><Text numeric size="xs">{calls}</Text></span>
		</div>
	{/if}
	{#if hooks.kind !== 'none'}
		<div class="mp-sep"></div>
		<div class="mp-row" data-testid="hook-policy">
			<span class="mp-key"><Text size="xs" tone="muted">{m.hook_policy_title()}</Text></span>
			<span class="mp-val">
				{#if hooks.kind === 'applied'}
					<Badge size="xs">{m.hook_policy_count({ count: hooks.rules.length })}</Badge>
				{:else}
					<Badge size="xs" tone="warn" title={m.hook_policy_unguarded_help()}>{m.hook_policy_unguarded()}</Badge>
				{/if}
			</span>
		</div>
		<ul class="mp-hooks">
			{#each hooks.rules as rule, i (i)}
				<li><Text variant="code" size="xs" tone="muted">{describeRule(rule)}</Text></li>
			{/each}
		</ul>
	{/if}
	{#if toolGuardSwitch && toolGuard.data}
		{@const tg = toolGuard.data}
		<div class="mp-sep"></div>
		<div class="mp-row" data-testid="tool-guard-switch">
			<span class="mp-key"><Text size="xs" tone="muted">{m.tool_guard_label()}</Text></span>
			<span class="mp-val">
				<Switch
					size="sm"
					labelVisible
					label={tg.bypass ? m.tool_guard_off() : m.tool_guard_on()}
					disabled={toolGuardSaving}
					bind:checked={() => !tg.bypass, (on) => void setToolGuard(on)}
				/>
			</span>
		</div>
		{#if tg.bypass && tg.by}
			<Text size="xs" tone="muted">{m.tool_guard_off_by({ name: tg.by })}</Text>
		{/if}
	{/if}
</div>

<style>
	/* The meta row degrades to a logo on a phone, so this panel is the only
	   place the whole readout is reachable. */
	.metapop {
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: var(--sp-2);
		min-width: 0;
		max-width: 100%;
	}
	.mp-grid {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: var(--sp-1) var(--sp-3);
		align-items: baseline;
	}
	.mp-row {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--sp-3);
		min-width: 0;
	}
	.mp-key {
		min-width: 0;
	}
	.mp-row > .mp-key {
		flex: none;
		white-space: nowrap;
	}
	.mp-num {
		justify-self: end;
		white-space: nowrap;
	}
	.mp-val {
		display: inline-flex;
		flex: 1 1 auto;
		justify-content: flex-end;
		align-items: center;
		min-width: 0;
		text-align: right;
	}
	.mp-sep {
		border-top: 1px solid var(--border);
	}
	.mp-hooks {
		margin: 0;
		padding: 0;
		list-style: none;
		min-width: 0;
		overflow-wrap: anywhere;
	}
</style>
