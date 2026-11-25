<script lang="ts">
	import type { EventRetentionInfo } from '@bindings/EventRetentionInfo';
	import { Badge, Button, ConfirmModal, Input } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { eventRetention, setEventRetention } from '#lib/queries/index.ts';
	import { SAVE_DEBOUNCE_MS, serverSave } from './serverSave.svelte.ts';
	import { sourceLabel } from './serverSettings.logic';
	import { m } from '#lib/paraglide/messages.js';

	let info = $state<EventRetentionInfo | null>(null);
	let days = $state(0);
	let ask = $state<{ to: number; reset: boolean } | null>(null);
	let confirming = $state(false);

	function apply(next: EventRetentionInfo) {
		info = next;
		days = next.days;
	}

	$effect(() => {
		eventRetention()
			.then(apply)
			.catch(() => {});
	});

	const valid = $derived(Number.isInteger(days) && days >= 0 && days <= 36_500);
	const dirty = $derived(!!info && days !== info.days);
	const keepsLess = (from: number, to: number) => to !== 0 && (from === 0 || to < from);
	const shortens = $derived(!!info && valid && dirty && keepsLess(info.days, Math.trunc(days)));

	async function save(next: number | null) {
		const sent = days;
		const res = await serverSave.run(() => setEventRetention(next));
		if (!res) return;
		if (days === sent) apply(res);
		else info = res;
	}
	function request(to: number, reset: boolean) {
		if (info && keepsLess(info.days, to)) {
			ask = { to, reset };
			confirming = true;
		} else void save(reset ? null : to);
	}
	$effect(() => {
		if (!dirty || !valid || shortens) return;
		const next = Math.trunc(days);
		const t = setTimeout(() => save(next), SAVE_DEBOUNCE_MS);
		return () => clearTimeout(t);
	});
</script>

<SettingGroup title={m.settings_event_retention_title()}>
	<SettingRow
		label={m.settings_event_retention_label()}
		help={m.settings_event_retention_help({ days: info?.default_days ?? 90 })}
		journey="setting"
		journeyKey="event-retention"
	>
		<span class="ctl">
			<Input
				type="number"
				min="0"
				max="36500"
				step="1"
				bind:value={days}
				aria-label={m.settings_event_retention_label()}
				data-testid="event-retention-days"
			/>
			{#if info}
				<Badge size="sm">{sourceLabel(info.source)}</Badge>
			{/if}
			{#if shortens}
				<Button variant="danger" onclick={() => request(Math.trunc(days), false)}>
					{m.settings_event_retention_shorten({ days: Math.trunc(days) })}
				</Button>
			{/if}
			{#if info?.source === 'settings'}
				<Button
					variant="ghost"
					disabled={serverSave.status === 'pending'}
					onclick={() => info && request(info.default_days, true)}>{m.settings_reset()}</Button
				>
			{/if}
		</span>
	</SettingRow>
</SettingGroup>

{#if ask}
	{@const target = ask}
	<ConfirmModal
		bind:open={confirming}
		tone="danger"
		title={m.settings_event_retention_shorten_title()}
		message={m.settings_event_retention_shorten_message({ days: target.to })}
		confirmLabel={m.settings_event_retention_shorten({ days: target.to })}
		cancelLabel={m.common_cancel()}
		onconfirm={() => save(target.reset ? null : target.to)}
	/>
{/if}

<style>
	.ctl {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
