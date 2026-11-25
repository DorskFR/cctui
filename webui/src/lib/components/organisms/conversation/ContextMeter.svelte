<script lang="ts">
	import type { SessionListItem } from '@bindings/SessionListItem';
	import type { AutoFollowupOverride } from '@bindings/AutoFollowupOverride';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Button, Popover, Text } from '@dorsk/tsumikit';
	import AutoFollowupOverrideFields from '#lib/components/molecules/AutoFollowupOverrideFields.svelte';
	import { clampOverride, INHERIT } from '#lib/autoFollowup.ts';
	import { endpoints } from '#lib/queries/index.ts';
	import { qk } from '#lib/queries/keys.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { errMessage } from '#lib/api.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { contextMeter, contextTokens } from './contextMeter';

	let { session, archived }: { session: SessionListItem; archived: boolean } = $props();

	const qc = useQueryClient();
	const meter = $derived(contextMeter(session));
	const title = $derived.by(() => {
		if (!meter) return '';
		return meter.followupAt === null
			? m.autofollowup_meter_title({ label: meter.label })
			: m.autofollowup_meter_title_followup({
					label: meter.label,
					at: contextTokens(meter.followupAt)
				});
	});

	let draft = $state<AutoFollowupOverride>({ ...INHERIT });
	let saving = $state(false);

	function seed() {
		draft = clampOverride(session.auto_followup_override);
	}

	async function apply(close: () => void) {
		saving = true;
		try {
			await endpoints.setAutoFollowup(session.id, draft);
			await qc.invalidateQueries({ queryKey: qk.sessionsAll });
			close();
		} catch (e) {
			toasts.error(m.autofollowup_save_failed({ error: errMessage(e) }));
		} finally {
			saving = false;
		}
	}
</script>

{#if meter}
	<Popover
		label={m.autofollowup_meter_aria({ label: meter.label })}
		placement="bottom-end"
		variant="ghost"
		size="sm"
		haspopup="dialog"
		panelStyle="min-width: min(17rem, calc(100vw - 2 * var(--sp-3)))"
		onopen={seed}
	>
		{#snippet trigger()}
			<span class="meter" data-testid="context-meter" {title}>
				<Text variant="code" size="xs" tone="muted">{meter.label}</Text>
				{#if meter.fill !== null}
					<span class="bar" aria-hidden="true">
						<span class="fill" style:width="{meter.fill * 100}%"></span>
						{#if meter.mark !== null}
							<span class="mark" data-testid="context-meter-mark" style:left="{meter.mark * 100}%"
							></span>
						{/if}
					</span>
				{/if}
			</span>
		{/snippet}
		{#snippet children({ close })}
			<div class="menu" data-testid="context-meter-menu">
				<Text size="xs" tone="faint" as="div">{title}</Text>
				<Text size="sm" weight="semibold" as="div">{m.autofollowup_session_title()}</Text>
				<AutoFollowupOverrideFields
					value={draft}
					onchange={(next) => (draft = next)}
					windowKnown={(session.context_window ?? null) !== null}
				/>
				<div class="actions">
					<Button
						size="sm"
						variant="primary"
						disabled={saving || archived}
						onclick={() => void apply(close)}>{m.common_apply()}</Button
					>
				</div>
			</div>
		{/snippet}
	</Popover>
{/if}

<style>
	.meter {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
		white-space: nowrap;
	}
	.bar {
		position: relative;
		display: inline-block;
		width: 2.5rem;
		height: 4px;
		border-radius: 2px;
		background: var(--border);
	}
	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		border-radius: 2px;
		background: var(--text-faint);
	}
	.mark {
		position: absolute;
		top: -2px;
		bottom: -2px;
		width: 2px;
		margin-left: -1px;
		background: var(--warn);
	}
	.menu {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
	}
</style>
