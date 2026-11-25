<script lang="ts">
	import type { EventRetentionInfo } from '@bindings/EventRetentionInfo';
	import { Badge, Button, Input } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { eventRetention, setEventRetention } from '#lib/queries/index.ts';
	import { errMessage } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { sourceLabel } from './serverSettings.logic';
	import { m } from '#lib/paraglide/messages.js';

	let info = $state<EventRetentionInfo | null>(null);
	let days = $state(0);
	let saving = $state(false);

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

	async function save(next: number | null) {
		saving = true;
		try {
			apply(await setEventRetention(next));
			toasts.ok(m.settings_event_retention_saved());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			saving = false;
		}
	}
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
			<Button disabled={!dirty || !valid || saving} onclick={() => save(Math.trunc(days))}>
				{m.settings_admin_instance_save()}
			</Button>
			{#if info?.source === 'settings'}
				<Button variant="ghost" disabled={saving} onclick={() => save(null)}>{m.settings_reset()}</Button>
			{/if}
		</span>
	</SettingRow>
</SettingGroup>

<style>
	.ctl {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
</style>
