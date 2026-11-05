<script lang="ts">
	// Instance › Limits › Event retention: how many days of lifecycle events the
	// server keeps (`0` keeps every one). Saved when the field is committed.
	import { Badge, Button, Input } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import type { EventRetentionInfo } from '@bindings/EventRetentionInfo';
	import { errMessage } from '$lib/api';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';
	import { settingsApi } from './settingsApi';
	import { parseRetentionDays, sourceLabel } from './serverSettings.logic';

	let info = $state<EventRetentionInfo | null>(null);
	let draft = $state('');
	let saving = $state(false);

	function apply(next: EventRetentionInfo) {
		info = next;
		draft = String(next.days);
	}

	$effect(() => {
		settingsApi
			.eventRetention()
			.then(apply)
			.catch(() => {});
	});

	const parsed = $derived(parseRetentionDays(draft));

	async function save(days: number | null) {
		saving = true;
		try {
			apply(await settingsApi.setEventRetention(days));
			toasts.ok(m.set_event_retention_saved());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			saving = false;
		}
	}

	function commit() {
		if (parsed === null || saving || parsed === info?.days) return;
		void save(parsed);
	}
</script>

<div id="event-retention">
	<SettingGroup title={m.set_event_retention_title()}>
		<SettingRow
			label={m.set_event_retention_label()}
			help={m.set_event_retention_help({ days: info?.default_days ?? 90 })}
		>
			<Input
				inputmode="numeric"
				bind:value={draft}
				onchange={commit}
				invalid={parsed === null}
				aria-label={m.set_event_retention_label()}
			/>
		</SettingRow>
		{#if info?.source === 'settings'}
			<SettingRow label={m.set_event_retention_reset_label()} selfLabelled>
				<span class="actions">
					<Badge size="sm">{sourceLabel(info.source)}</Badge>
					<Button variant="ghost" disabled={saving} onclick={() => save(null)}>{m.settings_reset()}</Button>
				</span>
			</SettingRow>
		{/if}
	</SettingGroup>
</div>

<style>
	.actions {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
	}
</style>
